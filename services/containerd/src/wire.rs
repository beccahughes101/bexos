use crate::{launcher, runtime::*, spec::*};
use app_opener_fidl as opener;
use app_opener_fidl::FidlDecode as OpenerFidlDecode;
use bexos_pkg_client::{PackageStatus, PendingResolution};
use bexos_userspace::{Channel, Memory, Socket};
use container_fidl::*;
use std::collections::BTreeSet;

fn reply<T: FidlEncode>(channel: u64, value: &T) -> Result<(), ()> {
    let mut bytes = vec![0; 128 * 1024];
    let mut refs = [HandleRef { raw: 0 }; 4];
    let encoded = value.encode(&mut bytes, &mut refs).map_err(|_| ())?;
    Channel(channel)
        .send(
            &bytes[..encoded.bytes],
            &refs[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| ())
}
fn status(channel: u64, ordinal: u64, value: ContainerStatus) {
    match ordinal {
        1 => {
            let _ = reply(channel, &ContainerManagerCreateResponse { status: value });
        }
        2 => {
            let _ = reply(
                channel,
                &ContainerManagerStartResponse {
                    status: value,
                    process_control: HandleRef { raw: 0 },
                },
            );
        }
        3 => {
            let _ = reply(channel, &ContainerManagerSignalResponse { status: value });
        }
        4 => {
            let _ = reply(channel, &ContainerManagerDeleteResponse { status: value });
        }
        _ => {}
    }
}
fn map_package(value: PackageStatus) -> ContainerStatus {
    match value {
        PackageStatus::InvalidArgs => ContainerStatus::InvalidArgs,
        PackageStatus::NotFound => ContainerStatus::NotFound,
        PackageStatus::VerifyFailed | PackageStatus::AccessDenied => ContainerStatus::VerifyFailed,
        PackageStatus::Unavailable | PackageStatus::TimedOut => ContainerStatus::Unavailable,
        _ => ContainerStatus::Storage,
    }
}
fn close_handles(handles: &[u64]) {
    for handle in handles {
        if *handle != 0 {
            let _ = Memory::close(*handle);
        }
    }
}
fn info(container: &Container) -> ContainerInfo<'_> {
    ContainerInfo {
        container_id: &container.spec.container_id,
        state: container.state,
        manifest_digest: container.spec.manifest_digest,
        exit_code: container.exit_code,
    }
}

pub fn poll_clients(runtime: &mut Runtime) {
    for client in runtime.clients.clone() {
        let message = match Channel(client.channel).try_recv() {
            Ok(message) => message,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                if runtime
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.client == client.channel)
                {
                    runtime.pending.as_mut().unwrap().client = 0;
                }
                let _ = Memory::close(client.channel);
                runtime
                    .clients
                    .retain(|other| other.channel != client.channel);
                continue;
            }
            Err(_) => continue,
        };
        let ordinal = message
            .bytes
            .get(..8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
            .unwrap_or(0);
        if !client.methods.contains(&ordinal) {
            for handle in message.handles {
                let _ = Memory::close(handle);
            }
            status(client.channel, ordinal, ContainerStatus::InvalidArgs);
            continue;
        }
        dispatch(
            runtime,
            client.channel,
            ordinal,
            &message.bytes[8..],
            &message.handles,
        );
    }
}

