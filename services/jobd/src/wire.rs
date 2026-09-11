use alloc::string::ToString;
use alloc::vec::Vec;
use app_worker_fidl::{
    FidlDecode as WorkerFidlDecode, HandleRef as WorkerHandleRef, PackagePolicyEventKind,
    PackagePolicyWatcherOnPackagePolicyChangedRequest, WorkerLaunchStatus,
    WorkerLauncherGetJobDeclarationsRequest, WorkerLauncherGetJobDeclarationsResponse,
    WorkerLauncherSpawnWorkerRequest, WorkerLauncherSpawnWorkerResponse,
    WorkerLauncherStopWorkerRequest, WorkerLauncherStopWorkerResponse,
    WorkerLauncherWatchPackagePolicyRequest, WorkerLauncherWatchPackagePolicyResponse,
};
use bexos_job_store::{
    ClockSnapshot, JobConstraints, JobRecord, JobSpec, JobTimebase, NetworkRequirement,
    RuntimeConditions,
};
use bexos_userspace::service_binding::ServiceBinding;
use bexos_userspace::{Channel, KernelTransport, Memory, Rpc};
use job_fidl::{
    FidlDecode, FidlEncode, HandleRef, JobControlCompleteRequest, JobControlCompleteResponse,
    SchedulerCancelJobRequest, SchedulerCancelJobResponse, SchedulerGetJobRequest,
    SchedulerGetJobResponse, SchedulerListJobsRequest, SchedulerListJobsResponse,
    SchedulerRunDueJobsNowRequest, SchedulerRunDueJobsNowResponse, SchedulerScheduleJobRequest,
    SchedulerScheduleJobResponse, Status,
};
use kernel_fidl::{ClockGetTimeRequest, ClockPublicClient, ClockType, Status as KernelStatus};
use net_fidl::{
    NetstackGetLinkStatusRequest, NetstackPublicClient, NetstackWatchLinkStatusRequest,
};
use power_fidl::{
    PowerManagerAcquireWakeLeaseRequest, PowerManagerGetPowerSnapshotRequest,
    PowerManagerPublicClient, PowerManagerWatchPowerSnapshotsRequest,
};
use time_fidl::{
    FidlDecode as TimeFidlDecode, SyncState, TimeManagerWatchTimeQualityRequest,
    TimeQualityWatcherOnTimeQualityRequest,
};
use user_manager_fidl::{
    FidlDecode as UserFidlDecode, UserEventKind, UserManagerWatchUserEventsRequest,
    UserStateWatcherOnUserStateChangedRequest,
};

use crate::migration::Runtime;
use crate::service::{JobdStatus, SchedulerClient, empty_job_record};

pub async fn poll(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    changed |= ensure_provider_watchers(runtime);
    changed |= poll_provider_watchers(runtime);
    if let Ok(message) = runtime.control.try_recv() {
        if let (Some(endpoint), Ok(metadata)) = (
            message.handles.first().copied(),
            core::str::from_utf8(&message.bytes),
        ) {
            if let Some(binding) = ServiceBinding::parse(metadata) {
                if binding.protocol_is("Scheduler") {
                    if let (Some(package_id), Some(uid)) =
                        (binding.caller_package.clone(), binding.caller_uid)
                    {
                        runtime.service.clients.push(SchedulerClient {
                            channel: endpoint,
                            package_id,
                            uid,
                            allowed_ordinals: binding.method_ordinals.iter().fold(
                                0,
                                |mask, ordinal| {
                                    if *ordinal < 64 {
                                        mask | (1u64 << *ordinal)
                                    } else {
                                        mask
                                    }
                                },
                            ),
                        });
                    } else {
                        let _ = Memory::close(endpoint);
                    }
                    changed = true;
                }
            }
        }
    }
    changed |= poll_scheduler_clients(runtime).await;
    changed |= poll_job_controls(runtime);
    let now = now_seconds(runtime);
    for expired in runtime.service.expire_timeouts(now) {
        let _ = Memory::close(expired.control);
        if let Some(lease) = runtime.service.finish_batch_job(expired.batch_id) {
            let _ = Memory::close(lease);
        }
        stop_worker(runtime, &expired.package_id, expired.uid, expired.token, -6);
        changed = true;
    }
    if run_due_jobs(runtime) != 0 {
        changed = true;
    }
    changed
}

