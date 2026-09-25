use alloc::vec;
use alloc::vec::Vec;

use bexos_userspace::{
    Channel, Memory, Startup,
    live_migration::{RecordChanges, Source},
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use net_fidl::{
    FidlDecode, FidlEncode, HandleRef, Status, VirtualSwitchControllerCommitGenerationRequest,
    VirtualSwitchControllerCommitGenerationResponse, VirtualSwitchControllerCreatePortRequest,
    VirtualSwitchControllerCreatePortResponse, VirtualSwitchControllerRecoverGenerationRequest,
    VirtualSwitchControllerRecoverGenerationResponse, VirtualSwitchControllerRemovePortRequest,
    VirtualSwitchControllerRemovePortResponse,
};

use crate::device::{VirtualDevice, poll_devices};
use crate::migration::Runtime;
use crate::packet::{Packet, PacketDisposition};
use crate::physical::PhysicalDevice;
use crate::routing::InterfaceEndpoint;
use crate::switch::{PortPolicy, SwitchError};
use bexos_network_extension_abi::Direction;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("vswitchd startup");
    let mut runtime = if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(runtime) => runtime,
            Err(_) => bexos_userspace::exit(),
        }
    } else {
        let mut runtime = Runtime::new(control, startup.migration);
        for grant in &startup.service_grants {
            if grant.service == "bexos.hardware.ethernet.Device" {
                let id = grant
                    .provider_instance_id
                    .as_deref()
                    .and_then(bexos_network_policy::selector_id)
                    .unwrap_or(grant.endpoint);
                match PhysicalDevice::connect(grant.endpoint) {
                    Ok(device) => {
                        runtime.physical_devices.insert(id, device);
                    }
                    Err(_) => {
                        let _ = Memory::close(grant.endpoint);
                    }
                }
            }
        }
        Startup::ready(control).expect("vswitchd ready");
        runtime
    };
    let mut source = Source::new(runtime.migration);
    let mut changes = RecordChanges::default();
    loop {
        changes.poll(&runtime, &mut source);
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        accept(&mut runtime);
        let mut changed = poll(&mut runtime);
        changed |= poll_devices(&mut runtime.virtual_devices, &mut runtime.switch);
        let now_ns = monotonic_ns();
        let mut routed_output = Vec::new();
        for (interface, device) in &mut runtime.physical_devices {
            changed |= device.poll(
                *interface,
                &mut runtime.switch,
                &mut runtime.data_plane,
                &mut routed_output,
                now_ns,
            );
        }
        for (interface, bytes) in routed_output {
            if let Some(device) = runtime.physical_devices.get_mut(&interface) {
                changed |= device.enqueue_routed(bytes).is_ok();
            }
        }
        if changed {
            if runtime.refresh_migration_image().is_err() {
                let _ = bexos_userspace::migration::abort();
            }
            source.changed_keys([0, 1, 2].into_iter().chain(Runtime::data_plane_keys()));
        }
        bexos_userspace::yield_now();
    }
}

fn accept(runtime: &mut Runtime) {
    if let Ok(message) = runtime.control.try_recv() {
        let Some(endpoint) = message.handles.first().copied() else {
            return;
        };
        let binding = core::str::from_utf8(&message.bytes)
            .ok()
            .and_then(ServiceBinding::parse);
        if let Some(binding) = binding.filter(|binding| {
            matches!(
                binding.protocol.as_str(),
                "VirtualSwitchController" | "SwitchRoutingController" | "SwitchExtensionController"
            ) && binding.caller_package.as_deref() == Some("bexos.service.networkd")
        }) {
            runtime
                .clients
                .push(BoundServiceEndpoint::new_with_protocol(
                    Channel(endpoint),
                    binding.method_ordinals,
                    &binding.protocol,
                ));
        } else {
            let _ = Memory::close(endpoint);
        }
    }
}

