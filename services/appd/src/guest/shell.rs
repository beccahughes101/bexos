//! Graphical session broker. All UI authority is selected here, never by guests.
use super::shell_users::call as user_call;
use super::*;
use crate::manifest::ShellRole;
use shell_session_fidl as wire;
use wire::{FidlDecode, FidlEncode};
pub const KEY: u64 = 15;
type Kernel = KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>;

fn launch(
    state: &mut state::AppdState,
    kernel: &mut Kernel,
    package: &str,
    role: ShellRole,
    uid: u64,
) -> bool {
    let process = state
        .registry
        .record(package)
        .ok()
        .and_then(|r| Manifest::decode(&r.manifest_bytes).ok())
        .and_then(|m| crate::shell::entrypoint(&m, role).map(|p| p.name.clone()));
    let Some(process) = process else { return false };
    launch_application(
        &mut state.registry,
        &mut state.launches,
        &mut state.services,
        state.vfsd,
        state.users,
        kernel,
        &mut state.broker,
        &mut state.permission_routes,
        &mut state.permissions,
        &mut state.opener_bindings,
        &mut state.version_manager_bindings,
        &mut state.app_manager_bindings,
        &mut state.worker_launcher_bindings,
        &mut state.service_directory_bindings,
        &mut state.lazy,
        &state.domain_associations,
        &state.config,
        &state.component_configs,
        package,
        &process,
        // Internal shell launches bind UserUI's compositor grant to this login.
        state.shell.epoch,
        uid,
        None,
        false,
        None,
        Vec::new(),
        0,
        true,
    ) == lifecycle::AppLifecycleStatus::Ok
}
fn diagnostic(state: &mut state::AppdState, text: &str) {
    state.shell.diagnostic = text.chars().take(256).collect();
    log(&format!("appd: shell: {}\n", state.shell.diagnostic));
}
fn configure(state: &state::AppdState) -> bool {
    let Some(scene) = state
        .services
        .iter()
        .find(|s| s.package == "bexos.service.scened")
    else {
        return false;
    };
    let Ok((client, server)) = Channel::pair() else {
        return false;
    };
    let mut w = bexos_migration::codec::Encoder::new();
    w.text(&state.shell.sysui);
    w.text(&state.shell.userui);
    w.word(state.shell.uid);
    w.word((state.shell.locked || state.shell.uid == 0) as u64);
    w.word(state.shell.epoch);
    let mut bytes = b"bexos.shell.configure\0".to_vec();
    bytes.extend(w.finish());
    let sent = Channel(scene.manager).send(&bytes, &[server.0]).is_ok();
    let response = if sent {
        client.recv_with_timeout(5).ok()
    } else {
        let _ = Memory::close(server.0);
        None
    };
    let _ = Memory::close(client.0);
    let ok = response.is_some_and(|m| m.bytes.get(..4) == Some(&0i32.to_le_bytes()));
    if ok {
        notify_state(state);
    }
    ok
}
fn running(state: &state::AppdState, package: &str, uid: u64) -> bool {
    state
        .launches
        .iter()
        .any(|l| l.package == package && l.uid == uid)
}
fn select(
    state: &mut state::AppdState,
    kernel: &mut Kernel,
    selected: String,
    role: ShellRole,
    uid: u64,
) -> bool {
    let fallback = if role == ShellRole::System {
        crate::shell::SYSTEM_PACKAGE
    } else {
        crate::shell::USER_PACKAGE
    };
    for package in [selected.as_str(), fallback] {
        if role == ShellRole::System {
            state.shell.sysui = package.into();
        } else {
            state.shell.userui = package.into();
        }
        if !configure(state) {
            diagnostic(state, "Compositor session configuration failed.");
            return false;
        }
        if launch(state, kernel, package, role, uid) {
            return true;
        }
        diagnostic(
            state,
            &format!("Unable to launch {package}; trying bundled shell."),
        );
        if package == fallback {
            break;
        }
    }
    diagnostic(
        state,
        "Bundled shell failed to start; use the configuration CLI to recover.",
    );
    false
}
pub fn poll(state: &mut state::AppdState, kernel: &mut Kernel, source: &mut Source) {
    if cfg!(standalone_graphics) {
        return;
    }

    if !state
        .services
        .iter()
        .any(|s| s.package == "bexos.service.scened")
        || !state
            .services
            .iter()
            .any(|s| s.package == preferences::PACKAGE)
    {
        return;
    }
    let before = state.shell.encode();
    let dead = state
        .launches
        .iter()
        .filter(|l| {
            state.shell.authorized(&l.package, l.uid) && process_terminated(l.process_handle)
        })
        .cloned()
        .collect::<Vec<_>>();
    let reaped = !dead.is_empty();
    for l in dead {
        state
            .launches
            .retain(|v| v.process_handle != l.process_handle);
        cleanup_dead_launch(state, &l);
    }
    let now = bexos_userspace::syscall::ticks();
    if !state.shell.initialized {
        if now < state.shell.retry_at {
            return;
        }
        log("appd: resolving system shell selection\n");
        let selected = if state.shell.sysui.is_empty() {
            preferences::shell_selection(state, 0, "sysui_package")
        } else {
            Ok(state.shell.sysui.clone())
        };
        match selected {
            Ok(selected) => {
                state.shell.initialized = true;
                state.shell.locked = true;
                select(state, kernel, selected, ShellRole::System, 0);
            }
            Err(e) => diagnostic(
                state,
                &format!("Waiting for system shell preferences: {e:?}."),
            ),
        }
        state.shell.retry_at = now + bexos_userspace::syscall::frequency() * 5;
    } else if now >= state.shell.retry_at && !running(state, &state.shell.sysui, 0) {
        state.shell.locked = true;
        let _ = configure(state);
        if state.shell.uid != 0 {
            let uid = state.shell.uid;
            let _ = user_call(state, 7, &user_manager::UserManagerLockUserRequest { uid });
        }
        let package = state.shell.sysui.clone();
        select(state, kernel, package, ShellRole::System, 0);
        state.shell.retry_at = now + bexos_userspace::syscall::frequency() * 5;
    } else if state.shell.uid != 0
        // A locked UID cannot launch processes. Keep the selected desktop so
        // authentication can recover it instead of treating that denial as an
        // invalid package and replacing the selection with the bundled shell.
        && !state.shell.locked
        && !running(state, &state.shell.userui, state.shell.uid)
        && !state.shell.userui.is_empty()
        && now >= state.shell.retry_at
    {
        let was_locked = state.shell.locked;
        state.shell.locked = true;
        let _ = configure(state);
        let package = state.shell.userui.clone();
        let uid = state.shell.uid;
        if select(state, kernel, package, ShellRole::User, uid) {
            state.shell.locked = was_locked;
            let _ = configure(state);
        }
        state.shell.retry_at = now + bexos_userspace::syscall::frequency() * 5;
    }
    record_changes(state, source, reaped || before != state.shell.encode());
}

