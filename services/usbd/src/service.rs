use crate::{Runtime, registry, topology::Topology, wire};
use alloc::vec::Vec;
use bexos_userspace::{
    Channel, Memory, ServiceGrant, Startup,
    live_migration::Source,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use usb_host_fidl::*;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap_or_else(|_| bexos_userspace::exit());
    let mut state = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, start.migration_generation)
            .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let mut state = Runtime::new(control, start.migration);
        attach_startup_grants(&mut state, &start.service_grants);
        Startup::ready(control).unwrap();
        bexos_userspace::log("usbd: ready\n");
        state
    };
    serve(&mut state).await
}

async fn serve(state: &mut Runtime) -> ! {
    let control = state.control;
    let mut source = Source::new(state.migration);
    loop {
        if source.poll(state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace_async::yield_once().await;
            continue;
        }
        poll_control(control, state, &mut source);
        poll_clients(state, &mut source);
        bexos_userspace_async::yield_once().await;
    }
}

fn attach_startup_grants(state: &mut Runtime, grants: &[ServiceGrant]) {
    for grant in grants {
        match grant.protocol.as_str() {
            "XhciController" => state.controller = Some(Channel(grant.endpoint)),
            "DeviceRegistry" => state.registry = Some(Channel(grant.endpoint)),
            _ => {}
        }
    }
    if let Some(controller) = state.controller {
        if let Ok((events_local, events_remote)) = Channel::pair() {
            let request = XhciControllerRegisterBusManagerRequest {
                events: HandleRef {
                    raw: events_remote.0,
                },
            };
            let mut out = [0; 128];
            let mut handles = [HandleRef { raw: 0 }; 2];
            out[..8].copy_from_slice(&2u64.to_le_bytes());
            if let Ok(encoded) = request.encode(&mut out[8..], &mut handles) {
                let _ = controller.send(&out[..8 + encoded.bytes], &[events_remote.0]);
                if let Ok(reply) = controller.try_recv() {
                    let refs = wire::refs(&reply.handles);
                    let mut adopted_bus = 0;
                    if let Ok(response) =
                        XhciControllerRegisterBusManagerResponse::decode(&reply.bytes, &refs)
                    {
                        if response.status == Status::Ok {
                            state.controller_events = Some(events_local);
                            adopted_bus = response.bus.raw;
                            state.controller_bus = Some(Channel(adopted_bus));
                        }
                    }
                    for handle in reply.handles {
                        if handle != adopted_bus {
                            let _ = Memory::close(handle);
                        }
                    }
                }
            }
        }
    }
}

fn poll_control(control: Channel, state: &mut Runtime, source: &mut Source) {
    let Ok(message) = control.try_recv() else {
        return;
    };
    if let (Some(endpoint), Ok(metadata)) = (
        message.handles.first().copied(),
        core::str::from_utf8(&message.bytes),
    ) {
        if let Some(binding) = ServiceBinding::parse(metadata) {
            if binding.protocol_is("UsbBus") {
                state.clients.push(BoundServiceEndpoint::new(
                    Channel(endpoint),
                    binding.method_ordinals,
                ));
                source.changed(2);
                return;
            }
        }
        let _ = Memory::close(endpoint);
        return;
    }
    wire::close(&message.handles);
}

