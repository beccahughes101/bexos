use alloc::vec::Vec;
use bexos_trace::{MAX_PRODUCER_BUFFER_SIZE, TraceProducer};
use bexos_userspace::live_migration::Source;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Memory, Startup, log};
use kernel_fidl::Status;
use tracing_fidl::{
    FidlDecode, HandleRef, TraceControllerGetStatusRequest, TraceControllerStartSessionRequest,
    TraceControllerStartSessionResponse, TraceControllerStopSessionRequest,
    TraceControllerStopSessionResponse, TraceRegistryRegisterProducerRequest,
    TraceRegistryRegisterProducerResponse, TraceRegistryReplaceProducerRequest,
    TraceRegistryReplaceProducerResponse, TraceRegistryUnregisterProducerRequest,
    TraceRegistryUnregisterProducerResponse,
};

use crate::manager::TraceManager;
use crate::migration::Runtime;
use crate::wire::{
    envelope, handle_refs, metadata_protocol, send_response, status_from_error, trace_buffer_mode,
    trace_output_format, trace_status_response,
};

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("traced startup");
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state).await,
            Err(_) => bexos_userspace::exit(),
        }
    }
    Startup::ready(control).unwrap();
    log("traced: ready\n");
    serve(Runtime::new(
        control,
        startup.migration,
        TraceManager::new(),
    ))
    .await
}

async fn serve(mut runtime: Runtime) -> ! {
    let mut source = Source::new(runtime.migration);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if let Ok(message) = runtime.control.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("TraceController")
                        || binding.protocol_is("TraceRegistry")
                    {
                        runtime
                            .clients
                            .push(BoundServiceEndpoint::new_with_protocol(
                                Channel(endpoint),
                                binding.method_ordinals,
                                &binding.protocol,
                            ));
                        source.changed(1);
                    }
                } else if matches!(
                    metadata_protocol(metadata),
                    Some("TraceController" | "TraceRegistry")
                ) {
                    let _ = Memory::close(endpoint);
                }
            }
        }
        if poll_trace_clients(&mut runtime.clients, &mut runtime.manager) {
            source.changed_keys([1, 2]);
        }
        runtime
            .manager
            .record_debug_event("traced:heartbeat", now_ns());
        if source.active() {
            source.changed(2);
        }
        bexos_userspace::yield_now();
    }
}

fn poll_trace_clients(clients: &mut Vec<BoundServiceEndpoint>, manager: &mut TraceManager) -> bool {
    let mut changed = false;
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if !client.allows(ordinal) {
                send_invalid_method_response(client.channel, &client.protocol, ordinal);
                close_unconsumed_handles(&message.handles);
                return true;
            }
            if client.protocol == "TraceRegistry" {
                match ordinal {
                    1 => handle_registry_register(client.channel, manager, req, &handles),
                    2 => handle_registry_replace(client.channel, manager, req, &handles),
                    3 => {
                        close_unconsumed_handles(&message.handles);
                        handle_registry_unregister(client.channel, manager, req, &handles);
                    }
                    _ => {
                        send_invalid_method_response(client.channel, &client.protocol, ordinal);
                        close_unconsumed_handles(&message.handles);
                    }
                }
            } else {
                match ordinal {
                    1 => {
                        close_unconsumed_handles(&message.handles);
                        handle_start(client.channel, manager, req, &handles);
                    }
                    2 => {
                        close_unconsumed_handles(&message.handles);
                        handle_stop(client.channel, manager, req, &handles);
                    }
                    3 => {
                        close_unconsumed_handles(&message.handles);
                        handle_status(client.channel, manager, req, &handles);
                    }
                    _ => {
                        send_invalid_method_response(client.channel, &client.protocol, ordinal);
                        close_unconsumed_handles(&message.handles);
                    }
                }
            }
            true
        }
        Err(Status::ErrPeerClosed) => {
            changed = true;
            false
        }
        Err(_) => true,
    });
    changed
}