fn poll(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut clients = core::mem::take(&mut runtime.clients);
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, request) = envelope(&message.bytes);
            let handles = message
                .handles
                .iter()
                .map(|raw| HandleRef { raw: *raw })
                .collect::<Vec<_>>();
            if !client.allows(ordinal) {
                close_handles(&message.handles);
                return true;
            }
            if client.protocol == "SwitchRoutingController" {
                crate::routing_control::handle(runtime, client.channel, ordinal, request, &handles);
                close_handles(&message.handles);
                return true;
            }
            if client.protocol == "SwitchExtensionController" {
                crate::extension_control::handle(
                    runtime,
                    client.channel,
                    ordinal,
                    request,
                    &handles,
                );
                if ordinal != 1 {
                    close_handles(&message.handles);
                }
                return true;
            }
            match ordinal {
                1 => {
                    let response =
                        match VirtualSwitchControllerCreatePortRequest::decode(request, &handles) {
                            Ok(request) => create_port(runtime, request),
                            Err(_) => VirtualSwitchControllerCreatePortResponse {
                                status: Status::ErrInvalidArgs,
                                device: HandleRef { raw: 0 },
                            },
                        };
                    reply(client.channel, &response);
                }
                2 => {
                    let status =
                        VirtualSwitchControllerRemovePortRequest::decode(request, &handles)
                            .map(|request| {
                                if let Some(mut device) =
                                    runtime.virtual_devices.remove(&request.port_id)
                                {
                                    device.close();
                                }
                                runtime
                                    .switch
                                    .remove_port(request.port_id)
                                    .map(|_| Status::Ok)
                                    .unwrap_or_else(status)
                            })
                            .unwrap_or(Status::ErrInvalidArgs);
                    reply(
                        client.channel,
                        &VirtualSwitchControllerRemovePortResponse { status },
                    );
                }
                3 => {
                    let status =
                        VirtualSwitchControllerCommitGenerationRequest::decode(request, &handles)
                            .map(|request| {
                                let physical = runtime
                                    .switch
                                    .port(request.port_id)
                                    .map(|port| port.policy.physical_interface);
                                let count = runtime
                                    .switch
                                    .port(request.port_id)
                                    .map(|port| {
                                        port.staged_tx
                                            .iter()
                                            .take_while(|frame| {
                                                frame.generation <= request.generation
                                            })
                                            .count()
                                    })
                                    .unwrap_or(0);
                                if !physical
                                    .and_then(|interface| runtime.physical_devices.get(&interface))
                                    .is_some_and(|device| device.can_enqueue(count))
                                {
                                    return Status::ErrShouldWait;
                                }
                                runtime
                                    .switch
                                    .commit_generation(request.port_id, request.generation)
                                    .and_then(|frames| {
                                        commit_frames(runtime, request.port_id, frames)
                                    })
                                    .map(|_| Status::Ok)
                                    .unwrap_or_else(status)
                            })
                            .unwrap_or(Status::ErrInvalidArgs);
                    reply(
                        client.channel,
                        &VirtualSwitchControllerCommitGenerationResponse { status },
                    );
                }
                4 => {
                    let status =
                        VirtualSwitchControllerRecoverGenerationRequest::decode(request, &handles)
                            .map(|request| {
                                runtime
                                    .switch
                                    .recover_port(request.port_id, request.generation)
                                    .map(|_| Status::Ok)
                                    .unwrap_or_else(status)
                            })
                            .unwrap_or(Status::ErrInvalidArgs);
                    reply(
                        client.channel,
                        &VirtualSwitchControllerRecoverGenerationResponse { status },
                    );
                }
                _ => close_handles(&message.handles),
            }
            true
        }
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            let _ = Memory::close(client.channel.0);
            changed = true;
            false
        }
        Err(_) => true,
    });
    runtime.clients = clients;
    changed
}

