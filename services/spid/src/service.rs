use crate::{Runtime, registry, wire};
use alloc::vec::Vec;
use bexos_i2c_spi::{Request, SpiBundle, SpiOp, Status, SubmitResult, topology_proto};
use bexos_userspace::{
    Channel, Memory, ServiceGrant, Startup,
    live_migration::{Source, now_ms},
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use i2c_spi_fidl::*;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap_or_else(|_| bexos_userspace::exit());
    let mut state = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, start.migration_generation)
            .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let topology = topology_proto::decode(include_bytes!(env!("I2C_SPI_TOPOLOGY")))
            .unwrap_or_else(|_| bexos_userspace::exit());
        let mut state = Runtime::new(control, start.migration, topology)
            .unwrap_or_else(|_| bexos_userspace::exit());
        attach_startup_grants(&mut state, &start.service_grants);
        register_configured_peripherals(&mut state);
        Startup::ready(control).unwrap();
        bexos_userspace::log("spid: ready\n");
        state
    };
    serve(&mut state).await
}

async fn serve(state: &mut Runtime) -> ! {
    let control = state.control;
    let mut source = Source::new(state.migration);
    loop {
        let now = now_ms();
        if source.draining() {
            for controller in &mut state.controllers {
                let _ = controller.quiescence_ready(&mut state.backend, now);
            }
        }
        if source.poll(state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace_async::yield_once().await;
            continue;
        }
        poll_control(control, state, &mut source);
        poll_fixture_clients(state, &mut source);
        poll_clients(state, &mut source, now);
        for controller in &mut state.controllers {
            if controller.poll(&mut state.backend, now) {
                source.changed_keys([1, 2]);
            }
            while let Some(completion) = controller.next_completion() {
                wire::reply_spi_transfer(Channel(completion.client_channel), &completion.outcome);
                source.changed(1);
            }
        }
        bexos_userspace_async::yield_once().await;
    }
}

fn attach_startup_grants(state: &mut Runtime, grants: &[ServiceGrant]) {
    for grant in grants {
        if grant.protocol == "DeviceRegistry" {
            state.registry = Some(Channel(grant.endpoint));
        }
    }
}

fn register_configured_peripherals(state: &mut Runtime) {
    let Some(registry) = state.registry else {
        return;
    };
    for controller in &mut state.controllers {
        if registry::register_controller(registry, &controller.config)
            != hardware_manager_fidl::Status::Ok
        {
            continue;
        }
        for peripheral in controller.config.peripherals.clone() {
            let Ok((local, remote)) = Channel::pair() else {
                continue;
            };
            if controller
                .add_endpoint(local.0, peripheral.node_id, alloc::vec![1, 2, 3])
                .is_err()
            {
                let _ = Memory::close(local.0);
                let _ = Memory::close(remote.0);
                continue;
            }
            if registry::register_peripheral(
                registry,
                controller.config.node_id,
                &peripheral,
                remote.0,
            ) != hardware_manager_fidl::Status::Ok
            {
                controller.release_channel(local.0);
                let _ = Memory::close(local.0);
                let _ = Memory::close(remote.0);
            }
        }
    }
    state.registered = true;
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
            if binding.protocol_is("DeviceRegistry") {
                state.registry = Some(Channel(endpoint));
                source.changed(0);
                return;
            }
            if binding.protocol_is("DeterministicFixtureControl") {
                state.fixture_clients.push(BoundServiceEndpoint::new(
                    Channel(endpoint),
                    binding.method_ordinals,
                ));
                source.changed(0);
                return;
            }
        }
        let _ = Memory::close(endpoint);
        return;
    }
    wire::close(&message.handles);
}