async fn poll_scheduler_clients(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut clients = core::mem::take(&mut runtime.service.clients);
    clients.retain(|client| match Channel(client.channel).try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if !matches!(ordinal, 1 | 2 | 3 | 4 | 5) || !client_allows(client, ordinal) {
                return true;
            }
            match ordinal {
                1 => handle_schedule(runtime, client, req, &handles),
                2 => handle_cancel(runtime, client, req, &handles),
                3 => handle_get(runtime, client, req, &handles),
                4 => handle_list(runtime, client, req, &handles),
                5 => handle_run_due(runtime, client, req, &handles),
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
    runtime.service.clients = clients;
    changed
}

fn handle_schedule(
    runtime: &mut Runtime,
    client: &SchedulerClient,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = match SchedulerScheduleJobRequest::decode(req, handles) {
        Ok(request) => {
            refresh_declarations(runtime, &client.package_id);
            runtime
                .service
                .schedule(
                    client,
                    from_fidl_spec(&request.spec),
                    clock_snapshot(runtime),
                )
                .map(|_| JobdStatus::Ok)
                .unwrap_or_else(|status| status)
        }
        Err(_) => JobdStatus::InvalidArgs,
    };
    reply(
        Channel(client.channel),
        &SchedulerScheduleJobResponse {
            status: to_fidl_status(status),
        },
    );
}

fn handle_cancel(
    runtime: &mut Runtime,
    client: &SchedulerClient,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = SchedulerCancelJobRequest::decode(req, handles)
        .map(|request| runtime.service.cancel(client, request.job_id))
        .unwrap_or(JobdStatus::InvalidArgs);
    reply(
        Channel(client.channel),
        &SchedulerCancelJobResponse {
            status: to_fidl_status(status),
        },
    );
}

fn handle_get(runtime: &Runtime, client: &SchedulerClient, req: &[u8], handles: &[HandleRef]) {
    let (status, job) = match SchedulerGetJobRequest::decode(req, handles) {
        Ok(request) => match runtime.service.get(client, request.job_id) {
            Ok(job) => (JobdStatus::Ok, job),
            Err(status) => (status, empty_job_record()),
        },
        Err(_) => (JobdStatus::InvalidArgs, empty_job_record()),
    };
    let fidl_job = to_fidl_record(&job);
    reply(
        Channel(client.channel),
        &SchedulerGetJobResponse {
            status: to_fidl_status(status),
            job: fidl_job,
        },
    );
}

fn handle_list(runtime: &Runtime, client: &SchedulerClient, req: &[u8], handles: &[HandleRef]) {
    let _ = SchedulerListJobsRequest::decode(req, handles);
    let records = runtime.service.list(client);
    let fidl_jobs = records.iter().map(to_fidl_record).collect::<Vec<_>>();
    reply(
        Channel(client.channel),
        &SchedulerListJobsResponse {
            status: Status::Ok,
            jobs: job_fidl::WireVector::from_slice(&fidl_jobs),
        },
    );
}

fn handle_run_due(
    runtime: &mut Runtime,
    client: &SchedulerClient,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = if SchedulerRunDueJobsNowRequest::decode(req, handles).is_ok() {
        JobdStatus::Ok
    } else {
        JobdStatus::InvalidArgs
    };
    let launched = if status == JobdStatus::Ok {
        run_due_jobs(runtime)
    } else {
        0
    };
    reply(
        Channel(client.channel),
        &SchedulerRunDueJobsNowResponse {
            status: to_fidl_status(status),
            launched,
        },
    );
}

fn run_due_jobs(runtime: &mut Runtime) -> u32 {
    let now = now_seconds(runtime);
    let conditions = runtime_conditions(runtime);
    runtime.service.refresh_waiting_states(now, conditions);
    let due = runtime.service.due_jobs(now, conditions);
    let mut launched = 0u32;
    let lease = if due.is_empty() {
        0
    } else {
        acquire_wake_lease(runtime).unwrap_or(0)
    };
    let batch_id = if due.is_empty() {
        0
    } else {
        runtime.service.begin_batch(lease, due.len() as u32)
    };
    for record in due {
        let status = launch_job(runtime, &record, now, lease, batch_id);
        if status == JobdStatus::Ok {
            launched = launched.saturating_add(1);
        } else if let Some(lease) = runtime.service.finish_batch_job(batch_id) {
            let _ = Memory::close(lease);
        }
    }
    launched
}

fn ensure_provider_watchers(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    if runtime.power_watcher.is_none() {
        if let Some(power) = runtime.power {
            if let Ok((local, remote)) = Channel::pair() {
                let mut client = PowerManagerPublicClient::new(Rpc(power));
                let mut request_bytes = [0; 64];
                let mut response_bytes = [0; 32];
                let mut request_handles = [power_fidl::HandleRef { raw: 0 }; 1];
                let mut response_handles = [power_fidl::HandleRef { raw: 0 }; 1];
                if client
                    .watch_power_snapshots(
                        &PowerManagerWatchPowerSnapshotsRequest {
                            watcher: power_fidl::HandleRef { raw: remote.0 },
                        },
                        &mut request_bytes,
                        &mut request_handles,
                        &mut response_bytes,
                        &mut response_handles,
                    )
                    .is_ok()
                {
                    runtime.power_watcher = Some(local);
                    changed = true;
                }
            }
        }
    }
    if runtime.link_watcher.is_none() {
        if let Some(netstack) = runtime.netstack {
            if let Ok((local, remote)) = Channel::pair() {
                let mut client = NetstackPublicClient::new(Rpc(netstack));
                let mut request_bytes = [0; 64];
                let mut response_bytes = [0; 32];
                let mut request_handles = [net_fidl::HandleRef { raw: 0 }; 1];
                let mut response_handles = [net_fidl::HandleRef { raw: 0 }; 1];
                if client
                    .watch_link_status(
                        &NetstackWatchLinkStatusRequest {
                            watcher: net_fidl::HandleRef { raw: remote.0 },
                        },
                        &mut request_bytes,
                        &mut request_handles,
                        &mut response_bytes,
                        &mut response_handles,
                    )
                    .is_ok()
                {
                    runtime.link_watcher = Some(local);
                    changed = true;
                }
            }
        }
    }
    if runtime.time_watcher.is_none() {
        if let Some(timed) = runtime.timed {
            if let Ok((local, remote)) = Channel::pair() {
                let mut client = time_fidl::TimeManagerPublicClient::new(Rpc(timed));
                let mut request_bytes = [0; 64];
                let mut response_bytes = [0; 32];
                let mut request_handles = [time_fidl::HandleRef { raw: 0 }; 1];
                let mut response_handles = [time_fidl::HandleRef { raw: 0 }; 1];
                if client
                    .watch_time_quality(
                        &TimeManagerWatchTimeQualityRequest {
                            watcher: time_fidl::HandleRef { raw: remote.0 },
                        },
                        &mut request_bytes,
                        &mut request_handles,
                        &mut response_bytes,
                        &mut response_handles,
                    )
                    .is_ok()
                {
                    runtime.time_watcher = Some(local);
                    changed = true;
                }
            }
        }
    }
    if runtime.user_watcher.is_none() {
        if let Some(usersd) = runtime.usersd {
            if let Ok((local, remote)) = Channel::pair() {
                let mut client = user_manager_fidl::UserManagerPublicClient::new(Rpc(usersd));
                let mut request_bytes = [0; 64];
                let mut response_bytes = [0; 32];
                let mut request_handles = [user_manager_fidl::HandleRef { raw: 0 }; 1];
                let mut response_handles = [user_manager_fidl::HandleRef { raw: 0 }; 1];
                if client
                    .watch_user_events(
                        &UserManagerWatchUserEventsRequest {
                            watcher: user_manager_fidl::HandleRef { raw: remote.0 },
                        },
                        &mut request_bytes,
                        &mut request_handles,
                        &mut response_bytes,
                        &mut response_handles,
                    )
                    .is_ok()
                {
                    runtime.user_watcher = Some(local);
                    changed = true;
                }
            }
        }
    }
    if runtime.package_watcher.is_none() {
        if let Some(worker_launcher) = runtime.worker_launcher {
            if let Ok((local, remote)) = Channel::pair() {
                let mut client =
                    app_worker_fidl::WorkerLauncherPublicClient::new(Rpc(worker_launcher));
                let mut request_bytes = [0; 64];
                let mut response_bytes = [0; 32];
                let mut request_handles = [WorkerHandleRef { raw: 0 }; 1];
                let mut response_handles = [WorkerHandleRef { raw: 0 }; 1];
                if matches!(
                    client.watch_package_policy(
                        &WorkerLauncherWatchPackagePolicyRequest {
                            watcher: WorkerHandleRef { raw: remote.0 },
                        },
                        &mut request_bytes,
                        &mut request_handles,
                        &mut response_bytes,
                        &mut response_handles,
                    ),
                    Ok(WorkerLauncherWatchPackagePolicyResponse {
                        status: WorkerLaunchStatus::Ok
                    })
                ) {
                    runtime.package_watcher = Some(local);
                    changed = true;
                }
            }
        }
    }
    changed
}

fn poll_provider_watchers(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    if let Some(channel) = runtime.power_watcher {
        if channel.try_recv().is_ok() {
            changed = true;
        }
    }
    if let Some(channel) = runtime.link_watcher {
        if channel.try_recv().is_ok() {
            changed = true;
        }
    }
    if let Some(channel) = runtime.time_watcher {
        if let Ok(message) = channel.try_recv() {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = time_refs(&message.handles);
            if ordinal == 1 {
                if let Ok(event) = TimeQualityWatcherOnTimeQualityRequest::decode(req, &handles) {
                    if matches!(event.quality.state, SyncState::Synced | SyncState::Manual) {
                        if let Some(realtime) = clock_snapshot(runtime).realtime_seconds {
                            runtime.service.anchor_realtime_jobs(realtime);
                        }
                    }
                }
            }
            changed = true;
        }
    }
    if let Some(channel) = runtime.user_watcher {
        if let Ok(message) = channel.try_recv() {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = user_refs(&message.handles);
            if ordinal == 1 {
                if let Ok(event) = UserStateWatcherOnUserStateChangedRequest::decode(req, &handles)
                {
                    match event.kind {
                        UserEventKind::Locked => runtime.service.mark_user_locked(event.uid, true),
                        UserEventKind::Unlocked => {
                            crate::runtime::load_persistent_user_jobs(runtime, event.uid);
                            runtime.service.mark_user_locked(event.uid, false)
                        }
                        UserEventKind::Deleted => runtime.service.delete_user(event.uid),
                        _ => {}
                    }
                }
            }
            changed = true;
        }
    }
    if let Some(channel) = runtime.package_watcher {
        if let Ok(message) = channel.try_recv() {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = worker_refs(&message.handles);
            if ordinal == 1 {
                if let Ok(event) =
                    PackagePolicyWatcherOnPackagePolicyChangedRequest::decode(req, &handles)
                {
                    match event.kind {
                        PackagePolicyEventKind::Uninstalled => {
                            runtime.service.package_uninstalled(event.package_id)
                        }
                        PackagePolicyEventKind::Updated | PackagePolicyEventKind::Installed => {
                            runtime
                                .service
                                .package_updated(event.package_id, event.package_instance_id)
                        }
                    }
                }
            }
            changed = true;
        }
    }
    changed
}

fn launch_job(
    runtime: &mut Runtime,
    record: &JobRecord,
    now: u64,
    lease: u64,
    batch_id: u64,
) -> JobdStatus {
    let (control_server, control_client) = match Channel::pair() {
        Ok(pair) => pair,
        Err(_) => return JobdStatus::LaunchFailed,
    };
    let token = match runtime
        .service
        .begin_running(record, now, control_server.0, lease, batch_id)
    {
        Ok(token) => token,
        Err(status) => return status,
    };
    let Some(worker_launcher) = runtime.worker_launcher else {
        return JobdStatus::LaunchFailed;
    };
    let mut client = app_worker_fidl::WorkerLauncherPublicClient::new(Rpc(worker_launcher));
    let mut request_bytes = [0; 512];
    let mut response_bytes = [0; 64];
    let mut request_handles = [WorkerHandleRef { raw: 0 }; 4];
    let mut response_handles = [WorkerHandleRef { raw: 0 }; 4];
    let response = client.spawn_worker(
        &WorkerLauncherSpawnWorkerRequest {
            package_id: &record.package_id,
            process_name: &record.spec.target_component,
            uid: record.uid,
            job_token: token,
            job_control: WorkerHandleRef {
                raw: control_client.0,
            },
        },
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    );
    match response {
        Ok(WorkerLauncherSpawnWorkerResponse {
            status: WorkerLaunchStatus::Ok,
        }) => JobdStatus::Ok,
        _ => {
            let _ = runtime
                .service
                .complete(token, false, record.spec.interval_seconds != 0, now);
            stop_worker(runtime, &record.package_id, record.uid, token, -22);
            JobdStatus::LaunchFailed
        }
    }
}

fn poll_job_controls(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let running = runtime.service.running.clone();
    for job in running {
        if let Ok(message) = Channel(job.control).try_recv() {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if ordinal == 1 {
                let status = match JobControlCompleteRequest::decode(req, &handles) {
                    Ok(request) => runtime.service.complete(
                        request.job_token,
                        request.success,
                        request.reschedule,
                        now_seconds(runtime),
                    ),
                    Err(_) => JobdStatus::InvalidArgs,
                };
                reply(
                    Channel(job.control),
                    &JobControlCompleteResponse {
                        status: to_fidl_status(status),
                    },
                );
                if let Some(lease) = runtime.service.finish_batch_job(job.batch_id) {
                    let _ = Memory::close(lease);
                }
                stop_worker(runtime, &job.package_id, job.uid, job.token, 0);
                changed = true;
            }
        }
    }
    changed
}

fn refresh_declarations(runtime: &mut Runtime, package_id: &str) -> JobdStatus {
    let Some(worker_launcher) = runtime.worker_launcher else {
        return JobdStatus::AccessDenied;
    };
    let mut client = app_worker_fidl::WorkerLauncherPublicClient::new(Rpc(worker_launcher));
    let mut request_bytes = [0; 256];
    let mut response_bytes = [0; 4096];
    let mut request_handles = [WorkerHandleRef { raw: 0 }; 1];
    let mut response_handles = [WorkerHandleRef { raw: 0 }; 1];
    match client.get_job_declarations(
        &WorkerLauncherGetJobDeclarationsRequest { package_id },
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    ) {
        Ok(WorkerLauncherGetJobDeclarationsResponse {
            status: WorkerLaunchStatus::Ok,
            package_instance_id,
            declarations,
        }) => {
            let mut specs = Vec::new();
            for index in 0..declarations.len() {
                let Ok(declaration) = declarations.get(index) else {
                    return JobdStatus::AccessDenied;
                };
                specs.push(from_worker_declaration(declaration));
            }
            runtime
                .service
                .register_declarations(package_id, package_instance_id, specs);
            JobdStatus::Ok
        }
        Ok(WorkerLauncherGetJobDeclarationsResponse {
            status: WorkerLaunchStatus::NotFound,
            ..
        }) => JobdStatus::NotFound,
        _ => JobdStatus::AccessDenied,
    }
}

fn stop_worker(
    runtime: &Runtime,
    package_id: &str,
    uid: u64,
    job_token: u64,
    exit_code: i32,
) -> JobdStatus {
    let Some(worker_launcher) = runtime.worker_launcher else {
        return JobdStatus::LaunchFailed;
    };
    let mut client = app_worker_fidl::WorkerLauncherPublicClient::new(Rpc(worker_launcher));
    let mut request_bytes = [0; 256];
    let mut response_bytes = [0; 64];
    let mut request_handles = [WorkerHandleRef { raw: 0 }; 1];
    let mut response_handles = [WorkerHandleRef { raw: 0 }; 1];
    match client.stop_worker(
        &WorkerLauncherStopWorkerRequest {
            package_id,
            uid,
            job_token,
            exit_code,
        },
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    ) {
        Ok(WorkerLauncherStopWorkerResponse {
            status: WorkerLaunchStatus::Ok | WorkerLaunchStatus::NotFound,
        }) => JobdStatus::Ok,
        _ => JobdStatus::LaunchFailed,
    }
}

fn runtime_conditions(runtime: &Runtime) -> RuntimeConditions {
    let mut conditions = RuntimeConditions {
        available: false,
        ..RuntimeConditions::default()
    };
    let mut power_available = false;
    let mut net_available = false;
    if let Some(power) = runtime.power {
        let mut client = PowerManagerPublicClient::new(Rpc(power));
        let mut request_bytes = [0; 16];
        let mut response_bytes = [0; 128];
        let mut request_handles = [power_fidl::HandleRef { raw: 0 }; 1];
        let mut response_handles = [power_fidl::HandleRef { raw: 0 }; 1];
        if let Ok(response) = client.get_power_snapshot(
            &PowerManagerGetPowerSnapshotRequest {},
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        ) {
            conditions.charging = response.snapshot.charging;
            conditions.device_idle = response.snapshot.device_idle;
            conditions.battery_low = response.snapshot.battery_low;
            conditions.thermal_throttled = response.snapshot.thermal_throttled;
            power_available = response.status == power_fidl::Status::Ok;
        }
    }
    if let Some(netstack) = runtime.netstack {
        let mut client = NetstackPublicClient::new(Rpc(netstack));
        let mut request_bytes = [0; 16];
        let mut response_bytes = [0; 64];
        let mut request_handles = [net_fidl::HandleRef { raw: 0 }; 1];
        let mut response_handles = [net_fidl::HandleRef { raw: 0 }; 1];
        if let Ok(response) = client.get_link_status(
            &NetstackGetLinkStatusRequest {},
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        ) {
            conditions.network_available = response.link.available;
            conditions.network_unmetered = !response.link.metered;
            net_available = response.status == net_fidl::Status::Ok;
        }
    }
    conditions.available = power_available && net_available;
    conditions
}

fn acquire_wake_lease(runtime: &Runtime) -> Option<u64> {
    let power = runtime.power?;
    let mut client = PowerManagerPublicClient::new(Rpc(power));
    let mut request_bytes = [0; 128];
    let mut response_bytes = [0; 64];
    let mut request_handles = [power_fidl::HandleRef { raw: 0 }; 1];
    let mut response_handles = [power_fidl::HandleRef { raw: 0 }; 1];
    client
        .acquire_wake_lease(
            &PowerManagerAcquireWakeLeaseRequest { reason: "jobd" },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )
        .ok()
        .map(|response| response.lease_handle.raw)
}

fn now_seconds(runtime: &Runtime) -> u64 {
    clock_seconds(runtime, ClockType::BootTime).unwrap_or_else(|| {
        bexos_userspace::syscall::ticks() / bexos_userspace::syscall::frequency().max(1)
    })
}

fn clock_snapshot(runtime: &Runtime) -> ClockSnapshot {
    ClockSnapshot {
        monotonic_seconds: now_seconds(runtime),
        realtime_seconds: clock_seconds(runtime, ClockType::Realtime),
    }
}

fn clock_seconds(runtime: &Runtime, clock_type: ClockType) -> Option<u64> {
    if runtime.clock {
        let mut client = ClockPublicClient::new(KernelTransport(8));
        let mut request_bytes = [0; 16];
        let mut response_bytes = [0; 32];
        let mut request_handles = [kernel_fidl::HandleRef { raw: 0 }; 1];
        let mut response_handles = [kernel_fidl::HandleRef { raw: 0 }; 1];
        if let Ok(response) = client.get_time(
            &ClockGetTimeRequest { clock_type },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        ) {
            return (response.status == KernelStatus::Ok).then_some(response.nanos / 1_000_000_000);
        }
    }
    None
}

fn from_fidl_spec(spec: &job_fidl::JobSpec) -> JobSpec {
    JobSpec {
        job_id: spec.job_id.to_string(),
        target_component: spec.target_component.to_string(),
        initial_delay_seconds: spec.initial_delay_seconds,
        interval_seconds: spec.interval_seconds,
        flex_window_seconds: spec.flex_window_seconds,
        constraints: JobConstraints {
            network: match spec.constraints.network {
                Some(job_fidl::NetworkRequirement::None) => NetworkRequirement::None,
                Some(job_fidl::NetworkRequirement::Unmetered) => NetworkRequirement::Unmetered,
                _ => NetworkRequirement::Any,
            },
            require_charging: spec.constraints.require_charging.unwrap_or(false),
            require_device_idle: spec.constraints.require_device_idle.unwrap_or(false),
            require_battery_not_low: spec.constraints.require_battery_not_low.unwrap_or(false),
        },
        max_execution_seconds: spec.max_execution_seconds,
        persist_across_reboots: spec.persist_across_reboots,
    }
}

fn from_worker_declaration(job: app_worker_fidl::JobDeclaration<'_>) -> JobSpec {
    JobSpec {
        job_id: job.job_id.to_string(),
        target_component: job.target_component.to_string(),
        initial_delay_seconds: job.initial_delay_seconds,
        interval_seconds: job.interval_seconds,
        flex_window_seconds: job.flex_window_seconds,
        constraints: JobConstraints {
            network: match job.network {
                app_worker_fidl::JobNetworkConstraint::None => NetworkRequirement::None,
                app_worker_fidl::JobNetworkConstraint::UnmeteredOnly => {
                    NetworkRequirement::Unmetered
                }
                app_worker_fidl::JobNetworkConstraint::Any => NetworkRequirement::Any,
            },
            require_charging: job.requires_charging,
            require_device_idle: job.requires_device_idle,
            require_battery_not_low: job.requires_battery_not_low,
        },
        max_execution_seconds: job.max_execution_seconds,
        persist_across_reboots: job.persist_across_reboots,
    }
}

fn to_fidl_record(record: &JobRecord) -> job_fidl::JobRecord<'_> {
    job_fidl::JobRecord {
        package_id: &record.package_id,
        uid: record.uid,
        spec: to_fidl_spec(&record.spec),
        state: match record.state {
            bexos_job_store::JobState::Scheduled => job_fidl::JobState::Scheduled,
            bexos_job_store::JobState::Running => job_fidl::JobState::Running,
            bexos_job_store::JobState::WaitingConstraints => job_fidl::JobState::WaitingConstraints,
            bexos_job_store::JobState::LockedUser => job_fidl::JobState::LockedUser,
            bexos_job_store::JobState::Completed => job_fidl::JobState::Completed,
            bexos_job_store::JobState::Failed => job_fidl::JobState::Failed,
            bexos_job_store::JobState::Cancelled => job_fidl::JobState::Cancelled,
        },
        last_run_status: match record.last_run_status {
            bexos_job_store::JobRunStatus::NeverRun => job_fidl::JobRunStatus::NeverRun,
            bexos_job_store::JobRunStatus::Ok => job_fidl::JobRunStatus::Ok,
            bexos_job_store::JobRunStatus::Failed => job_fidl::JobRunStatus::Failed,
            bexos_job_store::JobRunStatus::TimedOut => job_fidl::JobRunStatus::TimedOut,
            bexos_job_store::JobRunStatus::Cancelled => job_fidl::JobRunStatus::Cancelled,
        },
        package_instance_id: record.package_instance_id,
        timebase: match record.timebase {
            JobTimebase::Monotonic => job_fidl::JobTimebase::Monotonic,
            JobTimebase::RealtimeUtc => job_fidl::JobTimebase::RealtimeUtc,
            JobTimebase::WaitingRealtimeAnchor => job_fidl::JobTimebase::WaitingRealtimeAnchor,
        },
        next_run_seconds: record.next_run_seconds,
        last_run_seconds: record.last_run_seconds,
        job_token: record.job_token,
        run_attempts: record.run_attempts,
        anchor_delay_seconds: record.anchor_delay_seconds,
    }
}

fn to_fidl_spec(spec: &JobSpec) -> job_fidl::JobSpec<'_> {
    job_fidl::JobSpec {
        job_id: &spec.job_id,
        target_component: &spec.target_component,
        initial_delay_seconds: spec.initial_delay_seconds,
        interval_seconds: spec.interval_seconds,
        flex_window_seconds: spec.flex_window_seconds,
        constraints: job_fidl::JobConstraints {
            network: match spec.constraints.network {
                NetworkRequirement::None => Some(job_fidl::NetworkRequirement::None),
                NetworkRequirement::Unmetered => Some(job_fidl::NetworkRequirement::Unmetered),
                NetworkRequirement::Any => Some(job_fidl::NetworkRequirement::Any),
            },
            require_charging: Some(spec.constraints.require_charging),
            require_device_idle: Some(spec.constraints.require_device_idle),
            require_battery_not_low: Some(spec.constraints.require_battery_not_low),
        },
        max_execution_seconds: spec.max_execution_seconds,
        persist_across_reboots: spec.persist_across_reboots,
    }
}