fn handle_start(channel: Channel, manager: &mut TraceManager, req: &[u8], handles: &[HandleRef]) {
    let status = match TraceControllerStartSessionRequest::decode(req, handles) {
        Ok(request) => manager
            .start_with_format(
                request.categories.0,
                trace_buffer_mode(request.buffer_mode),
                request.buffer_size_kb,
                trace_output_format(request.output_format),
                now_ns(),
            )
            .map(|()| Status::Ok)
            .unwrap_or_else(status_from_error),
        Err(_) => {
            for handle in handles {
                if handle.raw != 0 {
                    let _ = Memory::close(handle.raw);
                }
            }
            Status::ErrInvalidArgs
        }
    };
    send_response(channel, &TraceControllerStartSessionResponse { status });
}

fn handle_stop(channel: Channel, manager: &mut TraceManager, req: &[u8], handles: &[HandleRef]) {
    let response = match TraceControllerStopSessionRequest::decode(req, handles) {
        Ok(_) => match manager.stop(now_ns()) {
            Ok(bytes) => match Memory::from_bytes(&bytes) {
                Ok(trace_file) => TraceControllerStopSessionResponse {
                    status: Status::Ok,
                    trace_len: bytes.len() as u64,
                    trace_file: HandleRef { raw: trace_file },
                },
                Err(_) => TraceControllerStopSessionResponse {
                    status: Status::ErrNoMemory,
                    trace_len: 0,
                    trace_file: HandleRef { raw: 0 },
                },
            },
            Err(error) => TraceControllerStopSessionResponse {
                status: status_from_error(error),
                trace_len: 0,
                trace_file: HandleRef { raw: 0 },
            },
        },
        Err(_) => TraceControllerStopSessionResponse {
            status: Status::ErrInvalidArgs,
            trace_len: 0,
            trace_file: HandleRef { raw: 0 },
        },
    };
    send_response(channel, &response);
}

fn handle_status(channel: Channel, manager: &mut TraceManager, req: &[u8], handles: &[HandleRef]) {
    manager.refresh_status();
    let response = match TraceControllerGetStatusRequest::decode(req, handles) {
        Ok(_) => trace_status_response(Status::Ok, manager.status()),
        Err(_) => trace_status_response(Status::ErrInvalidArgs, TraceManager::new().status()),
    };
    send_response(channel, &response);
}

fn handle_registry_register(
    channel: Channel,
    manager: &mut TraceManager,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = match TraceRegistryRegisterProducerRequest::decode(req, handles) {
        Ok(request) => register_or_replace(manager, request.into(), false),
        Err(_) => Status::ErrInvalidArgs,
    };
    send_response(channel, &TraceRegistryRegisterProducerResponse { status });
}

fn handle_registry_replace(
    channel: Channel,
    manager: &mut TraceManager,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = match TraceRegistryReplaceProducerRequest::decode(req, handles) {
        Ok(request) => register_or_replace(manager, request.into(), true),
        Err(_) => Status::ErrInvalidArgs,
    };
    send_response(channel, &TraceRegistryReplaceProducerResponse { status });
}

fn handle_registry_unregister(
    channel: Channel,
    manager: &mut TraceManager,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = match TraceRegistryUnregisterProducerRequest::decode(req, handles) {
        Ok(request) => match manager.unregister_producer(request.producer_id, now_ns()) {
            Ok(Some(vmo)) => {
                let _ = Memory::close(vmo);
                Status::Ok
            }
            Ok(None) => Status::Ok,
            Err(error) => status_from_error(error),
        },
        Err(_) => Status::ErrInvalidArgs,
    };
    send_response(channel, &TraceRegistryUnregisterProducerResponse { status });
}

struct ProducerRequest<'a> {
    producer_id: u64,
    pid: u64,
    main_tid: u64,
    process_name: &'a str,
    categories: u32,
    mapped_len: u64,
    buffer: HandleRef,
}

