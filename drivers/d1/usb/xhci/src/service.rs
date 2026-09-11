use crate::{Hardware, Runtime, wire};
use alloc::vec::Vec;
use bexos_usb_host::validate_transfer;
use bexos_userspace::{
    Channel, HardwareResourceKind, Memory, Startup,
    live_migration::Source,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use usb_host_fidl::*;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap_or_else(|_| bexos_userspace::exit());
    let state = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, start.migration_generation)
            .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let has_iommu = start
            .driver_resources
            .iter()
            .any(|r| r.kind == HardwareResourceKind::IommuDomain);
        if !has_iommu {
            bexos_userspace::log("xhcid: missing IOMMU isolation\n");
            bexos_userspace::exit();
        }
        let hardware = match Hardware::connect(&start.driver_resources) {
            Ok(hardware) => hardware,
            Err(error) => {
                bexos_userspace::log(&alloc::format!("xhcid: init failed {error:?}\n"));
                bexos_userspace::exit()
            }
        };
        Startup::ready(control).unwrap();
        Runtime::new(control, start.migration, Some(hardware))
    };
    serve(state).await
}

async fn serve(mut state: Runtime) -> ! {
    let control = state.control;
    let mut source = Source::new(state.migration);
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace_async::yield_once().await;
            continue;
        }
        poll_control(control, &mut state, &mut source);
        poll_clients(&mut state, &mut source);
        bexos_userspace_async::yield_once().await;
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
            if binding.protocol_is("XhciController") {
                state.clients.push(BoundServiceEndpoint::new_with_protocol(
                    Channel(endpoint),
                    binding.method_ordinals,
                    "XhciController",
                ));
                source.changed(1);
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
                let keep = handle_client_message(index, state, message.bytes, message.handles);
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
            source.changed(1);
        }
    }
}