fn poll_clients(state: &mut Runtime, source: &mut Source) {
    let mut index = 0;
    while index < state.clients.len() {
        let keep = match state.clients[index].channel.try_recv() {
            Ok(message) => {
                let keep = handle_client(index, state, message.bytes, message.handles);
                source.changed_keys([1, 2, 3]);
                keep
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => false,
            Err(_) => true,
        };
        if keep {
            index += 1;
        } else {
            let channel = state.clients.remove(index).channel;
            let _ = Memory::close(channel.0);
            source.changed(2);
        }
    }
}

fn handle_client(
    client_index: usize,
    state: &mut Runtime,
    bytes: Vec<u8>,
    handles: Vec<u64>,
) -> bool {
    let channel = state.clients[client_index].channel;
    let Some((ordinal, request)) = wire::envelope(&bytes) else {
        wire::close(&handles);
        return true;
    };
    if !state.clients[client_index].allows(ordinal) {
        wire::close(&handles);
        return true;
    }
    let refs = wire::refs(&handles);
    match ordinal {
        1 => {
            let status = UsbBusSubscribeTopologyRequest::decode(request, &refs)
                .ok()
                .filter(|_| handles.len() == 1)
                .map_or(Status::ErrInvalidArgs, |_| {
                    state.topology_watchers.push(Channel(handles[0]));
                    Status::Ok
                });
            wire::reply(
                channel,
                &UsbBusSubscribeTopologyResponse {
                    status,
                    generation: state.topology.generation,
                },
            );
        }
        2 if handles.is_empty() => {
            let mut info = InterfaceInfo {
                device_id: 0,
                interface_id: 0,
                generation: 0,
                class_code: 0,
                subclass: 0,
                protocol: 0,
                speed: UsbSpeed::Full,
                endpoints: WireVector::from_slice(&[]),
            };
            let (status, handle) = UsbBusClaimInterfaceRequest::decode(request, &[])
                .ok()
                .map_or((Status::ErrInvalidArgs, 0), |q| {
                    claim_interface(
                        &mut state.topology,
                        state.registry,
                        q.device_id,
                        q.interface_number,
                        channel.0,
                        &mut info,
                    )
                });
            wire::reply(
                channel,
                &UsbBusClaimInterfaceResponse {
                    status,
                    info,
                    interface_channel: HandleRef { raw: handle },
                },
            );
        }
        3 if handles.is_empty() => {
            let status = UsbBusReleaseInterfaceRequest::decode(request, &[])
                .ok()
                .map_or(Status::ErrInvalidArgs, |q| {
                    release_interface(&mut state.topology, state.registry, q.interface_id)
                });
            wire::reply(channel, &UsbBusReleaseInterfaceResponse { status });
        }
        _ => wire::close(&handles),
    }
    true
}

fn claim_interface(
    topology: &mut Topology,
    registry: Option<Channel>,
    device_id: u64,
    interface_number: u8,
    owner: u64,
    info: &mut InterfaceInfo<'static>,
) -> (Status, u64) {
    let Some(interface) = topology.interface_mut(device_id, interface_number) else {
        return (Status::ErrNotFound, 0);
    };
    if interface.owner.is_some() {
        return (Status::ErrAlreadyExists, 0);
    }
    let Ok((local, remote)) = Channel::pair() else {
        return (Status::ErrNoMemory, 0);
    };
    let registry_status = registry.map_or(Status::Ok, |registry| {
        map_registry_status(registry::register_interface(registry, interface, local.0))
    });
    if registry_status != Status::Ok {
        let _ = Memory::close(local.0);
        let _ = Memory::close(remote.0);
        return (registry_status, 0);
    }
    interface.owner = Some(owner);
    interface.channel = Some(local.0);
    *info = Topology::wire_info(interface);
    (Status::Ok, remote.0)
}

fn map_registry_status(status: hardware_manager_fidl::Status) -> Status {
    match status {
        hardware_manager_fidl::Status::Ok => Status::Ok,
        hardware_manager_fidl::Status::ErrInvalidHandle => Status::ErrInvalidHandle,
        hardware_manager_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        hardware_manager_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        hardware_manager_fidl::Status::ErrBufferTooSmall => Status::ErrBufferTooSmall,
        hardware_manager_fidl::Status::ErrPeerClosed => Status::ErrPeerClosed,
        hardware_manager_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        hardware_manager_fidl::Status::ErrAlreadyExists => Status::ErrAlreadyExists,
        hardware_manager_fidl::Status::ErrNotFound => Status::ErrNotFound,
        hardware_manager_fidl::Status::ErrBadState => Status::ErrBadState,
        hardware_manager_fidl::Status::ErrUnsupported => Status::ErrUnsupported,
        hardware_manager_fidl::Status::ErrInvalidArgs => Status::ErrInvalidArgs,
    }
}

fn release_interface(
    topology: &mut Topology,
    registry: Option<Channel>,
    interface_id: u64,
) -> Status {
    for device in topology.devices.values_mut() {
        if let Some(interface) = device
            .interfaces
            .iter_mut()
            .find(|interface| interface.id == interface_id)
        {
            interface.owner = None;
            if let Some(channel) = interface.channel.take() {
                let _ = Memory::close(channel);
            }
            if let Some(registry) = registry {
                let _ = registry::unregister_interface(registry, interface_id);
            }
            return Status::Ok;
        }
    }
    Status::ErrNotFound
}
