extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Memory, Startup, log};
use kernel_fidl::Status;
use tee_manager_fidl::{
    FidlDecode, FidlEncode, HandleRef, TeeManagerActivateTrustedAppPackageRequest,
    TeeManagerActivateTrustedAppPackageResponse, TeeManagerCloseSessionRequest,
    TeeManagerCloseSessionResponse, TeeManagerDeactivateTrustedAppPackageRequest,
    TeeManagerDeactivateTrustedAppPackageResponse, TeeManagerGetTeeInfoRequest,
    TeeManagerGetTeeInfoResponse, TeeManagerGetTeeUpdateStatusRequest,
    TeeManagerGetTeeUpdateStatusResponse, TeeManagerInstallTrustedAppRequest,
    TeeManagerInstallTrustedAppResponse, TeeManagerInvokeCommandRequest,
    TeeManagerInvokeCommandResponse, TeeManagerListTrustedAppsRequest,
    TeeManagerListTrustedAppsResponse, TeeManagerOpenSessionByEndpointRequest,
    TeeManagerOpenSessionByEndpointResponse, TeeManagerOpenSessionRequest,
    TeeManagerOpenSessionResponse, TeeManagerQueryTrustedAppPackageRequest,
    TeeManagerQueryTrustedAppPackageResponse, TeeManagerUninstallTrustedAppRequest,
    TeeManagerUninstallTrustedAppResponse, TeeManagerUpdateTeeCoreRequest,
    TeeManagerUpdateTeeCoreResponse, TeeStatus, TeeUpdatePhase, TeeUpdateStatus, WireVector,
};

use crate::migration::{MigratableBackend, Runtime};
use crate::{DriverBackend, TeeBackend, TeeService, trusted_apps_to_fidl};

pub async fn main(channel: u64) -> ! {
    let manager = Channel(channel);
    let startup = Startup::receive(manager).unwrap();
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            manager,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state).await,
            Err(_) => bexos_userspace::exit(),
        }
    }
    // The secure-monitor grant is attached to the process handle after launch.
    // Wait for appd's explicit acknowledgement so the driver cannot race its
    // pin/SMC probe against that security transition.
    let authority = manager.recv();
    if !matches!(authority, Ok(message) if message.bytes == b"bexos.authority.ready" && message.handles.is_empty())
    {
        log("teed: secure-monitor authority handshake failed; failing closed\n");
        bexos_userspace::exit();
    }
    let backend = match DriverBackend::load(&[]) {
        Ok(backend) => backend,
        Err(error) => {
            log(&format!(
                "teed: tee driver load/probe failed status={error:?}; failing closed\n"
            ));
            bexos_userspace::exit();
        }
    };
    let service = TeeService::new(backend);
    let _ = Startup::ready(manager);
    log("teed: service ready\n");
    serve(Runtime::new(manager, startup.migration, service)).await
}

async fn serve<B: MigratableBackend + Default>(mut runtime: Runtime<B>) -> ! {
    let mut source = bexos_userspace::live_migration::Source::new(runtime.migration);
    let mut transport_paused = false;
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.active() {
            if !transport_paused {
                if let Err(status) = runtime.service.backend_mut().pause_backend() {
                    log(&format!(
                        "teed: transport quiescence failed status={status:?}\n"
                    ));
                    let _ = bexos_userspace::migration::abort();
                    bexos_userspace::exit();
                }
                transport_paused = true;
            }
            bexos_userspace_async::yield_once().await;
            continue;
        }
        if transport_paused {
            // An aborted handover resumes the source using its original
            // logical sessions and the same RPMB endpoint.
            if runtime.service.backend_mut().activate_backend().is_err() {
                log("teed: source transport recovery failed; failing closed\n");
                bexos_userspace::exit();
            }
            transport_paused = false;
        }
        if let Ok(message) = runtime.control.try_recv() {
            if message.bytes == b"bexos.rpmb.bind" && message.handles.len() == 1 {
                let result = runtime
                    .service
                    .backend_mut()
                    .attach_rpmb(Channel(message.handles[0]));
                let status: i32 = if result.is_ok() { 0 } else { -1 };
                let _ = runtime.control.send(&status.to_le_bytes(), &[]);
                if result.is_err() {
                    log("teed: RPMB proxy initialization failed; failing closed\n");
                    bexos_userspace::exit();
                }
                log("teed: RPMB proxy connected\n");
                source.changed(0);
                continue;
            }
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("TeeManager") {
                        runtime.clients.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                        source.changed(0);
                        log("teed: TeeManager client connected\n");
                    }
                } else if metadata_protocol(metadata) == Some("TeeManager") {
                    let _ = Memory::close(endpoint);
                    log("teed: TeeManager client connected\n");
                }
            }
        }
        let changed = poll_clients(&mut runtime).await;
        if changed {
            source.changed(0);
        }
        if let Err(status) = runtime.service.backend_mut().progress_storage() {
            log(&format!(
                "teed: storage proxy disconnected status={status:?}; failing closed\n"
            ));
            bexos_userspace::exit();
        }
        bexos_userspace_async::yield_once().await;
    }
}