fn poll_fixture_clients(state: &mut Runtime, source: &mut Source) {
    let mut index = 0;
    while index < state.fixture_clients.len() {
        let channel = state.fixture_clients[index].channel;
        let keep = match channel.try_recv() {
            Ok(message) => {
                handle_fixture_client(index, state, message.bytes, message.handles);
                source.changed_keys([2, 3]);
                true
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => false,
            Err(_) => true,
        };
        if keep {
            index += 1;
        } else {
            let channel = state.fixture_clients.remove(index).channel;
            let _ = Memory::close(channel.0);
            source.changed(3);
        }
    }
}

fn handle_fixture_client(index: usize, state: &mut Runtime, bytes: Vec<u8>, handles: Vec<u64>) {
    let channel = state.fixture_clients[index].channel;
    let Some((ordinal, request)) = wire::envelope(&bytes) else {
        wire::close(&handles);
        return;
    };
    if !state.fixture_clients[index].allows(ordinal) || !handles.is_empty() {
        wire::close(&handles);
        return;
    }
    match ordinal {
        1 => {
            let status = DeterministicFixtureControlInjectFailureRequest::decode(request, &[])
                .ok()
                .map_or(Status::InvalidArgs, |request| {
                    state.backend.inject_failure(
                        request.peripheral_node_id,
                        status_from_fidl(request.status),
                        request.count,
                    );
                    Status::Ok
                });
            wire::reply(
                channel,
                &DeterministicFixtureControlInjectFailureResponse {
                    status: wire::status(status),
                },
            );
        }
        2 => {
            let status = DeterministicFixtureControlDelayCompletionsRequest::decode(request, &[])
                .ok()
                .map_or(Status::InvalidArgs, |request| {
                    state
                        .backend
                        .set_delay(request.controller_node_id, request.delay_ms);
                    Status::Ok
                });
            wire::reply(
                channel,
                &DeterministicFixtureControlDelayCompletionsResponse {
                    status: wire::status(status),
                },
            );
        }
        3 => wire::reply(
            channel,
            &DeterministicFixtureControlOperationCountResponse {
                i2c_operations: state.backend.i2c_operations,
                spi_operations: state.backend.spi_operations,
            },
        ),
        _ => {}
    }
}

fn poll_clients(state: &mut Runtime, source: &mut Source, now: u64) {
    for controller_index in 0..state.controllers.len() {
        let mut index = 0;
        while index < state.controllers[controller_index].clients.len() {
            let channel = Channel(state.controllers[controller_index].clients[index].channel);
            let keep = match channel.try_recv() {
                Ok(message) => {
                    let keep = handle_client(
                        controller_index,
                        index,
                        state,
                        message.bytes,
                        message.handles,
                        now,
                    );
                    source.changed_keys([1, 2]);
                    keep
                }
                Err(kernel_fidl::Status::ErrPeerClosed) => false,
                Err(_) => true,
            };
            if keep {
                index += 1;
            } else {
                let channel = state.controllers[controller_index]
                    .clients
                    .remove(index)
                    .channel;
                state.controllers[controller_index].release_channel(channel);
                let _ = Memory::close(channel);
                source.changed(1);
            }
        }
    }
}

fn handle_client(
    controller_index: usize,
    client_index: usize,
    state: &mut Runtime,
    bytes: Vec<u8>,
    handles: Vec<u64>,
    now: u64,
) -> bool {
    let Some((ordinal, request)) = wire::envelope(&bytes) else {
        wire::close(&handles);
        return true;
    };
    let controller = &mut state.controllers[controller_index];
    let client = controller.clients[client_index].clone();
    if !client.allows(ordinal) || !handles.is_empty() {
        wire::close(&handles);
        return true;
    }
    match ordinal {
        1 => reply_info(controller, &client),
        2 => {
            let outcome = SpiDeviceTransferRequest::decode(request, &[])
                .ok()
                .and_then(|request| {
                    spi_request_from_fidl(&request)
                        .ok()
                        .map(|bundle| (request.deadline_ms, bundle))
                })
                .map_or_else(
                    || Err(Status::InvalidArgs),
                    |(deadline, bundle)| Ok((deadline, bundle)),
                );
            match outcome {
                Ok((deadline, bundle)) => match controller.submit(
                    client.channel,
                    client.peripheral_node_id,
                    Request::Spi(bundle),
                    deadline,
                    now,
                ) {
                    SubmitResult::Queued(_) => {}
                    SubmitResult::Rejected(status) => wire::reply_spi_transfer(
                        Channel(client.channel),
                        &bexos_i2c_spi::TransferOutcome {
                            status,
                            reads: Vec::new(),
                            operations_completed: 0,
                        },
                    ),
                },
                Err(status) => wire::reply_spi_transfer(
                    Channel(client.channel),
                    &bexos_i2c_spi::TransferOutcome {
                        status,
                        reads: Vec::new(),
                        operations_completed: 0,
                    },
                ),
            }
        }
        3 => {
            let response = SpiDeviceLockBusRequest::decode(request, &[]).ok().map_or(
                (Status::InvalidArgs, 0, 0),
                |request| {
                    let Ok((local, remote)) = Channel::pair() else {
                        return (Status::NoMemory, 0, 0);
                    };
                    match controller.lock_bus(
                        client.channel,
                        local.0,
                        client.peripheral_node_id,
                        alloc::vec![1, 2],
                        request.lease_ms as u64,
                        now,
                    ) {
                        Ok(expires) => (Status::Ok, remote.0, expires),
                        Err(status) => {
                            let _ = Memory::close(local.0);
                            let _ = Memory::close(remote.0);
                            (status, 0, 0)
                        }
                    }
                },
            );
            wire::reply(
                Channel(client.channel),
                &SpiDeviceLockBusResponse {
                    status: wire::status(response.0),
                    locked_device: HandleRef { raw: response.1 },
                    expires_at_ms: response.2,
                },
            );
        }
        _ => {}
    }
    true
}

fn reply_info(controller: &bexos_i2c_spi::BusController, client: &bexos_i2c_spi::ClientEndpoint) {
    let Some(peripheral) = controller
        .config
        .peripherals
        .iter()
        .find(|peripheral| peripheral.node_id == client.peripheral_node_id)
    else {
        return;
    };
    let bexos_i2c_spi::PeripheralConfig::Spi(spi) = &peripheral.config else {
        return;
    };
    wire::reply(
        Channel(client.channel),
        &SpiDeviceGetInfoResponse {
            status: i2c_spi_fidl::Status::Ok,
            info: SpiDeviceInfo {
                controller_node_id: controller.config.node_id,
                peripheral_node_id: peripheral.node_id,
                chip_select: spi.chip_select,
                min_speed_hz: spi.min_speed_hz,
                max_speed_hz: spi.max_speed_hz,
                mode_mask: spi.mode_mask,
                max_operations: bexos_i2c_spi::MAX_OPERATIONS_PER_BUNDLE as u32,
                max_transfer_bytes: bexos_i2c_spi::MAX_TRANSFER_BYTES_PER_BUNDLE as u32,
            },
        },
    );
}

fn spi_request_from_fidl(request: &SpiDeviceTransferRequest<'_>) -> Result<SpiBundle, Status> {
    let mut operations = Vec::new();
    for index in 0..request.operations.len() {
        let op = request
            .operations
            .get(index)
            .map_err(|_| Status::InvalidArgs)?;
        operations.push(match op.kind {
            SpiOpKind::Write => SpiOp::Write(collect_bytes(&op.write_bytes)?),
            SpiOpKind::Read => {
                if op.read_length == 0 || !op.write_bytes.is_empty() {
                    return Err(Status::InvalidArgs);
                }
                SpiOp::Read(op.read_length as usize)
            }
            SpiOpKind::FullDuplex => {
                let bytes = collect_bytes(&op.write_bytes)?;
                if bytes.is_empty() || op.read_length as usize != bytes.len() {
                    return Err(Status::InvalidArgs);
                }
                SpiOp::FullDuplex(bytes)
            }
        });
    }
    Ok(SpiBundle {
        operations,
        speed_hz: request.speed_hz,
        mode: wire::spi_mode(request.mode),
    })
}

fn collect_bytes(bytes: &[u8]) -> Result<Vec<u8>, Status> {
    Ok(bytes.to_vec())
}

fn status_from_fidl(status: i2c_spi_fidl::Status) -> Status {
    match status {
        i2c_spi_fidl::Status::Ok => Status::Ok,
        i2c_spi_fidl::Status::ErrInvalidHandle => Status::InvalidHandle,
        i2c_spi_fidl::Status::ErrAccessDenied => Status::AccessDenied,
        i2c_spi_fidl::Status::ErrNoMemory => Status::NoMemory,
        i2c_spi_fidl::Status::ErrBufferTooSmall => Status::BufferTooSmall,
        i2c_spi_fidl::Status::ErrPeerClosed => Status::PeerClosed,
        i2c_spi_fidl::Status::ErrTimedOut => Status::TimedOut,
        i2c_spi_fidl::Status::ErrAlreadyExists => Status::AlreadyExists,
        i2c_spi_fidl::Status::ErrInvalidArgs => Status::InvalidArgs,
        i2c_spi_fidl::Status::ErrNotFound => Status::NotFound,
        i2c_spi_fidl::Status::ErrBadState => Status::BadState,
        i2c_spi_fidl::Status::ErrUnsupported => Status::Unsupported,
        i2c_spi_fidl::Status::ErrNack => Status::Nack,
        i2c_spi_fidl::Status::ErrArbitrationLost => Status::ArbitrationLost,
        i2c_spi_fidl::Status::ErrQueueFull => Status::QueueFull,
        i2c_spi_fidl::Status::ErrTooLarge => Status::TooLarge,
        i2c_spi_fidl::Status::ErrLockExpired => Status::LockExpired,
        i2c_spi_fidl::Status::ErrInjectedFailure => Status::InjectedFailure,
    }
}
