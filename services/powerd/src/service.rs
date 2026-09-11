use alloc::vec::Vec;
use bexos_userspace::live_migration::Source;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, KernelTransport, Memory, Rpc, Startup, log};
use kernel_fidl::{
    HandleRef as KernelHandleRef, Status as KernelStatus,
    SystemPrivilegedBexosSystemPrivilegedClient, SystemPrivilegedGetResourceGroupV2Request,
    SystemPrivilegedOpenResourceGroupRequest, SystemPrivilegedRequestSystemPowerStateRequest,
    SystemPrivilegedSetResourceGroupLimitsV2Request,
};
use power_fidl::{
    DevicePowerControlPublicClient, DevicePowerControlSetPowerStateRequest, DevicePowerState,
    FidlDecode, HandleRef, PerformanceControlPublicClient,
    PerformanceControlSetPerformanceLevelRequest, PowerManagerAcquireWakeLeaseRequest,
    PowerManagerAcquireWakeLeaseResponse, PowerManagerGetPowerSnapshotRequest,
    PowerManagerGetPowerSnapshotResponse, PowerManagerRequestSystemStateRequest,
    PowerManagerRequestSystemStateResponse, PowerManagerWatchPowerSnapshotsRequest,
    PowerManagerWatchPowerSnapshotsResponse, PowerSnapshot, PowerTelemetryGetTelemetryRequest,
    PowerTelemetryPublicClient, PowerWatcherOnPowerSnapshotRequest, PowerWatcherPublicClient,
    Status, SystemPowerState,
};

use crate::migration::Runtime;
use crate::wire::{
    envelope, from_kernel_status, handle_refs, metadata_protocol, send_response, to_kernel_state,
};
use crate::{PowerPolicy, TelemetrySample};

const BACKGROUND_RESOURCE_GROUP_ID: u32 = 3;
const TELEMETRY_POLL_MS: u64 = 1_000;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("powerd startup");
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state).await,
            Err(_) => bexos_userspace::exit(),
        }
    }
    let mut policy = PowerPolicy::new();
    for grant in startup.service_grants {
        match grant.protocol.as_str() {
            "DevicePowerControl" => {
                let _ = policy.register_device(grant.endpoint);
            }
            "PowerTelemetry" => {
                let _ = policy.register_telemetry_provider(grant.endpoint);
            }
            "PerformanceControl" => {
                let _ = policy.register_performance_provider(grant.endpoint);
            }
            _ => {}
        }
    }
    Startup::ready(control).unwrap();
    log("powerd: ready\n");
    serve(Runtime::new(control, startup.migration, policy)).await
}

async fn serve(mut runtime: Runtime) -> ! {
    let control = runtime.control;
    let mut source = Source::new(runtime.migration);
    let mut system = SystemPrivilegedBexosSystemPrivilegedClient::new(KernelTransport(4));
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if retain_live_leases(&mut runtime.policy) {
            notify_power_watchers(&mut runtime.watchers, &runtime.policy);
            source.changed(4);
        }
        let now = now_ms();
        if runtime.next_telemetry_poll_ms == 0 || now >= runtime.next_telemetry_poll_ms {
            poll_policy_inputs(&mut runtime.policy, &mut system);
            notify_power_watchers(&mut runtime.watchers, &runtime.policy);
            runtime.next_telemetry_poll_ms = now.saturating_add(TELEMETRY_POLL_MS);
            source.changed_keys([2, 4, 6]);
        }
        if let Ok(message) = control.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                match metadata_protocol(metadata) {
                    Some("PowerManager") => {}
                    Some("DevicePowerControl") => {
                        if runtime.policy.register_device(endpoint) == Status::Ok {
                            source.changed(2);
                            log("powerd: registered device power endpoint\n");
                        }
                    }
                    Some("PowerTelemetry") => {
                        if runtime.policy.register_telemetry_provider(endpoint) == Status::Ok {
                            source.changed(6);
                            log("powerd: registered telemetry endpoint\n");
                        }
                    }
                    Some("PerformanceControl") => {
                        if runtime.policy.register_performance_provider(endpoint) == Status::Ok {
                            source.changed(6);
                            log("powerd: registered performance endpoint\n");
                        }
                    }
                    _ => {}
                }
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("PowerManager") {
                        runtime.clients.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                        source.changed(1);
                    }
                }
            }
        }
        if poll_power_clients(
            &mut runtime.clients,
            &mut runtime.watchers,
            &mut runtime.policy,
            &mut system,
        ) {
            notify_power_watchers(&mut runtime.watchers, &runtime.policy);
            source.changed_keys([1, 3, 4, 5]);
        }
        bexos_userspace::yield_now();
    }
}

