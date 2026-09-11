use crate::{Runtime, block, wire};
use alloc::vec::Vec;
use bexos_userspace::{
    Channel, HardwareResourceKind, Memory, Startup,
    live_migration::Source,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use block_fidl::*;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap_or_else(|_| bexos_userspace::exit());
    let state = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, start.migration_generation)
            .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let mut runtime = Runtime::new(control, start.migration);
        runtime.lifecycle = start.driver_lifecycle;
        runtime.interface = start
            .driver_resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::BusControl)
            .map(|resource| Channel(resource.handle));
        runtime.node = start.arg1;
        if runtime.interface.is_none() {
            bexos_userspace::log("usb-bot: missing scoped interface channel\n");
            bexos_userspace::exit();
        }
        Startup::ready(control).unwrap();
        runtime
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
        poll_fifos(&mut state, &mut source);
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
            if binding.protocol_is("BlockDevice") {
                state.clients.push(BoundServiceEndpoint::new(
                    Channel(endpoint),
                    binding.method_ordinals,
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
                handle_client(index, state, message.bytes, message.handles);
                source.changed_keys([1, 2]);
                true
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

fn handle_client(index: usize, state: &mut Runtime, bytes: Vec<u8>, handles: Vec<u64>) {
    let channel = state.clients[index].channel;
    let Some((ordinal, request)) = wire::envelope(&bytes) else {
        wire::close(&handles);
        return;
    };
    if !state.clients[index].allows(ordinal) {
        wire::close(&handles);
        return;
    }
    let refs = wire::refs(&handles);
    match ordinal {
        1 if handles.is_empty() => {
            wire::reply(
                channel,
                &BlockDeviceGetInfoResponse {
                    info: block::BlockServer::info(
                        state.transport.block_size,
                        state.transport.block_count,
                    ),
                },
            );
        }
        2 => {
            let (status, vmo_id) = BlockDeviceRegisterBufferRequest::decode(request, &refs)
                .ok()
                .filter(|_| handles.len() == 1)
                .map_or((Status::ErrInvalidArgs, 0), |_| {
                    let id = state.block.next_buffer_id;
                    state.block.next_buffer_id = state.block.next_buffer_id.wrapping_add(1).max(1);
                    match Memory::map(handles[0], block::REGISTERED_BUFFER_BYTES, 6) {
                        Ok(mapped) => {
                            state.block.buffers.insert(
                                id,
                                block::Buffer {
                                    handle: handles[0],
                                    mapped,
                                    size_bytes: block::REGISTERED_BUFFER_BYTES,
                                },
                            );
                            (Status::Ok, id)
                        }
                        Err(_) => {
                            wire::close(&handles);
                            (Status::ErrInvalidHandle, 0)
                        }
                    }
                });
            wire::reply(
                channel,
                &BlockDeviceRegisterBufferResponse { status, vmo_id },
            );
        }
        3 if handles.is_empty() => {
            let status = BlockDeviceUnregisterBufferRequest::decode(request, &[])
                .ok()
                .map_or(Status::ErrInvalidArgs, |q| {
                    if state.block.in_flight != 0 {
                        Status::ErrInvalidArgs
                    } else if let Some(buffer) = state.block.buffers.remove(&q.vmo_id) {
                        let _ = Memory::unmap(buffer.mapped, buffer.size_bytes);
                        let _ = Memory::close(buffer.handle);
                        Status::Ok
                    } else {
                        Status::ErrInvalidArgs
                    }
                });
            wire::reply(channel, &BlockDeviceUnregisterBufferResponse { status });
        }
        4 if handles.is_empty() => {
            let (status, fifo) = match Channel::pair() {
                Ok((local, remote)) => {
                    state.fifos.push(local);
                    (Status::Ok, remote.0)
                }
                Err(_) => (Status::ErrNoMemory, 0),
            };
            wire::reply(
                channel,
                &BlockDeviceGetFifoResponse {
                    status,
                    fifo_handle: HandleRef { raw: fifo },
                },
            );
        }
        5 if handles.is_empty() => {
            wire::reply(
                channel,
                &BlockDeviceGetMigrationMarkersResponse {
                    sq_physical: state.transport.tag as u64,
                    cq_physical: state.transport.reset_count,
                    payload_physical: state.transport.completed_write_watermark,
                },
            );
        }
        _ => wire::close(&handles),
    }
}

fn poll_fifos(state: &mut Runtime, source: &mut Source) {
    for fifo in &state.fifos {
        if let Ok(message) = fifo.try_recv() {
            if let Ok(request) = BlockRequest::decode(&message.bytes, &[]) {
                state.queued.push(request.req_id);
                let info = block::BlockServer::info(
                    state.transport.block_size,
                    state.transport.block_count,
                );
                let response = state.block.validate(&request, info);
                if response.status == Status::Ok && request.opcode == BlockOpcode::Write {
                    state.transport.completed_write_watermark = request.req_id;
                }
                if response.status == Status::Ok && request.opcode == BlockOpcode::Flush {
                    let _ = state.transport.flush_cbw();
                }
                let mut out = [0; 64];
                if let Ok(encoded) = response.encode(&mut out, &mut []) {
                    let _ = fifo.send(&out[..encoded.bytes], &[]);
                }
                state.queued.retain(|id| *id != request.req_id);
                source.changed_keys([0, 2]);
            }
            wire::close(&message.handles);
        }
    }
}