async fn poll_clients<B: TeeBackend>(runtime: &mut Runtime<B>) -> bool {
    let mut changed = false;
    let clients = core::mem::take(&mut runtime.clients);
    for client in clients {
        match poll_client(&client, &mut runtime.service).await {
            Ok(()) => {
                changed = true;
                runtime.clients.push(client);
            }
            Err(Status::ErrTimedOut) => runtime.clients.push(client),
            Err(Status::ErrPeerClosed) => {}
            Err(_) => runtime.clients.push(client),
        }
    }
    changed
}

async fn poll_client<B: TeeBackend>(
    client: &BoundServiceEndpoint,
    service: &mut TeeService<B>,
) -> Result<(), Status> {
    let channel = client.channel;
    let message = channel.try_recv()?;
    let _received_handles = ReceivedHandles(message.handles.clone());
    let (ordinal, req) = envelope(&message.bytes);
    let handles: Vec<_> = message
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect();
    if !client.allows(ordinal) {
        return Ok(());
    }
    match ordinal {
        1 => {
            let _ = TeeManagerGetTeeInfoRequest::decode(req, &handles);
            let response = match service.info().await {
                Ok(info) => TeeManagerGetTeeInfoResponse {
                    status: TeeStatus::Ok,
                    present: info.present,
                    kind: info.kind,
                    secure_os_version: info.secure_os_version,
                    anti_rollback_version: info.anti_rollback_version,
                },
                Err(status) => TeeManagerGetTeeInfoResponse {
                    status,
                    present: false,
                    kind: tee_manager_fidl::TeeKind::SoftwareEmu,
                    secure_os_version: 0,
                    anti_rollback_version: 0,
                },
            };
            send_response(channel, &response);
        }
        2 => {
            let _ = TeeManagerListTrustedAppsRequest::decode(req, &handles);
            match service.list_apps().await {
                Ok(apps) => {
                    let fidl_apps = trusted_apps_to_fidl(&apps);
                    send_response(
                        channel,
                        &TeeManagerListTrustedAppsResponse {
                            status: TeeStatus::Ok,
                            apps: WireVector::from_slice(&fidl_apps),
                        },
                    );
                }
                Err(status) => send_response(
                    channel,
                    &TeeManagerListTrustedAppsResponse {
                        status,
                        apps: WireVector::from_slice(&[]),
                    },
                ),
            }
        }
        3 => {
            let response = match TeeManagerInstallTrustedAppRequest::decode(req, &handles) {
                Ok(request) => match vmo_bytes(request.ta_payload.raw, request.ta_payload_len) {
                    Ok((mapped, bytes)) => {
                        let result = service.install_app(bytes).await;
                        let _ = mapped.map(|(va, len)| Memory::unmap(va, len));
                        result.map_or_else(
                            |status| TeeManagerInstallTrustedAppResponse {
                                status,
                                uuid: [0; 16],
                            },
                            |uuid| TeeManagerInstallTrustedAppResponse {
                                status: TeeStatus::Ok,
                                uuid,
                            },
                        )
                    }
                    Err(status) => TeeManagerInstallTrustedAppResponse {
                        status,
                        uuid: [0; 16],
                    },
                },
                Err(_) => TeeManagerInstallTrustedAppResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    uuid: [0; 16],
                },
            };
            send_response(channel, &response);
        }
        4 => {
            let response = match TeeManagerUninstallTrustedAppRequest::decode(req, &handles) {
                Ok(request) => TeeManagerUninstallTrustedAppResponse {
                    status: service.uninstall_app(request.uuid).await,
                },
                Err(_) => TeeManagerUninstallTrustedAppResponse {
                    status: TeeStatus::ErrInvalidArgs,
                },
            };
            send_response(channel, &response);
        }
        5 => {
            let response = match TeeManagerOpenSessionRequest::decode(req, &handles) {
                Ok(request) => match service.open_session(request.uuid).await {
                    Ok(session_id) => TeeManagerOpenSessionResponse {
                        status: TeeStatus::Ok,
                        session_id,
                    },
                    Err(status) => TeeManagerOpenSessionResponse {
                        status,
                        session_id: 0,
                    },
                },
                Err(_) => TeeManagerOpenSessionResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    session_id: 0,
                },
            };
            send_response(channel, &response);
        }
        6 => {
            let response = match TeeManagerCloseSessionRequest::decode(req, &handles) {
                Ok(request) => TeeManagerCloseSessionResponse {
                    status: service.close_session(request.session_id).await,
                },
                Err(_) => TeeManagerCloseSessionResponse {
                    status: TeeStatus::ErrInvalidArgs,
                },
            };
            send_response(channel, &response);
        }
        7 => {
            let response = match TeeManagerInvokeCommandRequest::decode(req, &handles) {
                Ok(request) => match vmo_bytes(request.payload.raw, request.payload_len) {
                    Ok((mapped, payload)) => {
                        let result = service
                            .invoke(request.session_id, request.command_id, payload)
                            .await;
                        let _ = mapped.map(|(va, len)| Memory::unmap(va, len));
                        result.map_or_else(
                            |status| TeeManagerInvokeCommandResponse {
                                status,
                                response: HandleRef { raw: 0 },
                                response_len: 0,
                            },
                            |result| {
                                let vmo = Memory::from_bytes(&result.bytes).unwrap_or(0);
                                TeeManagerInvokeCommandResponse {
                                    status: if vmo == 0 {
                                        TeeStatus::ErrNoMemory
                                    } else {
                                        TeeStatus::Ok
                                    },
                                    response: HandleRef { raw: vmo },
                                    response_len: result.bytes.len() as u64,
                                }
                            },
                        )
                    }
                    Err(status) => TeeManagerInvokeCommandResponse {
                        status,
                        response: HandleRef { raw: 0 },
                        response_len: 0,
                    },
                },
                Err(_) => TeeManagerInvokeCommandResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    response: HandleRef { raw: 0 },
                    response_len: 0,
                },
            };
            send_response(channel, &response);
        }
        8 => {
            log("teed: update core request\n");
            let response = match TeeManagerUpdateTeeCoreRequest::decode(req, &handles) {
                Ok(request) => match vmo_bytes(request.tee_image.raw, request.tee_image_len) {
                    Ok((mapped, image)) => {
                        let pinned = Memory::pin(request.tee_image.raw)
                            .map_err(|_| TeeStatus::ErrAccessDenied);
                        let result = match pinned {
                            Ok((physical, token)) => {
                                let result = service
                                    .update_core(
                                        request.generation,
                                        request.target,
                                        request.activation,
                                        &request.artifact_hash,
                                        image,
                                        physical,
                                    )
                                    .await;
                                let _ = Memory::unpin(token);
                                result
                            }
                            Err(status) => Err(status),
                        };
                        let _ = mapped.map(|(va, len)| Memory::unmap(va, len));
                        result.map_or_else(
                            |status| TeeManagerUpdateTeeCoreResponse {
                                status,
                                new_version: 0,
                                message: "tee core update failed",
                            },
                            |new_version| TeeManagerUpdateTeeCoreResponse {
                                status: TeeStatus::Ok,
                                new_version,
                                message: "tee core update completed",
                            },
                        )
                    }
                    Err(status) => TeeManagerUpdateTeeCoreResponse {
                        status,
                        new_version: 0,
                        message: "tee core update failed",
                    },
                },
                Err(_) => TeeManagerUpdateTeeCoreResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    new_version: 0,
                    message: "invalid tee core update request",
                },
            };
            log("teed: update core response\n");
            send_response(channel, &response);
        }
        9 => {
            let _ = TeeManagerGetTeeUpdateStatusRequest::decode(req, &handles);
            match service.update_status().await {
                Ok(progress) => send_response(
                    channel,
                    &TeeManagerGetTeeUpdateStatusResponse {
                        status: TeeStatus::Ok,
                        update_status: progress.status,
                        phase: progress.phase,
                        active_slot: &progress.active_slot,
                        pending_slot: &progress.pending_slot,
                        generation: progress.generation,
                        rollback_available: progress.rollback_available,
                        reboot_required: progress.reboot_required,
                        message: &progress.message,
                    },
                ),
                Err(status) => send_response(
                    channel,
                    &TeeManagerGetTeeUpdateStatusResponse {
                        status,
                        update_status: TeeUpdateStatus::Failed,
                        phase: TeeUpdatePhase::Failed,
                        active_slot: "",
                        pending_slot: "",
                        generation: 0,
                        rollback_available: false,
                        reboot_required: false,
                        message: "tee update status unavailable",
                    },
                ),
            }
        }
        10 => {
            let response = match TeeManagerOpenSessionByEndpointRequest::decode(req, &handles) {
                Ok(request) => match service
                    .open_endpoint(request.package_id, request.service_port)
                    .await
                {
                    Ok(session_id) => TeeManagerOpenSessionByEndpointResponse {
                        status: TeeStatus::Ok,
                        session_id,
                    },
                    Err(status) => TeeManagerOpenSessionByEndpointResponse {
                        status,
                        session_id: 0,
                    },
                },
                Err(_) => TeeManagerOpenSessionByEndpointResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    session_id: 0,
                },
            };
            send_response(channel, &response);
        }
        11 => {
            let response = match TeeManagerActivateTrustedAppPackageRequest::decode(req, &handles) {
                Ok(request) => {
                    match vmo_bytes(request.archive_payload.raw, request.archive_payload_len) {
                        Ok((mapped, payload)) => {
                            let mut ports = Vec::new();
                            for index in 0..request.service_ports.len() {
                                match request.service_ports.get(index) {
                                    Ok(port) => ports.push(port.into()),
                                    Err(_) => {
                                        let _ = mapped.map(|(va, len)| Memory::unmap(va, len));
                                        send_response(
                                            channel,
                                            &TeeManagerActivateTrustedAppPackageResponse {
                                                status: TeeStatus::ErrInvalidArgs,
                                                message: "invalid service port vector",
                                            },
                                        );
                                        return Ok(());
                                    }
                                }
                            }
                            let status = service
                                .activate_package_app(
                                    request.package_id,
                                    request.provider,
                                    request.uuid,
                                    request.secure_version,
                                    &ports,
                                    request.protected,
                                    payload,
                                )
                                .await;
                            let _ = mapped.map(|(va, len)| Memory::unmap(va, len));
                            TeeManagerActivateTrustedAppPackageResponse {
                                status,
                                message: if status == TeeStatus::Ok {
                                    "trusted app package active"
                                } else {
                                    "trusted app package activation failed"
                                },
                            }
                        }
                        Err(status) => TeeManagerActivateTrustedAppPackageResponse {
                            status,
                            message: "trusted app package payload unavailable",
                        },
                    }
                }
                Err(_) => TeeManagerActivateTrustedAppPackageResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    message: "invalid trusted app package activation request",
                },
            };
            send_response(channel, &response);
        }
        12 => {
            let response = match TeeManagerDeactivateTrustedAppPackageRequest::decode(req, &handles)
            {
                Ok(request) => {
                    let status = service.deactivate_package_app(request.package_id).await;
                    TeeManagerDeactivateTrustedAppPackageResponse {
                        status,
                        message: if status == TeeStatus::Ok {
                            "trusted app package inactive"
                        } else {
                            "trusted app package deactivation failed"
                        },
                    }
                }
                Err(_) => TeeManagerDeactivateTrustedAppPackageResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    message: "invalid trusted app package deactivation request",
                },
            };
            send_response(channel, &response);
        }
        13 => {
            let response = match TeeManagerQueryTrustedAppPackageRequest::decode(req, &handles) {
                Ok(request) => match service.query_package_app(request.package_id).await {
                    Ok(app) => TeeManagerQueryTrustedAppPackageResponse {
                        status: TeeStatus::Ok,
                        uuid: app.uuid,
                        secure_version: app.version as u64,
                        active_sessions: app.active_sessions,
                    },
                    Err(status) => TeeManagerQueryTrustedAppPackageResponse {
                        status,
                        uuid: [0; 16],
                        secure_version: 0,
                        active_sessions: 0,
                    },
                },
                Err(_) => TeeManagerQueryTrustedAppPackageResponse {
                    status: TeeStatus::ErrInvalidArgs,
                    uuid: [0; 16],
                    secure_version: 0,
                    active_sessions: 0,
                },
            };
            send_response(channel, &response);
        }
        _ => {}
    }
    Ok(())
}

