//! Authenticated command launch and retained process-control endpoints.
use super::*;
use crate::KernelOps;
use crate::command_state::{ControlledProcess, LIMIT};
use bexos_starnix_abi::Control;
use bexos_userspace::command::CommandOptions;
use opener_fidl::*;

pub(super) struct CommandLaunch {
    pub options: CommandOptions,
    pub cwd: Option<u64>,
    pub stdio: [u64; 3],
}

pub(super) struct ContainerLaunch {
    pub rootfs: u64,
    pub options: bexos_starnix_abi::NixRunnerOptions,
    pub resource_group_id: u32,
    pub network_grants: NetworkGrants,
}

pub(super) struct NetworkGrants(pub Vec<ServiceGrant>);

impl Drop for NetworkGrants {
    fn drop(&mut self) {
        close_network_grants(&self.0);
    }
}

#[derive(Clone)]
struct OciNetworkIdentity {
    registry_host: String,
    repository: String,
    manifest_digest: [u8; 32],
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
                reaped.push((p.process, p.resource_group));
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
                        reaped.push((p.process, p.resource_group));
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
    for (process, resource_group) in reaped {
        if let Some(i) = state
            .launches
            .iter()
            .position(|l| l.process_handle == process)
        {
            let launch = state.launches.remove(i);
            cleanup_dead_launch(state, &launch);
        }
        if resource_group != 0 {
            let _ = Memory::close(resource_group);
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
        5 => {
            let result = CommandLauncherLaunchContainerRequest::decode(body, &refs)
                .map_err(|_| OpenerStatus::InvalidArgs)
                .and_then(|q| {
                    launch_container(state, kernel, binding, q, Vec::new(), None, handles)
                });
            match result {
                Ok(channel) => {
                    let sent = reply(
                        binding.channel,
                        &CommandLauncherLaunchContainerResponse {
                            status: OpenerStatus::Ok,
                            process_control: opener_fidl::HandleRef { raw: channel },
                        },
                    );
                    if sent.is_err() {
                        let _ = Memory::close(channel);
                    }
                }
                Err(status) => opener_reply(
                    binding.channel,
                    &CommandLauncherLaunchContainerResponse {
                        status,
                        process_control: opener_fidl::HandleRef { raw: 0 },
                    },
                ),
            }
        }
        6 => {
            let result = CommandLauncherLaunchContainerWithNetworkRequest::decode(body, &refs)
                .map_err(|_| OpenerStatus::InvalidArgs)
                .and_then(|q| {
                    let mut attachments = Vec::new();
                    for index in 0..q.network_attachments.len() {
                        let attachment = q
                            .network_attachments
                            .get(index)
                            .map_err(|_| OpenerStatus::InvalidArgs)?;
                        attachments.push(bexos_starnix_abi::NetworkAttachment {
                            profile: attachment.profile.into(),
                            interface_name: attachment.interface_name.into(),
                        });
                    }
                    if attachments.is_empty() {
                        return Err(OpenerStatus::InvalidArgs);
                    }
                    for attachment in &attachments {
                        if state
                            .config
                            .network_policy
                            .authorize_oci_profile(
                                &attachment.profile,
                                q.registry_host,
                                q.repository,
                                &q.manifest_digest,
                            )
                            .is_none()
                        {
                            return Err(OpenerStatus::AccessDenied);
                        }
                    }
                    let legacy = CommandLauncherLaunchContainerRequest {
                        container_id: q.container_id,
                        rootfs: q.rootfs,
                        executable: q.executable,
                        arguments: q.arguments,
                        environment: q.environment,
                        working_directory: q.working_directory,
                        uid: q.uid,
                        gid: q.gid,
                        hostname: q.hostname,
                        readonly_rootfs: q.readonly_rootfs,
                        resources: q.resources,
                        stdin_stream: q.stdin_stream,
                        stdout_stream: q.stdout_stream,
                        stderr_stream: q.stderr_stream,
                    };
                    launch_container(
                        state,
                        kernel,
                        binding,
                        legacy,
                        attachments,
                        Some(OciNetworkIdentity {
                            registry_host: q.registry_host.into(),
                            repository: q.repository.into(),
                            manifest_digest: q.manifest_digest,
                        }),
                        handles,
                    )
                });
            match result {
                Ok(channel) => {
                    let sent = reply(
                        binding.channel,
                        &CommandLauncherLaunchContainerWithNetworkResponse {
                            status: OpenerStatus::Ok,
                            process_control: opener_fidl::HandleRef { raw: channel },
                        },
                    );
                    if sent.is_err() {
                        let _ = Memory::close(channel);
                    }
                }
                Err(status) => opener_reply(
                    binding.channel,
                    &CommandLauncherLaunchContainerWithNetworkResponse {
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
        cwd: Some(q.cwd.raw),
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
        None,
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
        resource_group: 0,
    });
    Ok(client.0)
}

fn launch_container(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &OpenerBinding,
    q: CommandLauncherLaunchContainerRequest<'_>,
    network_attachments: Vec<bexos_starnix_abi::NetworkAttachment>,
    network_identity: Option<OciNetworkIdentity>,
    handles: &[u64],
) -> Result<u64, OpenerStatus> {
    const PACKAGE: &str = "bexos.service.containerd";
    const PROCESS: &str = "container";
    if binding.package != PACKAGE
        || binding.uid != 0
        || handles.len() != 4
        || state.commands.processes.len() >= LIMIT
        || q.container_id.is_empty()
        || q.arguments.len() > 64
        || q.environment.len() > 64
    {
        return Err(OpenerStatus::AccessDenied);
    }
    let expected = [
        q.rootfs.raw,
        q.stdin_stream.raw,
        q.stdout_stream.raw,
        q.stderr_stream.raw,
    ];
    if expected.iter().enumerate().any(|(index, handle)| {
        *handle == 0 || !handles.contains(handle) || expected[..index].contains(handle)
    }) {
        return Err(OpenerStatus::InvalidArgs);
    }
    for (index, handle) in expected.iter().copied().enumerate() {
        let (kind, rights) = Memory::object_info(handle).map_err(|_| OpenerStatus::AccessDenied)?;
        let required = if index <= 1 { 2 } else { 4 };
        if rights & (required | 1 | 32) != (required | 1 | 32)
            || (index == 0 && kind != kernel_fidl::ObjectType::Channel)
            || (index != 0
                && !matches!(
                    kind,
                    kernel_fidl::ObjectType::Channel | kernel_fidl::ObjectType::Socket
                ))
        {
            return Err(OpenerStatus::AccessDenied);
        }
    }
    let mut arguments = Vec::new();
    for index in 0..q.arguments.len() {
        arguments.push(
            q.arguments
                .get(index)
                .map_err(|_| OpenerStatus::InvalidArgs)?
                .to_string(),
        );
    }
    if arguments.is_empty() {
        arguments.push(q.executable.to_string());
    }
    let mut environment = Vec::new();
    let mut command_environment = Vec::new();
    for index in 0..q.environment.len() {
        let pair = q
            .environment
            .get(index)
            .map_err(|_| OpenerStatus::InvalidArgs)?;
        let (name, value) = pair.split_once('=').ok_or(OpenerStatus::InvalidArgs)?;
        environment.push(bexos_starnix_abi::Environment {
            name: name.into(),
            value: value.into(),
        });
        command_environment.push((name.into(), value.into()));
    }
    let mut resource_limits = Vec::new();
    if q.resources.process_limit != 0 {
        resource_limits.push(bexos_starnix_abi::NixResourceLimit {
            resource: 6,
            soft: u64::from(q.resources.process_limit),
            hard: u64::from(q.resources.process_limit),
        });
    }
    if q.resources.memory_limit_bytes != 0 {
        resource_limits.push(bexos_starnix_abi::NixResourceLimit {
            resource: 9,
            soft: q.resources.memory_limit_bytes,
            hard: q.resources.memory_limit_bytes,
        });
    }
    let network_grants = if network_attachments.is_empty() {
        Vec::new()
    } else {
        provision_container_networks(
            state,
            kernel,
            &network_attachments,
            network_identity
                .as_ref()
                .ok_or(OpenerStatus::AccessDenied)?,
        )?
    };
    let options = bexos_starnix_abi::NixRunnerOptions {
        path: q.executable.into(),
        arguments: arguments.clone(),
        environment,
        rootfs: bexos_starnix_abi::NixRootFilesystem {
            source: bexos_starnix_abi::NixRootSource::Data,
            subpath: String::new(),
            readonly: q.readonly_rootfs,
        },
        working_directory: q.working_directory.into(),
        uid: q.uid,
        gid: q.gid,
        umask: 0o022,
        resource_limits,
        hostname: q.hostname.into(),
        network_attachments,
    };
    options.validate().map_err(|_| OpenerStatus::InvalidArgs)?;
    let command_options = CommandOptions {
        arguments,
        environment: command_environment,
    };
    command_options
        .encode()
        .map_err(|_| OpenerStatus::InvalidArgs)?;

    let parent = kernel
        .open_resource_group("apps")
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let suffix: String = q
        .container_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .take(20)
        .collect();
    let group = kernel.create_resource_group_v2(
        &format!("ctr-{suffix}"),
        parent.handle,
        crate::runner::ResourceGroupLimits {
            cpu_weight: q.resources.cpu_shares.max(1),
            max_cpu_utilization_permille: 0,
            allow_realtime: false,
            memory_low_watermark_bytes: 0,
            memory_high_watermark_bytes: q.resources.memory_limit_bytes,
            max_render_budget_percent: 0,
            max_vram_bytes: 0,
        },
    );
    let _ = kernel.close_handle(parent.handle);
    let group = group.map_err(|_| OpenerStatus::LaunchFailed)?;
    let (server, client) = match Channel::pair() {
        Ok(pair) => pair,
        Err(_) => {
            let _ = kernel.close_handle(group.handle);
            return Err(OpenerStatus::LaunchFailed);
        }
    };
    let command = CommandLaunch {
        options: command_options,
        cwd: None,
        stdio: [q.stdin_stream.raw, q.stdout_stream.raw, q.stderr_stream.raw],
    };
    let container = ContainerLaunch {
        rootfs: q.rootfs.raw,
        options,
        resource_group_id: group.id,
        network_grants: NetworkGrants(network_grants),
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
        PACKAGE,
        PROCESS,
        server.0,
        0,
        None,
        false,
        None,
        Some(&command),
        Vec::new(),
        0,
        false,
        Some(&container),
    );
    if status != lifecycle::AppLifecycleStatus::Ok {
        close_handles(&[server.0, client.0]);
        let _ = kernel.close_handle(group.handle);
        return Err(OpenerStatus::LaunchFailed);
    }
    let Some(process) = state.launches.last().map(|launch| launch.process_handle) else {
        close_handles(&[server.0, client.0]);
        let _ = kernel.close_handle(group.handle);
        return Err(OpenerStatus::LaunchFailed);
    };
    state.commands.processes.push(ControlledProcess {
        channel: server.0,
        process,
        package: PACKAGE.into(),
        uid: 0,
        watch_pending: false,
        completion: None,
        resource_group: group.handle.raw,
    });
    Ok(client.0)
}

fn provision_container_networks(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    attachments: &[bexos_starnix_abi::NetworkAttachment],
    identity: &OciNetworkIdentity,
) -> Result<Vec<ServiceGrant>, OpenerStatus> {
    provision_networks(
        &state.config,
        &mut state.broker,
        kernel,
        attachments,
        WorkloadIdentity::Oci(identity),
        "bexos.service.containerd",
        0,
    )
}

enum WorkloadIdentity<'a> {
    Oci(&'a OciNetworkIdentity),
    Package {
        package_id: &'a str,
        signer: &'a str,
    },
}

pub(super) fn provision_package_networks(
    config: &crate::platform_config::PlatformConfig,
    broker: &mut crate::broker::AppdBroker,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    attachments: &[bexos_starnix_abi::NetworkAttachment],
    package_id: &str,
    signer: &str,
    caller_uid: u64,
) -> Result<Vec<ServiceGrant>, OpenerStatus> {
    provision_networks(
        config,
        broker,
        kernel,
        attachments,
        WorkloadIdentity::Package { package_id, signer },
        package_id,
        caller_uid,
    )
}

fn provision_networks(
    config: &crate::platform_config::PlatformConfig,
    broker: &mut crate::broker::AppdBroker,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    attachments: &[bexos_starnix_abi::NetworkAttachment],
    identity: WorkloadIdentity<'_>,
    caller_package: &str,
    caller_uid: u64,
) -> Result<Vec<ServiceGrant>, OpenerStatus> {
    // Own every provisioned endpoint until the complete attachment set has
    // succeeded. Any early return (authorization, bind, RPC, or metadata)
    // drops the guard and closes both the data endpoint and lifetime lease.
    let mut grants = NetworkGrants(Vec::new());
    for attachment in attachments {
        let profile = match identity {
            WorkloadIdentity::Oci(identity) => config.network_policy.authorize_oci_profile(
                &attachment.profile,
                &identity.registry_host,
                &identity.repository,
                &identity.manifest_digest,
            ),
            WorkloadIdentity::Package { package_id, signer } => config
                .network_policy
                .authorize_package_profile(&attachment.profile, package_id, signer),
        }
        .cloned()
        .ok_or(OpenerStatus::AccessDenied)?;
        let instance_id = match profile.mode {
            crate::platform_config::WorkloadNetworkMode::DirectProvider => config
                .network_policy
                .domain(&profile.domain)
                .map(|domain| domain.isolation_group.as_str()),
            crate::platform_config::WorkloadNetworkMode::VirtualL2 => {
                Some(profile.isolation_group.as_str())
            }
            crate::platform_config::WorkloadNetworkMode::Unspecified => None,
        }
        .ok_or(OpenerStatus::AccessDenied)?;
        let networkd_package = config
            .network_policy
            .isolation_groups
            .iter()
            .find(|group| group.name == instance_id)
            .map(|group| group.networkd_package.clone())
            .ok_or(OpenerStatus::AccessDenied)?;
        let instance_id = instance_id.to_string();
        let binding = bind_network_controller(broker, kernel, &networkd_package, &instance_id)?;
        let mut client = net_fidl::WorkloadNetworkControllerPublicClient::new(
            bexos_userspace::Rpc(Channel(binding.client_endpoint.object_id)),
        );
        let (package_id, signer, registry_host, repository, digest): (
            &str,
            &str,
            &str,
            &str,
            &[u8],
        ) = match identity {
            WorkloadIdentity::Oci(identity) => (
                "",
                "",
                &identity.registry_host,
                &identity.repository,
                &identity.manifest_digest,
            ),
            WorkloadIdentity::Package { package_id, signer } => (package_id, signer, "", "", &[]),
        };
        let request = net_fidl::WorkloadNetworkControllerProvisionRequest {
            package_id,
            signer,
            registry_host,
            repository,
            manifest_digest: digest,
            attachment: net_fidl::WorkloadNetworkAttachment {
                profile: &attachment.profile,
                interface_name: &attachment.interface_name,
            },
        };
        let mut request_bytes = alloc::vec![0; 4096];
        let mut request_handles = [net_fidl::HandleRef { raw: 0 }; 1];
        let mut response_bytes = alloc::vec![0; 4096];
        let mut response_handles = [net_fidl::HandleRef { raw: 0 }; 4];
        let response = client.provision(
            &request,
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        );
        super::close_bound_capabilities(core::slice::from_ref(&binding));
        let response = match response {
            Ok(response) if response.status == net_fidl::Status::Ok => response,
            Ok(response) => {
                close_workload_endpoint(&response.endpoint);
                return Err(network_status(response.status));
            }
            Err(_) => return Err(OpenerStatus::LaunchFailed),
        };
        let endpoint = response.endpoint;
        let (service, protocol, endpoint_handle) = match endpoint.mode {
            net_fidl::WorkloadNetworkMode::DirectProvider => (
                "bexos.net.WorkloadNetworkProvider",
                "SocketProvider",
                endpoint.provider.raw,
            ),
            net_fidl::WorkloadNetworkMode::VirtualL2 => (
                "bexos.net.WorkloadNetworkDevice",
                "EthernetDevice",
                endpoint.device.raw,
            ),
        };
        if endpoint_handle == 0 || endpoint.lease.raw == 0 {
            close_workload_endpoint(&endpoint);
            return Err(OpenerStatus::LaunchFailed);
        }
        let metadata = match endpoint_metadata(&attachment.interface_name, &endpoint) {
            Ok(metadata) => metadata,
            Err(error) => {
                close_workload_endpoint(&endpoint);
                return Err(error);
            }
        };
        grants.0.push(ServiceGrant {
            service: service.into(),
            protocol: protocol.into(),
            capability: "WorkloadNetwork".into(),
            method_ordinals: if endpoint.mode == net_fidl::WorkloadNetworkMode::DirectProvider {
                (1..=7).collect()
            } else {
                (1..=6).collect()
            },
            permission_values: vec![metadata],
            caller_package: Some(caller_package.into()),
            caller_uid: Some(caller_uid),
            caller_foreground: true,
            provider_instance_id: Some(attachment.interface_name.clone()),
            endpoint: endpoint_handle,
        });
        grants.0.push(ServiceGrant {
            service: "bexos.net.WorkloadNetworkLease".into(),
            protocol: "WorkloadNetworkLease".into(),
            capability: "Lease".into(),
            method_ordinals: Vec::new(),
            permission_values: Vec::new(),
            caller_package: Some(caller_package.into()),
            caller_uid: Some(caller_uid),
            caller_foreground: true,
            provider_instance_id: Some(attachment.interface_name.clone()),
            endpoint: endpoint.lease.raw,
        });
    }
    Ok(core::mem::take(&mut grants.0))
}

fn bind_network_controller(
    broker: &mut crate::broker::AppdBroker,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    provider_package: &str,
    instance_id: &str,
) -> Result<BoundCapability, OpenerStatus> {
    let consumed = crate::manifest::ConsumedService {
        name: "bexos.net.WorkloadNetworkController".into(),
        link_type: crate::manifest::LinkType::Optional,
        filter: None,
        capabilities: vec![crate::manifest::ConsumedCapability {
            capability: "Provision".into(),
            methods: vec![crate::manifest::MethodDependency {
                ordinal: 1,
                link_type: crate::manifest::LinkType::Required,
            }],
        }],
    };
    let client = crate::policy::ClientContext {
        package_name: state::APPD_PACKAGE.into(),
        permissions: vec!["BEXOS_SYSTEM_PRIVILEGED".into()],
        permission_values: Vec::new(),
        is_foreground: true,
        user_id: Some(0),
    };
    let mut bindings = broker
        .bind_consumed_service(&client, &consumed, kernel)
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let Some(index) = bindings.iter().position(|binding| {
        binding.provider_package == provider_package
            && binding.provider_instance_id.as_deref() == Some(instance_id)
    }) else {
        super::close_bound_capabilities(&bindings);
        return Err(OpenerStatus::LaunchFailed);
    };
    let binding = bindings.remove(index);
    super::close_bound_capabilities(&bindings);
    let metadata = super::provider_binding_metadata(&binding);
    if super::deliver_provider_endpoint(&binding, &metadata).is_err() {
        super::close_bound_capabilities(core::slice::from_ref(&binding));
        return Err(OpenerStatus::LaunchFailed);
    }
    Ok(binding)
}

fn close_workload_endpoint(endpoint: &net_fidl::WorkloadNetworkEndpoint<'_>) {
    close_handles(&[
        endpoint.provider.raw,
        endpoint.device.raw,
        endpoint.lease.raw,
    ]);
}

fn endpoint_metadata(
    interface_name: &str,
    endpoint: &net_fidl::WorkloadNetworkEndpoint<'_>,
) -> Result<String, OpenerStatus> {
    use core::fmt::Write;
    let mut value = alloc::format!(
        "v1;name={interface_name};profile={};mode={};id={};mtu={};mac={:02x}{:02x}{:02x}{:02x}{:02x}{:02x};vlan={};raw={};promisc={};group={};domain={};physical={};table={};addressing={}",
        endpoint.profile,
        if endpoint.mode == net_fidl::WorkloadNetworkMode::DirectProvider {
            "direct"
        } else {
            "l2"
        },
        endpoint.interface_id,
        endpoint.mtu,
        endpoint.mac[0],
        endpoint.mac[1],
        endpoint.mac[2],
        endpoint.mac[3],
        endpoint.mac[4],
        endpoint.mac[5],
        endpoint.vlan_id,
        u8::from(endpoint.allow_raw),
        u8::from(endpoint.allow_promiscuous),
        endpoint.isolation_group,
        endpoint.domain,
        endpoint.physical_selector,
        endpoint.table.value,
        endpoint.addressing,
    );
    for index in 0..endpoint.addresses.len() {
        let subnet = endpoint
            .addresses
            .get(index)
            .map_err(|_| OpenerStatus::LaunchFailed)?;
        write!(
            &mut value,
            ";addr={}/{}",
            format_ip(subnet.network),
            subnet.prefix_len
        )
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    }
    for index in 0..endpoint.gateways.len() {
        let subnet = endpoint
            .gateways
            .get(index)
            .map_err(|_| OpenerStatus::LaunchFailed)?;
        write!(
            &mut value,
            ";gateway={}/{}",
            format_ip(subnet.network),
            subnet.prefix_len
        )
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    }
    for index in 0..endpoint.dns_servers.len() {
        let address = endpoint
            .dns_servers
            .get(index)
            .map_err(|_| OpenerStatus::LaunchFailed)?;
        write!(&mut value, ";dns={}", format_ip(address))
            .map_err(|_| OpenerStatus::LaunchFailed)?;
    }
    Ok(value)
}

fn format_ip(address: net_fidl::IpAddress) -> String {
    match address {
        net_fidl::IpAddress::Ipv4(value) => alloc::format!(
            "{}.{}.{}.{}",
            value.octets[0],
            value.octets[1],
            value.octets[2],
            value.octets[3]
        ),
        net_fidl::IpAddress::Ipv6(value) => value
            .octets
            .chunks_exact(2)
            .map(|word| alloc::format!("{:x}", u16::from_be_bytes([word[0], word[1]])))
            .collect::<Vec<_>>()
            .join(":"),
    }
}

fn close_network_grants(grants: &[ServiceGrant]) {
    for grant in grants {
        let _ = Memory::close(grant.endpoint);
    }
}

fn network_status(status: net_fidl::Status) -> OpenerStatus {
    match status {
        net_fidl::Status::ErrAccessDenied => OpenerStatus::AccessDenied,
        net_fidl::Status::ErrInvalidArgs | net_fidl::Status::ErrNotFound => {
            OpenerStatus::InvalidArgs
        }
        _ => OpenerStatus::LaunchFailed,
    }
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