fn poll_power_clients(
    clients: &mut Vec<BoundServiceEndpoint>,
    watchers: &mut Vec<u64>,
    policy: &mut PowerPolicy,
    system: &mut SystemPrivilegedBexosSystemPrivilegedClient<KernelTransport>,
) -> bool {
    let mut changed = false;
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if !client.allows(ordinal) {
                if ordinal == 1 {
                    send_response(
                        client.channel,
                        &PowerManagerAcquireWakeLeaseResponse {
                            status: Status::ErrInvalidArgs,
                            lease_handle: HandleRef { raw: 0 },
                        },
                    );
                } else if ordinal == 2 {
                    send_response(
                        client.channel,
                        &PowerManagerRequestSystemStateResponse {
                            status: Status::ErrInvalidArgs,
                        },
                    );
                } else if ordinal == 3 {
                    send_response(
                        client.channel,
                        &PowerManagerGetPowerSnapshotResponse {
                            status: Status::ErrInvalidArgs,
                            snapshot: power_snapshot(policy),
                        },
                    );
                } else if ordinal == 4 {
                    send_response(
                        client.channel,
                        &PowerManagerWatchPowerSnapshotsResponse {
                            status: Status::ErrInvalidArgs,
                        },
                    );
                }
                return true;
            }
            match ordinal {
                1 => {
                    let response = match PowerManagerAcquireWakeLeaseRequest::decode(req, &handles)
                    {
                        Ok(request) => acquire_wake_lease(policy, request.reason),
                        Err(_) => PowerManagerAcquireWakeLeaseResponse {
                            status: Status::ErrInvalidArgs,
                            lease_handle: HandleRef { raw: 0 },
                        },
                    };
                    changed = true;
                    send_response(client.channel, &response);
                }
                2 => {
                    let response =
                        match PowerManagerRequestSystemStateRequest::decode(req, &handles) {
                            Ok(request) => request_system_state(policy, system, request.state),
                            Err(_) => PowerManagerRequestSystemStateResponse {
                                status: Status::ErrInvalidArgs,
                            },
                        };
                    changed = true;
                    send_response(client.channel, &response);
                }
                3 => {
                    let response =
                        if PowerManagerGetPowerSnapshotRequest::decode(req, &handles).is_ok() {
                            PowerManagerGetPowerSnapshotResponse {
                                status: Status::Ok,
                                snapshot: power_snapshot(policy),
                            }
                        } else {
                            PowerManagerGetPowerSnapshotResponse {
                                status: Status::ErrInvalidArgs,
                                snapshot: power_snapshot(policy),
                            }
                        };
                    changed = true;
                    send_response(client.channel, &response);
                }
                4 => {
                    let response =
                        match PowerManagerWatchPowerSnapshotsRequest::decode(req, &handles) {
                            Ok(request) if request.watcher.raw != 0 => {
                                watchers.push(request.watcher.raw);
                                notify_power_watchers(watchers, policy);
                                PowerManagerWatchPowerSnapshotsResponse { status: Status::Ok }
                            }
                            _ => PowerManagerWatchPowerSnapshotsResponse {
                                status: Status::ErrInvalidArgs,
                            },
                        };
                    changed = true;
                    send_response(client.channel, &response);
                }
                _ => {}
            }
            true
        }
        Err(KernelStatus::ErrPeerClosed) => {
            changed = true;
            false
        }
        Err(_) => true,
    });
    changed
}

fn notify_power_watchers(watchers: &mut Vec<u64>, policy: &PowerPolicy) {
    let snapshot = power_snapshot(policy);
    watchers.retain(|watcher| {
        let mut client = PowerWatcherPublicClient::new(Rpc(Channel(*watcher)));
        let mut request_bytes = [0; 128];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        client
            .on_power_snapshot(
                &PowerWatcherOnPowerSnapshotRequest { snapshot },
                &mut request_bytes,
                &mut request_handles,
            )
            .is_ok()
    });
}

fn power_snapshot(policy: &PowerPolicy) -> PowerSnapshot {
    policy.snapshot()
}