fn dispatch(runtime: &mut Runtime, channel: u64, ordinal: u64, bytes: &[u8], handles: &[u64]) {
    let refs = handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    match ordinal {
        1 => {
            if !handles.is_empty() || runtime.pending.is_some() || runtime.resolver == 0 {
                close_handles(handles);
                status(
                    channel,
                    1,
                    if runtime.pending.is_some() {
                        ContainerStatus::Busy
                    } else {
                        ContainerStatus::InvalidArgs
                    },
                );
                return;
            }
            let result = (|| {
                let q = ContainerManagerCreateRequest::decode(bytes, &refs)
                    .map_err(|_| ContainerStatus::InvalidArgs)?;
                let expected = q.image.expected_sha256.to_vec();
                let arguments = (0..q.arguments.len())
                    .map(|i| {
                        q.arguments
                            .get(i)
                            .map(str::to_string)
                            .map_err(|_| ContainerStatus::InvalidArgs)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let environment = (0..q.environment.len())
                    .map(|i| {
                        q.environment
                            .get(i)
                            .map(str::to_string)
                            .map_err(|_| ContainerStatus::InvalidArgs)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let desired = DesiredSpec {
                    container_id: q.container_id.into(),
                    image: crate::spec::ImageReference {
                        registry_host: q.image.registry_host.into(),
                        repository: q.image.repository.into(),
                        tag: q.image.tag.into(),
                        expected_sha256: expected,
                    },
                    arguments,
                    environment,
                    working_directory: q.working_directory.into(),
                    uid: q.uid,
                    gid: q.gid,
                    hostname: q.hostname.into(),
                    resources: Resources {
                        cpu_shares: q.resources.cpu_shares,
                        memory_limit_bytes: q.resources.memory_limit_bytes,
                        process_limit: q.resources.process_limit,
                    },
                    readonly_rootfs: q.readonly_rootfs,
                };
                desired.validate()?;
                if runtime.containers.contains_key(&desired.container_id) {
                    return Err(ContainerStatus::AlreadyExists);
                }
                if runtime.containers.len() >= MAX_CONTAINERS {
                    return Err(ContainerStatus::Busy);
                }
                let resolver = Channel(core::mem::replace(&mut runtime.resolver, 0));
                let resolution = match PendingResolution::begin(resolver, &desired.query()) {
                    Ok(pending) => pending,
                    Err(error) => {
                        runtime.resolver = resolver.0;
                        return Err(map_package(error));
                    }
                };
                runtime.pending = Some(PendingCreate {
                    client: channel,
                    desired,
                    resolution,
                });
                Ok(())
            })();
            if let Err(error) = result {
                status(channel, 1, error);
            }
        }
        2 => start(runtime, channel, bytes, handles, &refs),
        3 => {
            if !handles.is_empty() {
                close_handles(handles);
                status(channel, 3, ContainerStatus::InvalidArgs);
                return;
            }
            let result = ContainerManagerSignalRequest::decode(bytes, &refs)
                .map_err(|_| ContainerStatus::InvalidArgs)
                .and_then(|q| {
                    let container = runtime
                        .containers
                        .get(q.container_id)
                        .ok_or(ContainerStatus::NotFound)?;
                    if container.state != ContainerState::Running || container.process == 0 {
                        return Err(ContainerStatus::Busy);
                    }
                    launcher::signal(container.process, q.signal)
                        .map_err(|_| ContainerStatus::LaunchFailed)
                });
            status(
                channel,
                3,
                result.map_or_else(|e| e, |_| ContainerStatus::Ok),
            );
        }
        4 => {
            if !handles.is_empty() {
                close_handles(handles);
                status(channel, 4, ContainerStatus::InvalidArgs);
                return;
            }
            let result = ContainerManagerDeleteRequest::decode(bytes, &refs)
                .map_err(|_| ContainerStatus::InvalidArgs)
                .and_then(|q| {
                    let container = runtime
                        .containers
                        .get_mut(q.container_id)
                        .ok_or(ContainerStatus::NotFound)?;
                    if container.state == ContainerState::Running {
                        if !q.force {
                            return Err(ContainerStatus::Busy);
                        }
                        launcher::signal(container.process, 9)
                            .map_err(|_| ContainerStatus::LaunchFailed)?;
                        container.close_runtime_handles();
                    }
                    crate::storage::delete(runtime.data, q.container_id)
                        .map_err(|_| ContainerStatus::Storage)?;
                    for proxy in &container.proxies {
                        let _ = Memory::close(proxy.channel);
                    }
                    runtime.containers.remove(q.container_id);
                    Ok(())
                });
            status(
                channel,
                4,
                result.map_or_else(|e| e, |_| ContainerStatus::Ok),
            );
        }
        5 => {
            if !handles.is_empty() {
                close_handles(handles);
                let _ = reply(
                    channel,
                    &ContainerManagerInspectResponse {
                        status: ContainerStatus::InvalidArgs,
                        info: WireVector::from_slice(&[]),
                    },
                );
                return;
            }
            let request = ContainerManagerInspectRequest::decode(bytes, &refs);
            let result = request
                .as_ref()
                .ok()
                .and_then(|q| runtime.containers.get(q.container_id));
            let infos = result.map(info).into_iter().collect::<Vec<_>>();
            let _ = reply(
                channel,
                &ContainerManagerInspectResponse {
                    status: if request.is_err() {
                        ContainerStatus::InvalidArgs
                    } else if infos.is_empty() {
                        ContainerStatus::NotFound
                    } else {
                        ContainerStatus::Ok
                    },
                    info: WireVector::from_slice(&infos),
                },
            );
        }
        6 => {
            if !handles.is_empty() {
                close_handles(handles);
                let _ = reply(
                    channel,
                    &ContainerManagerListResponse {
                        status: ContainerStatus::InvalidArgs,
                        containers: WireVector::from_slice(&[]),
                    },
                );
                return;
            }
            if ContainerManagerListRequest::decode(bytes, &refs).is_err() {
                let _ = reply(
                    channel,
                    &ContainerManagerListResponse {
                        status: ContainerStatus::InvalidArgs,
                        containers: WireVector::from_slice(&[]),
                    },
                );
                return;
            }
            let infos = runtime.containers.values().map(info).collect::<Vec<_>>();
            let _ = reply(
                channel,
                &ContainerManagerListResponse {
                    status: ContainerStatus::Ok,
                    containers: WireVector::from_slice(&infos),
                },
            );
        }
        _ => {
            for handle in handles {
                let _ = Memory::close(*handle);
            }
        }
    }
}

fn start(runtime: &mut Runtime, channel: u64, bytes: &[u8], handles: &[u64], refs: &[HandleRef]) {
    // Every received handle remains owned here until it is either transferred to
    // appd or retained by the container record. This keeps all validation and
    // partial-construction failures leak-free.
    let mut owned = handles.iter().copied().collect::<BTreeSet<_>>();
    let result = (|| {
        let q = ContainerManagerStartRequest::decode(bytes, refs)
            .map_err(|_| ContainerStatus::InvalidArgs)?;
        let container = runtime
            .containers
            .get_mut(q.container_id)
            .ok_or(ContainerStatus::NotFound)?;
        if container.state == ContainerState::Running {
            return Err(ContainerStatus::Busy);
        }
        if container.proxies.len() >= MAX_CONTROL_PROXIES {
            return Err(ContainerStatus::Busy);
        }
        let requested = [q.stdin_stream.raw, q.stdout_stream.raw, q.stderr_stream.raw];
        let requested_handles = requested
            .iter()
            .copied()
            .filter(|handle| *handle != 0)
            .collect::<BTreeSet<_>>();
        if requested_handles != owned || requested_handles.len() != handles.len() {
            return Err(ContainerStatus::InvalidArgs);
        }
        let mut stdio = [0; 3];
        let mut peers = [0; 3];
        for index in 0..3 {
            if requested[index] != 0 {
                stdio[index] = requested[index];
            } else {
                let (local, remote) = Socket::pair().map_err(|_| ContainerStatus::LaunchFailed)?;
                stdio[index] = remote.0;
                owned.insert(remote.0);
                if index == 0 {
                    let _ = Memory::close(local.0);
                } else {
                    peers[index] = local.0;
                    owned.insert(local.0);
                }
            }
        }
        let rootfs = crate::storage::open_rootfs(runtime.data, q.container_id)
            .map_err(|_| ContainerStatus::Storage)?;
        owned.insert(rootfs.0);
        let transferred = [rootfs.0, stdio[0], stdio[1], stdio[2]];
        for handle in transferred {
            owned.remove(&handle);
        }
        let process = launcher::launch(runtime.launcher, &container.spec, rootfs.0, stdio)
            .map_err(|_| ContainerStatus::LaunchFailed)?;
        owned.insert(process);
        let (client, server) = Channel::pair().map_err(|_| ContainerStatus::LaunchFailed)?;
        owned.insert(client.0);
        owned.insert(server.0);
        let next_generation = container
            .generation
            .checked_add(1)
            .ok_or(ContainerStatus::Busy)?;
        container.close_runtime_handles();
        container.generation = next_generation;
        container.process = process;
        container.io_peers = peers;
        container.state = ContainerState::Running;
        container.exit_code = 0;
        container.proxies.push(ControlProxy {
            channel: server.0,
            generation: container.generation,
            completion: None,
            watch_pending: false,
        });
        owned.remove(&process);
        owned.remove(&server.0);
        for peer in peers {
            owned.remove(&peer);
        }
        owned.remove(&client.0);
        Ok(client.0)
    })();
    close_handles(&owned.into_iter().collect::<Vec<_>>());
    match result {
        Ok(process) => {
            let response = ContainerManagerStartResponse {
                status: ContainerStatus::Ok,
                process_control: HandleRef { raw: process },
            };
            if reply(channel, &response).is_err() {
                let _ = Memory::close(process);
            }
        }
        Err(error) => status(channel, 2, error),
    }
}

pub fn poll_pending(runtime: &mut Runtime) {
    let result = match runtime.pending.as_mut() {
        Some(p) => p.resolution.poll(),
        None => return,
    };
    let outcome = match result {
        Ok(None) => return,
        value => value,
    };
    let pending = runtime.pending.take().unwrap();
    runtime.resolver = pending.resolution.into_channel().map_or(0, |c| c.0);
    let result = match outcome {
        Err(error) => Err(map_package(error)),
        Ok(Some(vmo)) => (|| {
            let image = bexos_oci::parse_layout(vmo.bytes(), architecture())
                .map_err(|_| ContainerStatus::VerifyFailed)?;
            let spec = effective_spec(pending.desired, &image.process, image.manifest_digest)?;
            crate::storage::install(runtime.data, &spec, &image)
                .map_err(|_| ContainerStatus::Storage)?;
            runtime.containers.insert(
                spec.container_id.clone(),
                Container {
                    spec,
                    state: ContainerState::Created,
                    exit_code: 0,
                    process: 0,
                    io_peers: [0; 3],
                    generation: 0,
                    proxies: Vec::new(),
                },
            );
            Ok(())
        })(),
        Ok(None) => unreachable!(),
    };
    if pending.client != 0 {
        status(
            pending.client,
            1,
            result.map_or_else(|e| e, |_| ContainerStatus::Ok),
        );
    }
}

pub fn poll_processes(runtime: &mut Runtime) {
    runtime.poll_tick = runtime.poll_tick.wrapping_add(1);
    poll_control_proxies(runtime);
    for container in runtime
        .containers
        .values_mut()
        .filter(|c| c.state == ContainerState::Running)
    {
        container.drain_output();
    }
    if runtime.poll_tick % 128 != 0 {
        return;
    }
    for container in runtime
        .containers
        .values_mut()
        .filter(|c| c.state == ContainerState::Running)
    {
        if let Ok((true, code)) = launcher::status(container.process) {
            container.exit_code = code;
            for proxy in &mut container.proxies {
                if proxy.generation == container.generation {
                    proxy.completion = Some(code);
                }
            }
            container.state = ContainerState::Stopped;
            container.close_runtime_handles();
        }
    }
}

fn opener_reply<T: opener::FidlEncode>(channel: u64, value: &T) -> Result<(), ()> {
    let mut bytes = vec![0; 4096];
    let encoded = value.encode(&mut bytes, &mut []).map_err(|_| ())?;
    Channel(channel)
        .send(&bytes[..encoded.bytes], &[])
        .map_err(|_| ())
}

fn poll_control_proxies(runtime: &mut Runtime) {
    for container in runtime.containers.values_mut() {
        let mut closed = Vec::new();
        for proxy in &mut container.proxies {
            for _ in 0..8 {
                let message = match Channel(proxy.channel).try_recv() {
                    Ok(message) => message,
                    Err(kernel_fidl::Status::ErrPeerClosed) => {
                        closed.push(proxy.channel);
                        break;
                    }
                    Err(_) => break,
                };
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
                let ordinal = message
                    .bytes
                    .get(..8)
                    .and_then(|b| b.try_into().ok())
                    .map(u64::from_le_bytes)
                    .unwrap_or(0);
                let body = message.bytes.get(8..).unwrap_or_default();
                let active = proxy.generation == container.generation
                    && container.state == ContainerState::Running;
                match ordinal {
                    1 => {
                        let _ = opener_reply(
                            proxy.channel,
                            &opener::ProcessControlGetStatusResponse {
                                status: opener::OpenerStatus::Ok,
                                exited: proxy.completion.is_some(),
                                suspended: false,
                                exit_code: proxy.completion.unwrap_or(0),
                            },
                        );
                    }
                    2 => {
                        if let Some(code) = proxy.completion {
                            let _ = opener_reply(
                                proxy.channel,
                                &opener::ProcessControlWatchExitResponse {
                                    status: opener::OpenerStatus::Ok,
                                    exit_code: code,
                                },
                            );
                        } else if !proxy.watch_pending {
                            proxy.watch_pending = true;
                        } else {
                            let _ = opener_reply(
                                proxy.channel,
                                &opener::ProcessControlWatchExitResponse {
                                    status: opener::OpenerStatus::InvalidArgs,
                                    exit_code: 0,
                                },
                            );
                        }
                    }
                    3 => {
                        let response = opener::ProcessControlSendSignalRequest::decode(body, &[])
                            .ok()
                            .filter(|_| active)
                            .and_then(|q| launcher::signal(container.process, q.signal).ok())
                            .map_or(opener::OpenerStatus::InvalidArgs, |_| {
                                opener::OpenerStatus::Ok
                            });
                        let _ = opener_reply(
                            proxy.channel,
                            &opener::ProcessControlSendSignalResponse { status: response },
                        );
                    }
                    4 => {
                        let _ = opener_reply(
                            proxy.channel,
                            &opener::ProcessControlSuspendResponse {
                                status: opener::OpenerStatus::AccessDenied,
                            },
                        );
                    }
                    5 => {
                        let _ = opener_reply(
                            proxy.channel,
                            &opener::ProcessControlResumeResponse {
                                status: opener::OpenerStatus::AccessDenied,
                            },
                        );
                    }
                    _ => {}
                }
            }
            if proxy.watch_pending {
                if let Some(code) = proxy.completion {
                    if opener_reply(
                        proxy.channel,
                        &opener::ProcessControlWatchExitResponse {
                            status: opener::OpenerStatus::Ok,
                            exit_code: code,
                        },
                    )
                    .is_ok()
                    {
                        proxy.watch_pending = false;
                    }
                }
            }
        }
        container
            .proxies
            .retain(|proxy| !closed.contains(&proxy.channel));
        for channel in closed {
            let _ = Memory::close(channel);
        }
    }
}