/// A guest must be able to finish an outstanding state query to reach its
/// checkpoint. Keep these requests flowing while a replacement is staged,
/// without allowing login, logout, or process changes during the transfer.
pub fn poll_requests(
    state: &mut state::AppdState,
    kernel: &mut Kernel,
    source: &mut Source,
    session_changes: bool,
) {
    let before = state.shell.encode();
    for c in state.shell.clients.clone() {
        if c.callback != 0 {
            while let Ok(m) = Channel(c.callback).try_recv() {
                close_handles(&m.handles);
            }
        }
        match Channel(c.channel).try_recv() {
            Ok(mut message) => {
                handle(
                    state,
                    kernel,
                    &c,
                    &message.bytes,
                    &message.handles,
                    session_changes,
                );
                message.bytes.fill(0);
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                state.shell.clients.retain(|v| v.channel != c.channel);
                let _ = Memory::close(c.channel);
                if c.callback != 0 {
                    let _ = Memory::close(c.callback);
                }
            }
            Err(_) => {}
        }
    }
    record_changes(state, source, before != state.shell.encode());
}

fn record_changes(state: &state::AppdState, source: &mut Source, changed: bool) {
    if changed {
        source.changed(KEY);
        source.changed(0);
        source.changed_keys((0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64));
        source.changed_keys((0..state.services.len()).map(|i| state::SERVICE_BASE + i as u64));
        source.changed(state::SERVICE_DIRECTORY_STATE_KEY);
    }
}
fn user_status(message: Option<bexos_userspace::Message>) -> bool {
    message.is_some_and(|m| m.bytes.get(..4) == Some(&0i32.to_le_bytes()))
}
fn reply(channel: u64, response: &impl FidlEncode) {
    let mut bytes = vec![0; 32768];
    if let Ok(e) = response.encode(&mut bytes, &mut []) {
        let _ = Channel(channel).send(&bytes[..e.bytes], &[]);
    }
}
fn close_processes(
    state: &mut state::AppdState,
    kernel: &mut Kernel,
    uid: u64,
    package: Option<&str>,
) {
    // Graphical logout must never terminate system services.
    if uid == 0 {
        return;
    }
    let launches = state
        .launches
        .iter()
        .filter(|l| l.uid == uid && package.is_none_or(|p| p == l.package))
        .cloned()
        .collect::<Vec<_>>();
    for l in launches {
        state
            .launches
            .retain(|v| v.process_handle != l.process_handle);
        state
            .watchdogs
            .retain(|v| v.package_id != l.package || v.process_name != l.process || v.uid != l.uid);
        let _ = crate::runner::KernelOps::terminate_process(
            kernel,
            KernelHandle {
                raw: l.process_handle,
            },
            0,
        );
        cleanup_dead_launch(state, &l);
    }
}
fn lock(state: &mut state::AppdState) -> wire::Status {
    state.shell.locked = true;
    if !configure(state) {
        return wire::Status::Failed;
    }
    let uid = state.shell.uid;
    if uid != 0
        && !user_status(user_call(
            state,
            7,
            &user_manager::UserManagerLockUserRequest { uid },
        ))
    {
        return wire::Status::Failed;
    }
    wire::Status::Ok
}
fn handle(
    state: &mut state::AppdState,
    kernel: &mut Kernel,
    client: &crate::shell::Client,
    bytes: &[u8],
    handles: &[u64],
    session_changes: bool,
) {
    if bytes.len() < 8 {
        close_handles(handles);
        return;
    }
    let (ordinal, request) = envelope(bytes);
    let allowed = state.shell.authorized(&client.package, client.uid)
        && (client.uid == 0 || client.epoch == state.shell.epoch);
    if !allowed {
        close_handles(handles);
        reply(
            client.channel,
            &wire::SessionManagerLoginResponse {
                status: wire::Status::AccessDenied,
            },
        );
        return;
    }
    if !session_changes && ordinal != 1 {
        close_handles(handles);
        reply(
            client.channel,
            &wire::SessionManagerLoginResponse {
                status: wire::Status::Busy,
            },
        );
        return;
    }
    if ordinal == 7 || ordinal == 8 {
        let hs = handles
            .iter()
            .map(|h| wire::HandleRef { raw: *h })
            .collect::<Vec<_>>();
        let status = if ordinal == 7 && client.uid != 0 {
            match wire::SessionManagerRegisterUserShellRequest::decode(request, &hs) {
                Ok(q) if handles.len() == 1 => {
                    if let Some(c) = state
                        .shell
                        .clients
                        .iter_mut()
                        .find(|c| c.channel == client.channel)
                    {
                        if c.callback != 0 {
                            let _ = Memory::close(c.callback);
                        }
                        c.callback = q.endpoint.raw;
                    }
                    notify_state(state);
                    wire::Status::Ok
                }
                _ => {
                    close_handles(handles);
                    wire::Status::InvalidArgs
                }
            }
        } else if ordinal == 8 && client.uid == 0 {
            match wire::SessionManagerAttachUserDisplayRequest::decode(request, &hs) {
                Ok(q) if handles.len() == 1 && q.display_id == 1 => {
                    let target = state.shell.clients.iter().find(|c| {
                        c.uid == state.shell.uid
                            && c.uid != 0
                            && c.epoch == state.shell.epoch
                            && c.callback != 0
                    });
                    if let Some(c) = target {
                        if send_callback(
                            c.callback,
                            1,
                            &wire::UserShellAttachDisplayViewRequest {
                                display_id: q.display_id,
                                view_token: q.view_token,
                            },
                        ) {
                            wire::Status::Ok
                        } else {
                            close_handles(handles);
                            wire::Status::Failed
                        }
                    } else {
                        close_handles(handles);
                        wire::Status::Busy
                    }
                }
                _ => {
                    close_handles(handles);
                    wire::Status::InvalidArgs
                }
            }
        } else {
            close_handles(handles);
            wire::Status::AccessDenied
        };
        reply(
            client.channel,
            &wire::SessionManagerLoginResponse { status },
        );
        return;
    }
    if !handles.is_empty() {
        close_handles(handles);
        return;
    }
    if ordinal == 1 {
        let mut users = Vec::new();
        if client.uid == 0 {
            if let Some(m) = user_call(state, 1, &user_manager::UserManagerListUsersRequest {}) {
                if let Ok(r) = user_manager::UserManagerListUsersResponse::decode(&m.bytes, &[]) {
                    for i in 0..r.users.len().min(64) {
                        if let Ok(u) = r.users.get(i) {
                            if !u.disabled && u.uid != 0 {
                                users.push((
                                    u.uid,
                                    u.display_name.chars().take(64).collect::<String>(),
                                ));
                            }
                        }
                    }
                }
            }
        }
        let users = users
            .iter()
            .map(|(uid, name)| wire::User { uid: *uid, name })
            .collect::<Vec<_>>();
        let mut apps = Vec::new();
        if client.uid != 0 && !state.shell.locked {
            for r in state.registry.list_packages() {
                let Ok(m) = Manifest::decode(&r.manifest_bytes) else {
                    continue;
                };
                if m.processes.iter().any(|p| p.shell_role != ShellRole::None)
                    || !m
                        .services_consumed
                        .iter()
                        .any(|c| c.name == "bexos.ui.scened.FlatlandSession")
                {
                    continue;
                }
                apps.push((
                    r.package_id.clone(),
                    m.name.chars().take(64).collect::<String>(),
                ));
                if apps.len() == 64 {
                    break;
                }
            }
        }
        let apps = apps
            .iter()
            .map(|(package_id, name)| wire::App { package_id, name })
            .collect::<Vec<_>>();
        reply(
            client.channel,
            &wire::SessionManagerGetStateResponse {
                status: wire::Status::Ok,
                state: if state.shell.uid == 0 {
                    wire::SessionState::Login
                } else if state.shell.locked {
                    wire::SessionState::Locked
                } else {
                    wire::SessionState::ActiveFocused
                },
                uid: state.shell.uid,
                diagnostic: &state.shell.diagnostic,
                users: wire::WireVector::from_slice(&users),
                apps: wire::WireVector::from_slice(&apps),
            },
        );
        return;
    }
    let status = match ordinal {
        2 if client.uid == 0 => match wire::SessionManagerLoginRequest::decode(request, &[]) {
            Ok(q) if q.uid != 0 && (state.shell.uid == 0 || state.shell.uid == q.uid) => {
                if !user_status(user_call(
                    state,
                    6,
                    &user_manager::UserManagerUnlockUserRequest {
                        uid: q.uid,
                        password: q.password,
                    },
                )) {
                    wire::Status::AccessDenied
                } else {
                    let fresh = match state.shell.authenticated(q.uid) {
                        Ok(fresh) => fresh,
                        Err(_) => {
                            let _ = lock(state);
                            reply(
                                client.channel,
                                &wire::SessionManagerLoginResponse {
                                    status: wire::Status::Failed,
                                },
                            );
                            return;
                        }
                    };
                    let selected = if fresh {
                        preferences::shell_selection(state, q.uid, "userui_package").ok()
                    } else {
                        Some(state.shell.userui.clone())
                    };
                    let launched = selected.is_some_and(|p| {
                        running(state, &p, q.uid)
                            || select(state, kernel, p, ShellRole::User, q.uid)
                    });
                    if launched {
                        state.shell.locked = false;
                        if configure(state) {
                            wire::Status::Ok
                        } else {
                            state.shell.locked = true;
                            wire::Status::Failed
                        }
                    } else {
                        let _ = lock(state);
                        if fresh {
                            // No desktop was started. Retry preference resolution
                            // on the next authentication, without a phantom session.
                            state.shell.end();
                            let _ = configure(state);
                        }
                        wire::Status::Failed
                    }
                }
            }
            _ => wire::Status::AccessDenied,
        },
        3 => lock(state),
        4 | 5 => {
            let status = lock(state);
            if status == wire::Status::Ok {
                close_processes(state, kernel, state.shell.uid, None);
                state.shell.end();
                let _ = configure(state);
            }
            status
        }
        6 if client.uid != 0 && !state.shell.locked => {
            match wire::SessionManagerCloseAppRequest::decode(request, &[]) {
                Ok(q) if q.package_id != state.shell.userui => {
                    close_processes(state, kernel, client.uid, Some(q.package_id));
                    wire::Status::Ok
                }
                _ => wire::Status::AccessDenied,
            }
        }
        _ => wire::Status::AccessDenied,
    };
    reply(
        client.channel,
        &wire::SessionManagerLoginResponse { status },
    );
}

/// Usersd is authoritative even for locks initiated through the CLI.
pub fn user_locked(state: &mut state::AppdState, uid: u64) {
    if uid != 0 && uid == state.shell.uid {
        state.shell.locked = true;
        let _ = configure(state);
    }
}

fn send_callback(channel: u64, ordinal: u64, request: &impl FidlEncode) -> bool {
    let mut bytes = [0; 128];
    let mut hs = [wire::HandleRef { raw: 0 }; 1];
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    let Ok(e) = request.encode(&mut bytes[8..], &mut hs) else {
        return false;
    };
    Channel(channel)
        .send(
            &bytes[..8 + e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .is_ok()
}
fn notify_state(state: &state::AppdState) {
    let phase = if state.shell.uid == 0 {
        wire::SessionState::Login
    } else if state.shell.locked {
        wire::SessionState::Locked
    } else {
        wire::SessionState::ActiveFocused
    };
    for c in &state.shell.clients {
        if c.callback != 0 && c.uid == state.shell.uid && c.epoch == state.shell.epoch {
            let _ = send_callback(
                c.callback,
                2,
                &wire::UserShellSetSessionStateRequest { state: phase },
            );
        }
    }
}
