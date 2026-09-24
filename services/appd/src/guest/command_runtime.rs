//! Authenticated command launch and retained process-control endpoints.
use super::*;
use crate::KernelOps;
use crate::command_state::{ControlledProcess, LIMIT};
use bexos_starnix_abi::Control;
use bexos_userspace::command::CommandOptions;
use opener_fidl::*;

pub(super) struct CommandLaunch {
    pub options: CommandOptions,
    pub cwd: u64,
    pub stdio: [u64; 3],
}
fn control_status(
    process: u64,
) -> Result<kernel_fidl::SystemPrivilegedGetProcessStatusResponse, kernel_fidl::Status> {
    bexos_userspace::ipc::kernel_call(
        4,
        "GetProcessStatus",
        kernel_fidl::SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
        &kernel_fidl::SystemPrivilegedGetProcessStatusRequest {
            process_handle: kernel_fidl::HandleRef { raw: process },
        },
    )
}
fn set_suspended(process: u64, value: bool) -> bool {
    let h = kernel_fidl::HandleRef { raw: process };
    if value {
        bexos_userspace::ipc::kernel_call::<_, kernel_fidl::SystemPrivilegedSuspendProcessResponse>(
            4,
            "SuspendProcess",
            kernel_fidl::SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
            &kernel_fidl::SystemPrivilegedSuspendProcessRequest { process_handle: h },
        )
        .is_ok_and(|r| r.status == kernel_fidl::Status::Ok)
    } else {
        bexos_userspace::ipc::kernel_call::<_, kernel_fidl::SystemPrivilegedResumeProcessResponse>(
            4,
            "ResumeProcess",
            kernel_fidl::SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
            &kernel_fidl::SystemPrivilegedResumeProcessRequest { process_handle: h },
        )
        .is_ok_and(|r| r.status == kernel_fidl::Status::Ok)
    }
}
fn signal(
    launches: &[state::LaunchRecord],
    registry: &MemoryAppRegistry,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    process: u64,
    signal: u32,
) -> bool {
    let launch = launches
        .iter()
        .find(|launch| launch.process_handle == process);
    let is_nix = launch.is_some_and(|launch| {
        registry
            .record(&launch.package)
            .ok()
            .and_then(|record| Manifest::decode(&record.manifest_bytes).ok())
            .and_then(|manifest| {
                manifest
                    .processes
                    .into_iter()
                    .find(|candidate| candidate.name == launch.process)
            })
            .is_some_and(|candidate| candidate.runner == "nix")
    });
    let deliver =
        |kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>| {
            let Some(launch) = launch else { return false };
            Channel(launch.manager)
                .send(&Control::Signal(signal).encode(), &[])
                .is_ok()
                && kernel
                    .kick_restricted_thread(KernelHandle {
                        raw: launch.thread_handle,
                    })
                    .is_ok()
        };
    match signal {
        18 => set_suspended(process, false) && (!is_nix || deliver(kernel)),
        19 => set_suspended(process, true),
        20 if !is_nix => set_suspended(process, true),
        9 => bexos_userspace::ipc::kernel_call::<
            _,
            kernel_fidl::SystemPrivilegedTerminateProcessResponse,
        >(
            4,
            "TerminateProcess",
            kernel_fidl::SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
            &kernel_fidl::SystemPrivilegedTerminateProcessRequest {
                process_handle: kernel_fidl::HandleRef { raw: process },
                exit_code: 128 + signal as i32,
            },
        )
        .is_ok_and(|r| r.status == kernel_fidl::Status::Ok),
        1..=64 => {
            if !is_nix {
                return matches!(signal, 2 | 15)
                    && bexos_userspace::ipc::kernel_call::<
                        _,
                        kernel_fidl::SystemPrivilegedTerminateProcessResponse,
                    >(
                        4,
                        "TerminateProcess",
                        kernel_fidl::SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
                        &kernel_fidl::SystemPrivilegedTerminateProcessRequest {
                            process_handle: kernel_fidl::HandleRef { raw: process },
                            exit_code: 128 + signal as i32,
                        },
                    )
                    .is_ok_and(|r| r.status == kernel_fidl::Status::Ok);
            }
            deliver(kernel)
        }
        _ => false,
    }
}
pub(super) fn poll(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
) -> bool {
    let mut changed = false;
    let mut closed = Vec::new();
    for binding in state.commands.bindings.clone() {
        // Bound work keeps control traffic responsive under a busy client.
        for _ in 0..8 {
            match Channel(binding.channel).try_recv() {
                Ok(m) => {
                    dispatch(state, kernel, &binding, &m.bytes, &m.handles);
                    changed = true;
                }
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    closed.push(binding.channel);
                    changed = true;
                    break;
                }
                Err(_) => break,
            }
        }
    }
    state
        .commands
        .bindings
        .retain(|b| !closed.contains(&b.channel));
    for h in closed {
        let _ = Memory::close(h);
    }
    let mut reaped = Vec::new();
    let launches = &state.launches;
    let registry = &state.registry;
    state.commands.processes.retain_mut(|p| {
        let status = control_status(p.process)
            .ok()
            .filter(|r| r.status == kernel_fidl::Status::Ok);
        if let Some(status) = &status {
            if status.exited && p.completion.is_none() {
                p.completion = Some(status.exit_code);
                changed = true;
            }
        }
        if p.channel == 0 {
            if p.completion.is_some() {
                reaped.push(p.process);
                return false;
            }
            return true;
        }
        for _ in 0..8 {
            match Channel(p.channel).try_recv() {
                Ok(m) => {
                    changed = true;
                    close_handles(&m.handles);
                    if !m.handles.is_empty() || m.bytes.len() < 8 {
                        continue;
                    }
                    let (ordinal, bytes) = envelope(&m.bytes);
                    let ok = OpenerStatus::Ok;
                    let invalid = OpenerStatus::InvalidArgs;
                    match ordinal {
                        1 => opener_reply(
                            p.channel,
                            &ProcessControlGetStatusResponse {
                                status: if status.is_some() {
                                    ok
                                } else {
                                    OpenerStatus::NotFound
                                },
                                exited: p.completion.is_some(),
                                suspended: status.as_ref().is_some_and(|s| s.suspended),
                                exit_code: p.completion.unwrap_or(0),
                            },
                        ),
                        2 if !p.watch_pending => p.watch_pending = true,
                        3 => {
                            let status = ProcessControlSendSignalRequest::decode(bytes, &[])
                                .ok()
                                .filter(|q| signal(launches, registry, kernel, p.process, q.signal))
                                .map_or(invalid, |_| ok);
                            opener_reply(p.channel, &ProcessControlSendSignalResponse { status });
                        }
                        4 => opener_reply(
                            p.channel,
                            &ProcessControlSuspendResponse {
                                status: if set_suspended(p.process, true) {
                                    ok
                                } else {
                                    invalid
                                },
                            },
                        ),
                        5 => opener_reply(
                            p.channel,
                            &ProcessControlResumeResponse {
                                status: if set_suspended(p.process, false) {
                                    ok
                                } else {
                                    invalid
                                },
                            },
                        ),
                        _ => (),
                    }
                }
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    let _ = Memory::close(p.channel);
                    p.channel = 0;
                    p.watch_pending = false;
                    changed = true;
                    if p.completion.is_some() {
                        reaped.push(p.process);
                        return false;
                    }
                    return true;
                }
                Err(_) => break,
            }
        }
        if p.watch_pending {
            if let Some(code) = p.completion {
                if reply(
                    p.channel,
                    &ProcessControlWatchExitResponse {
                        status: OpenerStatus::Ok,
                        exit_code: code,
                    },
                )
                .is_ok()
                {
                    p.watch_pending = false;
                    changed = true;
                }
            }
        }
        true
    });
    for process in reaped {
        if let Some(i) = state
            .launches
            .iter()
            .position(|l| l.process_handle == process)
        {
            let launch = state.launches.remove(i);
            cleanup_dead_launch(state, &launch);
        }
    }
    changed
}
fn dispatch(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &OpenerBinding,
    bytes: &[u8],
    handles: &[u64],
) {
    if bytes.len() < 8 {
        close_handles(handles);
        return;
    }
    let (ordinal, body) = envelope(bytes);
    let refs = opener_refs(handles);
    match ordinal {
        1 => {
            let resolved = CommandLauncherResolveCommandRequest::decode(body, &refs)
                .map_err(|_| crate::commands::CommandError::InvalidManifest)
                .and_then(|q| {
                    if !handles.is_empty() {
                        return Err(crate::commands::CommandError::InvalidManifest);
                    }
                    crate::commands::catalogue(&state.registry)
                        .and_then(|c| crate::commands::resolve(&c, q.name))
                });
            let response = match &resolved {
                Ok(c) => CommandLauncherResolveCommandResponse {
                    status: OpenerStatus::Ok,
                    package_name: &c.package,
                    process_name: &c.process,
                    executable: &c.executable,
                },
                Err(error) => CommandLauncherResolveCommandResponse {
                    status: if matches!(error, crate::commands::CommandError::NotFound) {
                        OpenerStatus::NotFound
                    } else {
                        OpenerStatus::InvalidArgs
                    },
                    package_name: "",
                    process_name: "",
                    executable: "",
                },
            };
            opener_reply(binding.channel, &response);
        }
        2 => {
            let result = CommandLauncherLaunchCommandRequest::decode(body, &refs)
                .map_err(|_| OpenerStatus::InvalidArgs)
                .and_then(|q| launch(state, kernel, binding, q, handles));
            match result {
                Ok(channel) => {
                    let sent = reply(
                        binding.channel,
                        &CommandLauncherLaunchCommandResponse {
                            status: OpenerStatus::Ok,
                            process_control: opener_fidl::HandleRef { raw: channel },
                        },
                    );
                    // Successful sends move the endpoint. Failed sends retain it.
                    if sent.is_err() {
                        let _ = Memory::close(channel);
                    }
                }
                Err(status) => opener_reply(
                    binding.channel,
                    &CommandLauncherLaunchCommandResponse {
                        status,
                        process_control: opener_fidl::HandleRef { raw: 0 },
                    },
                ),
            }
        }
        3 if handles.is_empty() => {
            let mut records = Vec::new();
            for launch in &state.launches {
                if !crate::commands::can_control(binding.uid, launch.uid) {
                    continue;
                }
                if let Ok(status) = control_status(launch.process_handle) {
                    if status.status == kernel_fidl::Status::Ok {
                        records.push(CommandProcess {
                            process_id: status.process_id,
                            package_name: &launch.package,
                            process_name: &launch.process,
                            exited: status.exited,
                            suspended: status.suspended,
                            exit_code: status.exit_code,
                        });
                    }
                }
            }
            records.sort_by_key(|r| r.process_id);
            let valid = records.len() <= 128;
            if !valid {
                records.clear();
            }
            opener_reply(
                binding.channel,
                &CommandLauncherListProcessesResponse {
                    status: if valid {
                        OpenerStatus::Ok
                    } else {
                        OpenerStatus::LaunchFailed
                    },
                    processes: WireVector::from_slice(&records),
                },
            );
        }
        4 if handles.is_empty() => {
            let result = CommandLauncherSignalProcessRequest::decode(body, &[])
                .ok()
                .and_then(|q| {
                    state
                        .launches
                        .iter()
                        .filter(|l| crate::commands::can_control(binding.uid, l.uid))
                        .find_map(|l| {
                            control_status(l.process_handle)
                                .ok()
                                .filter(|s| {
                                    s.status == kernel_fidl::Status::Ok
                                        && s.process_id == q.process_id
                                })
                                .map(|_| {
                                    signal(
                                        &state.launches,
                                        &state.registry,
                                        kernel,
                                        l.process_handle,
                                        q.signal,
                                    )
                                })
                        })
                });
            opener_reply(
                binding.channel,
                &CommandLauncherSignalProcessResponse {
                    status: match result {
                        Some(true) => OpenerStatus::Ok,
                        Some(false) => OpenerStatus::InvalidArgs,
                        None => OpenerStatus::AccessDenied,
                    },
                },
            );
        }
        _ => (),
    }
    close_handles(handles);
}
fn launch(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &OpenerBinding,
    q: CommandLauncherLaunchCommandRequest<'_>,
    handles: &[u64],
) -> Result<u64, OpenerStatus> {
    if handles.len() != 4 || state.commands.processes.len() >= LIMIT {
        return Err(OpenerStatus::InvalidArgs);
    }
    let expected = [
        q.cwd.raw,
        q.stdin_stream.raw,
        q.stdout_stream.raw,
        q.stderr_stream.raw,
    ];
    if expected
        .iter()
        .enumerate()
        .any(|(i, h)| *h == 0 || !handles.contains(h) || expected[..i].contains(h))
    {
        return Err(OpenerStatus::InvalidArgs);
    }
    let catalogue =
        crate::commands::catalogue(&state.registry).map_err(|_| OpenerStatus::InvalidArgs)?;
    let command = crate::commands::resolve(
        &catalogue,
        &format!("{}:{}", q.package_name, q.command_name),
    )
    .map_err(|_| OpenerStatus::NotFound)?;
    if command.process != q.process_name {
        return Err(OpenerStatus::InvalidArgs);
    }
    let mut options = CommandOptions::default();
    options.arguments.push(q.command_name.into());
    for i in 0..q.arguments.len() {
        options.arguments.push(
            q.arguments
                .get(i)
                .map_err(|_| OpenerStatus::InvalidArgs)?
                .into(),
        );
    }
    for i in 0..q.environment.len() {
        let pair = q
            .environment
            .get(i)
            .map_err(|_| OpenerStatus::InvalidArgs)?;
        let (name, value) = pair.split_once('=').ok_or(OpenerStatus::InvalidArgs)?;
        options.environment.push((name.into(), value.into()));
    }
    options.encode().map_err(|_| OpenerStatus::InvalidArgs)?;
    options.environment.retain(|(name, _)| name != "PWD");
    options.environment.push(("PWD".into(), "/cwd".into()));
    options.encode().map_err(|_| OpenerStatus::InvalidArgs)?;
    for (i, h) in expected.into_iter().enumerate() {
        let (kind, rights) = Memory::object_info(h).map_err(|_| OpenerStatus::AccessDenied)?;
        let required = if i <= 1 { 2 } else { 4 };
        if rights & (required | 1 | 32) != (required | 1 | 32)
            || (i == 0 && kind != kernel_fidl::ObjectType::Channel)
            || (i != 0
                && !matches!(
                    kind,
                    kernel_fidl::ObjectType::Channel | kernel_fidl::ObjectType::Socket
                ))
        {
            return Err(OpenerStatus::AccessDenied);
        }
    }
    let (server, client) = Channel::pair().map_err(|_| OpenerStatus::LaunchFailed)?;
    let spec = CommandLaunch {
        options,
        cwd: q.cwd.raw,
        stdio: [q.stdin_stream.raw, q.stdout_stream.raw, q.stderr_stream.raw],
    };
    let status = launch_application(
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
        &command.package,
        &command.process,
        server.0,
        binding.uid,
        None,
        false,
        None,
        Some(&spec),
        Vec::new(),
        0,
        false,
    );
    if status != lifecycle::AppLifecycleStatus::Ok {
        close_handles(&[server.0, client.0]);
        return Err(OpenerStatus::LaunchFailed);
    }
    let launch = state.launches.last().ok_or(OpenerStatus::LaunchFailed)?;
    let process = launch.process_handle;
    state.commands.processes.push(ControlledProcess {
        channel: server.0,
        process,
        package: command.package,
        uid: binding.uid,
        watch_pending: false,
        completion: None,
    });
    Ok(client.0)
}

// Keep an exit notification pending until the channel accepts it. A full receive
// queue is transient; dropping the pending bit would lose a completion on transplant.
fn reply<Q: OpenerEncode>(channel: u64, q: &Q) -> Result<(), kernel_fidl::Status> {
    let mut bytes = alloc::vec![0; 65500];
    let mut handles = [opener_fidl::HandleRef { raw: 0 }; 16];
    let encoded = q
        .encode(&mut bytes, &mut handles)
        .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
    Channel(channel).send(
        &bytes[..encoded.bytes],
        &handles[..encoded.handles]
            .iter()
            .map(|h| h.raw)
            .collect::<Vec<_>>(),
    )
}