fn commit_frames(
    runtime: &mut Runtime,
    port_id: u64,
    frames: Vec<crate::switch::QueuedFrame>,
) -> Result<(), SwitchError> {
    let policy = runtime
        .switch
        .port(port_id)
        .ok_or(SwitchError::NotFound)?
        .policy;
    let routed = runtime
        .data_plane
        .routing
        .interface_for_endpoint(InterfaceEndpoint::Virtual(port_id))
        .cloned();
    if let Some(interface) = routed {
        let mut packets = frames
            .into_iter()
            .map(|frame| {
                Packet::parse(
                    &frame.bytes,
                    interface.id,
                    Direction::VirtualIngress,
                    interface.table_id,
                    interface.zone,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let additional = runtime.data_plane.process(&mut packets, monotonic_ns());
        packets.extend(additional);
        for packet in packets {
            match packet.disposition {
                PacketDisposition::Deliver(target) => {
                    runtime.switch.ingress_to_port(target, &packet.bytes)?;
                }
                PacketDisposition::Transmit(target) => {
                    runtime
                        .physical_devices
                        .get_mut(&target)
                        .ok_or(SwitchError::NotFound)?
                        .enqueue_routed(packet.bytes)
                        .map_err(|_| SwitchError::QueueFull)?;
                }
                PacketDisposition::Drop => {}
                PacketDisposition::Continue => return Err(SwitchError::InvalidRoute),
            }
        }
        Ok(())
    } else {
        runtime
            .physical_devices
            .get_mut(&policy.physical_interface)
            .ok_or(SwitchError::NotFound)?
            .enqueue(frames)
            .map_err(|_| SwitchError::QueueFull)
    }
}

fn monotonic_ns() -> u64 {
    let frequency = bexos_userspace::syscall::frequency().max(1);
    ((u128::from(bexos_userspace::syscall::ticks()) * 1_000_000_000) / u128::from(frequency)) as u64
}

fn create_port(
    runtime: &mut Runtime,
    mut request: VirtualSwitchControllerCreatePortRequest,
) -> VirtualSwitchControllerCreatePortResponse {
    if request.physical_interface == 0 {
        let Some(interface) = runtime.physical_devices.keys().next().copied() else {
            return VirtualSwitchControllerCreatePortResponse {
                status: Status::ErrNotFound,
                device: HandleRef { raw: 0 },
            };
        };
        request.physical_interface = interface;
    }
    let policy = PortPolicy {
        port_id: request.port_id,
        physical_interface: request.physical_interface,
        source_mac: request.mac,
        vlan_id: (request.vlan_id != 0).then_some(request.vlan_id),
        tagged: request.tagged,
        rx_capacity: request.rx_queue_depth as usize,
        tx_capacity: request.tx_queue_depth as usize,
    };
    if !runtime
        .physical_devices
        .contains_key(&request.physical_interface)
    {
        return VirtualSwitchControllerCreatePortResponse {
            status: Status::ErrNotFound,
            device: HandleRef { raw: 0 },
        };
    }
    let status = match runtime.switch.create_port(policy) {
        Ok(()) => Status::Ok,
        Err(error) => status(error),
    };
    if status != Status::Ok {
        return VirtualSwitchControllerCreatePortResponse {
            status,
            device: HandleRef { raw: 0 },
        };
    }
    let Ok((client, server)) = Channel::pair() else {
        let _ = runtime.switch.remove_port(request.port_id);
        return VirtualSwitchControllerCreatePortResponse {
            status: Status::ErrNoMemory,
            device: HandleRef { raw: 0 },
        };
    };
    runtime
        .virtual_devices
        .insert(request.port_id, VirtualDevice::new(server));
    VirtualSwitchControllerCreatePortResponse {
        status: Status::Ok,
        device: HandleRef { raw: client.0 },
    }
}

fn status(error: SwitchError) -> Status {
    match error {
        SwitchError::AlreadyExists => Status::ErrAlreadyExists,
        SwitchError::NotFound => Status::ErrNotFound,
        SwitchError::QueueFull => Status::ErrShouldWait,
        SwitchError::Spoofed => Status::ErrAccessDenied,
        SwitchError::StaleGeneration => Status::ErrInvalidArgs,
        _ => Status::ErrInvalidArgs,
    }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut bytes = vec![0; 512];
    let mut handles = [HandleRef { raw: 0 }; 2];
    if let Ok(encoded) = response.encode(&mut bytes, &mut handles) {
        let hs = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = channel.send(&bytes[..encoded.bytes], &hs);
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn close_handles(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}