fn acquire_wake_lease(
    policy: &mut PowerPolicy,
    reason: &str,
) -> PowerManagerAcquireWakeLeaseResponse {
    match Channel::pair() {
        Ok((lease_server, lease_client)) => {
            let status = policy.add_lease(lease_server.0, reason);
            let lease_handle = if status == Status::Ok {
                HandleRef {
                    raw: lease_client.0,
                }
            } else {
                HandleRef { raw: 0 }
            };
            PowerManagerAcquireWakeLeaseResponse {
                status,
                lease_handle,
            }
        }
        Err(_) => PowerManagerAcquireWakeLeaseResponse {
            status: Status::ErrNoMemory,
            lease_handle: HandleRef { raw: 0 },
        },
    }
}

fn request_system_state(
    policy: &mut PowerPolicy,
    system: &mut SystemPrivilegedBexosSystemPrivilegedClient<KernelTransport>,
    state: SystemPowerState,
) -> PowerManagerRequestSystemStateResponse {
    let local_status = policy.request_system_state(state);
    if local_status != Status::Ok || state == SystemPowerState::Active {
        return PowerManagerRequestSystemStateResponse {
            status: local_status,
        };
    }
    let down_status = set_devices(policy.device_endpoints(), DevicePowerState::D3Off);
    if down_status != Status::Ok {
        return PowerManagerRequestSystemStateResponse {
            status: down_status,
        };
    }
    let mut request_bytes = [0; 16];
    let mut response_bytes = [0; 16];
    let mut request_handles = [KernelHandleRef { raw: 0 }; 1];
    let mut response_handles = [KernelHandleRef { raw: 0 }; 1];
    let status = match system.request_system_power_state(
        &SystemPrivilegedRequestSystemPowerStateRequest {
            state: to_kernel_state(state),
        },
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    ) {
        Ok(response) => from_kernel_status(response.status),
        Err(_) => Status::ErrInvalidArgs,
    };
    if status != Status::Ok || state == SystemPowerState::SuspendToRam {
        let _ = restore_devices(policy.device_endpoints(), policy.device_endpoints().len());
    }
    PowerManagerRequestSystemStateResponse { status }
}

fn set_devices(devices: &[u64], state: DevicePowerState) -> Status {
    for (index, endpoint) in devices.iter().enumerate() {
        let mut client = DevicePowerControlPublicClient::new(Rpc(Channel(*endpoint)));
        let mut request_bytes = [0; 8];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        match client.set_power_state(
            &DevicePowerControlSetPowerStateRequest { state },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        ) {
            Ok(response) if response.status == Status::Ok => {}
            Ok(response) => {
                let _ = restore_devices(devices, index);
                return response.status;
            }
            Err(_) => {
                let _ = restore_devices(devices, index);
                return Status::ErrTimedOut;
            }
        }
    }
    Status::Ok
}