fn to_fidl_status(status: JobdStatus) -> Status {
    match status {
        JobdStatus::Ok => Status::Ok,
        JobdStatus::NotFound => Status::NotFound,
        JobdStatus::AccessDenied => Status::AccessDenied,
        JobdStatus::InvalidArgs => Status::InvalidArgs,
        JobdStatus::Storage => Status::Storage,
        JobdStatus::ConstraintsUnmet => Status::ConstraintsUnmet,
        JobdStatus::LaunchFailed => Status::LaunchFailed,
    }
}

fn client_allows(client: &SchedulerClient, ordinal: u64) -> bool {
    ordinal < 64 && (client.allowed_ordinals & (1u64 << ordinal)) != 0
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut bytes = [0; 4096];
    let mut handles = [HandleRef { raw: 0 }; 16];
    if let Ok(encoded) = response.encode(&mut bytes, &mut handles) {
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = channel.send(&bytes[..encoded.bytes], &raw_handles);
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    (
        u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn handle_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

fn time_refs(handles: &[u64]) -> Vec<time_fidl::HandleRef> {
    handles
        .iter()
        .map(|raw| time_fidl::HandleRef { raw: *raw })
        .collect()
}

fn user_refs(handles: &[u64]) -> Vec<user_manager_fidl::HandleRef> {
    handles
        .iter()
        .map(|raw| user_manager_fidl::HandleRef { raw: *raw })
        .collect()
}

fn worker_refs(handles: &[u64]) -> Vec<WorkerHandleRef> {
    handles
        .iter()
        .map(|raw| WorkerHandleRef { raw: *raw })
        .collect()
}