struct ReceivedHandles(Vec<u64>);

impl Drop for ReceivedHandles {
    fn drop(&mut self) {
        for handle in self.0.drain(..) {
            let _ = Memory::close(handle);
        }
    }
}

type MappedBytes = (Option<(u64, u64)>, &'static [u8]);

fn vmo_bytes(handle: u64, len: u64) -> Result<MappedBytes, TeeStatus> {
    if len == 0 {
        return Ok((None, &[]));
    }
    if handle == 0 {
        return Err(TeeStatus::ErrInvalidArgs);
    }
    let map_len = page_round(len).ok_or(TeeStatus::ErrInvalidArgs)?;
    let va = Memory::map(handle, map_len, 2).map_err(|_| TeeStatus::ErrInvalidHandle)?;
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) };
    Ok((Some((va, map_len)), bytes))
}

fn page_round(value: u64) -> Option<u64> {
    value.checked_add(4095).map(|n| n & !4095)
}

fn send_response<T: FidlEncode>(client: Channel, response: &T) {
    let mut out = [0; 4096];
    let mut handles = [HandleRef { raw: 0 }; 4];
    let Ok(encoded) = response.encode(&mut out, &mut handles) else {
        return;
    };
    let raw: Vec<_> = handles[..encoded.handles]
        .iter()
        .map(|handle| handle.raw)
        .collect();
    let _ = client.send(&out[..encoded.bytes], &raw);
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}

fn metadata_protocol(metadata: &str) -> Option<&str> {
    metadata.split('|').nth(1)
}