fn restore_devices(devices: &[u64], transitioned: usize) -> Status {
    let mut status = Status::Ok;
    for endpoint in devices.iter().take(transitioned).rev() {
        let mut client = DevicePowerControlPublicClient::new(Rpc(Channel(*endpoint)));
        let mut request_bytes = [0; 8];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        match client.set_power_state(
            &DevicePowerControlSetPowerStateRequest {
                state: DevicePowerState::D0FullPower,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        ) {
            Ok(response) if response.status == Status::Ok => {}
            Ok(response) => status = response.status,
            Err(_) => status = Status::ErrTimedOut,
        }
    }
    status
}

fn poll_policy_inputs(
    policy: &mut PowerPolicy,
    system: &mut SystemPrivilegedBexosSystemPrivilegedClient<KernelTransport>,
) {
    let sample = read_telemetry(policy.telemetry_provider);
    let applied = policy.apply_telemetry_result(sample);
    if applied.performance_level_changed {
        apply_performance_provider(policy);
    }
    apply_background_cap(policy, system, applied.background_cpu_cap_permille);
}

fn read_telemetry(endpoint: Option<u64>) -> Result<TelemetrySample, Status> {
    let Some(endpoint) = endpoint else {
        return Err(Status::ErrUnsupported);
    };
    let mut client = PowerTelemetryPublicClient::new(Rpc(Channel(endpoint)));
    let mut request_bytes = [0; 16];
    let mut response_bytes = [0; 64];
    let mut request_handles = [HandleRef { raw: 0 }; 1];
    let mut response_handles = [HandleRef { raw: 0 }; 1];
    match client.get_telemetry(
        &PowerTelemetryGetTelemetryRequest {},
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    ) {
        Ok(response) if response.status == Status::Ok => Ok(TelemetrySample {
            battery_available: response.battery_available,
            charging: response.charging,
            battery_percent: response.battery_percent,
            thermal_available: response.thermal_available,
            temperature_celsius: response.temperature_celsius,
        }),
        Ok(response) => Err(response.status),
        Err(_) => Err(Status::ErrIo),
    }
}

fn apply_performance_provider(policy: &PowerPolicy) {
    let Some(endpoint) = policy.performance_provider else {
        return;
    };
    let mut client = PerformanceControlPublicClient::new(Rpc(Channel(endpoint)));
    let mut request_bytes = [0; 16];
    let mut response_bytes = [0; 16];
    let mut request_handles = [HandleRef { raw: 0 }; 1];
    let mut response_handles = [HandleRef { raw: 0 }; 1];
    let _ = client.set_performance_level(
        &PerformanceControlSetPerformanceLevelRequest {
            level: policy.applied_performance_level,
        },
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    );
}

fn apply_background_cap(
    policy: &mut PowerPolicy,
    system: &mut SystemPrivilegedBexosSystemPrivilegedClient<KernelTransport>,
    cap: u16,
) {
    let mut request_bytes = [0; 32];
    let mut response_bytes = [0; 128];
    let mut request_handles = [KernelHandleRef { raw: 0 }; 1];
    let mut response_handles = [KernelHandleRef { raw: 0 }; 1];
    let Ok(open) = system.open_resource_group(
        &SystemPrivilegedOpenResourceGroupRequest {
            resource_group_id: BACKGROUND_RESOURCE_GROUP_ID,
        },
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    ) else {
        return;
    };
    if open.status != KernelStatus::Ok || open.group_handle.raw == 0 {
        return;
    }
    let mut get_req = [0; 32];
    let mut get_resp = [0; 160];
    let Ok(current) = system.get_resource_group_v2(
        &SystemPrivilegedGetResourceGroupV2Request {
            group_handle: open.group_handle,
        },
        &mut get_req,
        &mut request_handles,
        &mut get_resp,
        &mut response_handles,
    ) else {
        let _ = Memory::close(open.group_handle.raw);
        return;
    };
    if current.status != KernelStatus::Ok {
        let _ = Memory::close(open.group_handle.raw);
        return;
    }
    if policy.baseline_background_cpu_cap_permille.is_none() {
        policy.baseline_background_cpu_cap_permille = Some(current.max_cpu_utilization_permille);
    }
    if current.max_cpu_utilization_permille == cap {
        let _ = Memory::close(open.group_handle.raw);
        return;
    }
    let mut set_req = [0; 96];
    let mut set_resp = [0; 16];
    let _ = system.set_resource_group_limits_v2(
        &SystemPrivilegedSetResourceGroupLimitsV2Request {
            group_handle: open.group_handle,
            cpu_shares: current.cpu_shares,
            max_cpu_utilization_permille: cap,
            allow_realtime: current.allow_realtime,
            memory_low_watermark_bytes: current.memory_low_watermark_bytes,
            memory_high_watermark_bytes: current.memory_high_watermark_bytes,
            max_render_budget_percent: current.max_render_budget_percent,
            max_vram_bytes: current.max_vram_bytes,
        },
        &mut set_req,
        &mut request_handles,
        &mut set_resp,
        &mut response_handles,
    );
    let _ = Memory::close(open.group_handle.raw);
}

fn retain_live_leases(policy: &mut PowerPolicy) -> bool {
    let expired: Vec<_> = policy
        .leases()
        .iter()
        .filter_map(|lease| match Channel(lease.handle).try_recv() {
            Err(KernelStatus::ErrPeerClosed) => Some(lease.handle),
            _ => None,
        })
        .collect();
    let changed = !expired.is_empty();
    for handle in expired {
        let _ = policy.remove_lease(handle);
    }
    changed
}

fn now_ms() -> u64 {
    let ticks = bexos_userspace::syscall::ticks();
    let freq = bexos_userspace::syscall::frequency().max(1);
    (u128::from(ticks) * 1_000u128 / u128::from(freq)) as u64
}