fn handle_client_message(
    client_index: usize,
    state: &mut Runtime,
    bytes: Vec<u8>,
    handles: Vec<u64>,
) -> bool {
    let Some((ordinal, request)) = wire::envelope(&bytes) else {
        wire::close(&handles);
        return true;
    };
    let allowed = state.clients[client_index].allows(ordinal);
    let channel = state.clients[client_index].channel;
    let refs = wire::refs(&handles);
    match ordinal {
        1 if allowed && handles.is_empty() => {
            let h = state.hardware.as_ref().unwrap();
            wire::reply(
                channel,
                &XhciControllerGetControllerInfoResponse {
                    status: Status::Ok,
                    port_count: h.caps.max_ports as u16,
                    scratchpad_count: h.caps.scratchpads,
                    max_slots: h.caps.max_slots,
                    generation: h.generation,
                },
            );
        }
        2 if allowed => {
            let decoded = XhciControllerRegisterBusManagerRequest::decode(request, &refs);
            let status = if decoded.is_ok()
                && handles.len() == 1
                && state.bus.is_none()
                && state.bus_events.is_none()
            {
                let (local, remote) = match Channel::pair() {
                    Ok(pair) => pair,
                    Err(_) => {
                        wire::close(&handles);
                        wire::reply(
                            channel,
                            &XhciControllerRegisterBusManagerResponse {
                                status: Status::ErrNoMemory,
                                bus: HandleRef { raw: 0 },
                            },
                        );
                        return true;
                    }
                };
                state.bus = Some(local);
                state.bus_events = Some(Channel(handles[0]));
                wire::reply(
                    channel,
                    &XhciControllerRegisterBusManagerResponse {
                        status: Status::Ok,
                        bus: HandleRef { raw: remote.0 },
                    },
                );
                return true;
            } else {
                wire::close(&handles);
                Status::ErrInvalidArgs
            };
            wire::reply(
                channel,
                &XhciControllerRegisterBusManagerResponse {
                    status,
                    bus: HandleRef { raw: 0 },
                },
            );
        }
        3 if allowed => {
            let decoded = XhciControllerRegisterBufferRequest::decode(request, &refs);
            let mut info = BufferInfo {
                buffer_id: 0,
                size_bytes: 0,
                device_address: 0,
            };
            let status = if let Ok(q) = decoded {
                register_buffer(state, &q, &handles, &mut info)
            } else {
                wire::close(&handles);
                Status::ErrInvalidArgs
            };
            wire::reply(
                channel,
                &XhciControllerRegisterBufferResponse { status, info },
            );
        }
        4 if allowed && handles.is_empty() => {
            let status = XhciControllerUnregisterBufferRequest::decode(request, &[])
                .ok()
                .map_or(Status::ErrInvalidArgs, |q| {
                    unregister_buffer(state, q.buffer_id)
                });
            wire::reply(channel, &XhciControllerUnregisterBufferResponse { status });
        }
        5 if allowed && handles.is_empty() => {
            let status = XhciControllerSubmitTransferRequest::decode(request, &[])
                .ok()
                .map_or(Status::ErrInvalidArgs, |q| {
                    let buffer = state
                        .buffers
                        .get(&q.request.buffer_id)
                        .map(|buffer| buffer.info);
                    match validate_transfer(&q.request, buffer, state.limits) {
                        Ok(()) => {
                            if let Some(buffer) = state.buffers.get_mut(&q.request.buffer_id) {
                                buffer.in_flight = buffer.in_flight.saturating_add(1);
                            }
                            state.queued_transfers.push(q.request.transfer_id);
                            Status::Ok
                        }
                        Err(error) => wire::map_status(error),
                    }
                });
            wire::reply(channel, &XhciControllerSubmitTransferResponse { status });
        }
        6 if allowed && handles.is_empty() => {
            let status = XhciControllerCancelTransferRequest::decode(request, &[])
                .ok()
                .map_or(Status::ErrInvalidArgs, |q| {
                    let before = state.queued_transfers.len();
                    state.queued_transfers.retain(|id| *id != q.transfer_id);
                    if before == state.queued_transfers.len() {
                        Status::ErrNotFound
                    } else {
                        Status::Ok
                    }
                });
            wire::reply(channel, &XhciControllerCancelTransferResponse { status });
        }
        7 if allowed && handles.is_empty() => {
            let status = XhciControllerRecoverEndpointRequest::decode(request, &[])
                .map(|_| Status::Ok)
                .unwrap_or(Status::ErrInvalidArgs);
            wire::reply(channel, &XhciControllerRecoverEndpointResponse { status });
        }
        8 if allowed && handles.is_empty() => {
            let (status, handle) = match Channel::pair() {
                Ok((local, remote)) => {
                    state.completions.push(local);
                    (Status::Ok, remote.0)
                }
                Err(_) => (Status::ErrNoMemory, 0),
            };
            wire::reply(
                channel,
                &XhciControllerGetCompletionChannelResponse {
                    status,
                    completions: HandleRef { raw: handle },
                },
            );
        }
        9 if allowed && handles.is_empty() => {
            let h = state.hardware.as_ref().unwrap();
            wire::reply(
                channel,
                &XhciControllerGetMigrationMarkersResponse {
                    command_ring: h.command_ring.device_address(),
                    event_ring: h.event_ring.device_address(),
                    scratchpad_table: h.scratchpad_table.device_address(),
                },
            );
        }
        _ => {
            wire::close(&handles);
            return true;
        }
    }
    true
}

fn register_buffer(
    state: &mut Runtime,
    request: &XhciControllerRegisterBufferRequest,
    handles: &[u64],
    info: &mut BufferInfo,
) -> Status {
    if handles.len() != 1 || state.buffers.len() >= state.limits.max_buffers as usize {
        wire::close(handles);
        return Status::ErrInvalidArgs;
    }
    if request.size_bytes == 0 {
        wire::close(handles);
        return Status::ErrInvalidArgs;
    }
    let id = state.next_buffer_id;
    state.next_buffer_id = state.next_buffer_id.saturating_add(1).max(1);
    let domain = state.hardware.as_ref().unwrap().iommu_domain;
    let mapped = Memory::map_dma(domain, handles[0], request.offset, request.size_bytes, 6);
    let Ok((device_address, _token)) = mapped else {
        wire::close(handles);
        return Status::ErrAccessDenied;
    };
    *info = BufferInfo {
        buffer_id: id,
        size_bytes: request.size_bytes,
        device_address,
    };
    state.buffers.insert(
        id,
        crate::migration::RegisteredBuffer {
            info: bexos_usb_host::transfer::BufferRegistration {
                id,
                size_bytes: request.size_bytes,
                direction: request.direction,
            },
            handle: handles[0],
            device_address,
            size_bytes: request.size_bytes,
            direction: request.direction,
            in_flight: 0,
        },
    );
    Status::Ok
}

fn unregister_buffer(state: &mut Runtime, buffer_id: u32) -> Status {
    match state.buffers.get(&buffer_id) {
        Some(buffer) if buffer.in_flight != 0 => Status::ErrBadState,
        Some(_) => {
            state.buffers.remove(&buffer_id);
            Status::Ok
        }
        None => Status::ErrNotFound,
    }
}