impl<'a> From<TraceRegistryRegisterProducerRequest<'a>> for ProducerRequest<'a> {
    fn from(value: TraceRegistryRegisterProducerRequest<'a>) -> Self {
        Self {
            producer_id: value.producer_id,
            pid: value.pid,
            main_tid: value.main_tid,
            process_name: value.process_name,
            categories: value.categories.0,
            mapped_len: value.mapped_len,
            buffer: value.buffer,
        }
    }
}

impl<'a> From<TraceRegistryReplaceProducerRequest<'a>> for ProducerRequest<'a> {
    fn from(value: TraceRegistryReplaceProducerRequest<'a>) -> Self {
        Self {
            producer_id: value.producer_id,
            pid: value.pid,
            main_tid: value.main_tid,
            process_name: value.process_name,
            categories: value.categories.0,
            mapped_len: value.mapped_len,
            buffer: value.buffer,
        }
    }
}

fn register_or_replace(
    manager: &mut TraceManager,
    request: ProducerRequest<'_>,
    replace: bool,
) -> Status {
    if request.buffer.raw == 0 || request.mapped_len == 0 {
        return Status::ErrInvalidArgs;
    }
    let mapped_len = request
        .mapped_len
        .min(MAX_PRODUCER_BUFFER_SIZE as u64)
        .max((bexos_trace::HEADER_SIZE + bexos_trace::SLOT_SIZE) as u64);
    let mapped_addr = match Memory::map(request.buffer.raw, mapped_len, 2 | 4) {
        Ok(addr) => addr,
        Err(status) => return status,
    };
    let producer = TraceProducer {
        id: request.producer_id,
        pid: request.pid,
        main_tid: request.main_tid,
        process_name: request.process_name.into(),
        categories: request.categories,
    };
    let result = if replace {
        manager.replace_shared_producer(
            producer,
            request.buffer.raw,
            mapped_addr,
            mapped_len,
            now_ns(),
        )
    } else {
        manager
            .register_shared_producer(
                producer,
                request.buffer.raw,
                mapped_addr,
                mapped_len,
                now_ns(),
            )
            .map(|()| None)
    };
    match result {
        Ok(old) => {
            if let Some(vmo) = old {
                let _ = Memory::close(vmo);
            }
            Status::Ok
        }
        Err(error) => {
            let _ = Memory::unmap(mapped_addr, mapped_len);
            let _ = Memory::close(request.buffer.raw);
            status_from_error(error)
        }
    }
}

fn send_invalid_method_response(channel: Channel, protocol: &str, ordinal: u64) {
    match (protocol, ordinal) {
        ("TraceRegistry", 1) => send_response(
            channel,
            &TraceRegistryRegisterProducerResponse {
                status: Status::ErrInvalidArgs,
            },
        ),
        ("TraceRegistry", 2) => send_response(
            channel,
            &TraceRegistryReplaceProducerResponse {
                status: Status::ErrInvalidArgs,
            },
        ),
        ("TraceRegistry", 3) => send_response(
            channel,
            &TraceRegistryUnregisterProducerResponse {
                status: Status::ErrInvalidArgs,
            },
        ),
        (_, 1) => send_response(
            channel,
            &TraceControllerStartSessionResponse {
                status: Status::ErrInvalidArgs,
            },
        ),
        (_, 2) => send_response(
            channel,
            &TraceControllerStopSessionResponse {
                status: Status::ErrInvalidArgs,
                trace_len: 0,
                trace_file: HandleRef { raw: 0 },
            },
        ),
        (_, 3) => send_response(
            channel,
            &trace_status_response(Status::ErrInvalidArgs, TraceManager::new().status()),
        ),
        _ => {}
    }
}

fn close_unconsumed_handles(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}

fn now_ns() -> u64 {
    let frequency = bexos_userspace::syscall::frequency().max(1);
    bexos_userspace::syscall::ticks().saturating_mul(1_000_000_000) / frequency
}
