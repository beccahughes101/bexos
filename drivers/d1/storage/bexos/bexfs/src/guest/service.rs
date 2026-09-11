use alloc::vec::Vec;
use bexos_userspace::live_migration::State;
use bexos_userspace::{Channel, Memory, log};
use fs_fidl::*;

use crate::guest::mount::{endpoint_key, mount, prepare_resize, raw_mount};
use crate::guest::wire::{envelope, refs, reply};
use crate::guest::{Endpoint, MountControl, Volume, attributes, empty_attrs, migration};
use crate::server::status;
use rosefs_core::block::BlockDevice;

pub(crate) async fn serve(mut state: migration::Runtime) -> ! {
    let control = state.control;
    let mut source = bexos_userspace::live_migration::Source::new(state.migration);
    log("bexfs: service loop enter\n");
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        let old_volumes = state.volumes.len();
        let volumes = &mut state.volumes;
        let endpoints = &mut state.endpoints;
        let controls = &mut state.controls;
        if let Ok(message) = control.try_recv() {
            serve_control_message(control, volumes, endpoints, controls, &mut source, message);
        }
        serve_endpoint_messages(volumes, endpoints, &mut source);
        serve_mount_controls(volumes, endpoints, controls, &mut source);
        if state.volumes.len() != old_volumes {
            source.changed(0);
            source.changed_keys(
                state.keys().into_iter().filter(|key| {
                    *key & migration::ENDPOINT != 0 || *key >> 40 > old_volumes as u64
                }),
            );
        }
        for (id, volume) in state.volumes.iter_mut().enumerate() {
            source.changed_keys(
                volume
                    .fs
                    .take_changes()
                    .into_iter()
                    .map(|key| ((id as u64 + 1) << 40) | key),
            );
        }
        bexos_userspace::yield_now();
    }
}

fn serve_control_message(
    control: Channel,
    volumes: &mut Vec<Volume>,
    endpoints: &mut Vec<Endpoint>,
    controls: &mut Vec<MountControl>,
    source: &mut bexos_userspace::live_migration::Source,
    message: bexos_userspace::Message,
) {
    let (ordinal, request) = envelope(&message.bytes);
    let handles = refs(&message.handles);
    match ordinal {
        30 => {
            let request = FilesystemMountRequest::decode(request, &handles).unwrap();
            let result = mount(
                request.block.raw,
                request.partition_label,
                request.key_vmo.raw,
                request.read_only,
            );
            let (status, root) = match result {
                Ok(volume) => {
                    let id = volumes.len();
                    let inode = volume.fs.root_inode();
                    volumes.push(volume);
                    let (client, server) = Channel::pair().unwrap();
                    endpoints.push(Endpoint {
                        channel: server,
                        volume: id,
                        inode,
                        file: Ok(None),
                    });
                    (FsStatus::Ok, client.0)
                }
                Err(status) => {
                    log(&alloc::format!(
                        "bexfs: mount failed label={} status={status:?}\n",
                        request.partition_label
                    ));
                    (status, 0)
                }
            };
            reply(
                control,
                &FilesystemMountResponse {
                    status,
                    root: HandleRef { raw: root },
                },
            );
        }
        33 | 34 => {
            let result = if ordinal == 33 {
                let request =
                    FilesystemFormatAndMountRawRequest::decode(request, &handles).unwrap();
                raw_mount(
                    request.block.raw,
                    request.label,
                    request.key_vmo.raw,
                    request.read_only,
                    true,
                )
            } else {
                let request = FilesystemMountRawRequest::decode(request, &handles).unwrap();
                raw_mount(
                    request.block.raw,
                    request.label,
                    request.key_vmo.raw,
                    request.read_only,
                    false,
                )
            };
            let (status, root, mount_control) = match result {
                Ok(volume) => {
                    let id = volumes.len();
                    let inode = volume.fs.root_inode();
                    volumes.push(volume);
                    let (root_client, root_server) = Channel::pair().unwrap();
                    let (control_client, control_server) = Channel::pair().unwrap();
                    endpoints.push(Endpoint {
                        channel: root_server,
                        volume: id,
                        inode,
                        file: Ok(None),
                    });
                    controls.push(MountControl {
                        channel: control_server,
                        volume: id,
                    });
                    (FsStatus::Ok, root_client.0, control_client.0)
                }
                Err(status) => (status, 0, 0),
            };
            if ordinal == 33 {
                reply(
                    control,
                    &FilesystemFormatAndMountRawResponse {
                        status,
                        root: HandleRef { raw: root },
                        control: HandleRef { raw: mount_control },
                    },
                );
            } else {
                reply(
                    control,
                    &FilesystemMountRawResponse {
                        status,
                        root: HandleRef { raw: root },
                        control: HandleRef { raw: mount_control },
                    },
                );
            }
        }
        32 => {
            let status = volumes
                .iter_mut()
                .try_for_each(|volume| volume.fs.sync(&mut volume.device).map_err(status));
            reply(
                control,
                &FilesystemSyncResponse {
                    status: status.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        31 => {
            let status = volumes
                .iter_mut()
                .try_for_each(|volume| volume.fs.sync(&mut volume.device).map_err(status))
                .err()
                .unwrap_or(FsStatus::Ok);
            if status == FsStatus::Ok {
                for (id, volume) in volumes.iter().enumerate() {
                    source.changed_keys(
                        volume
                            .fs
                            .checkpoint_keys()
                            .into_iter()
                            .map(|key| ((id as u64 + 1) << 40) | key),
                    );
                }
                for endpoint in endpoints.drain(..) {
                    source.changed(endpoint_key(endpoint.channel));
                    let _ = Memory::close(endpoint.channel.0);
                }
                volumes.clear();
            }
            reply(control, &FilesystemUnmountResponse { status });
        }
        _ => panic!("unknown filesystem ordinal"),
    }
}

fn serve_endpoint_messages(
    volumes: &mut [Volume],
    endpoints: &mut Vec<Endpoint>,
    source: &mut bexos_userspace::live_migration::Source,
) {
    let count = endpoints.len();
    let mut close = Vec::new();
    for index in 0..count {
        let endpoint = &mut endpoints[index];
        let message = match endpoint.channel.try_recv() {
            Ok(message) => message,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                source.changed(endpoint_key(endpoint.channel));
                close.push(index);
                continue;
            }
            Err(kernel_fidl::Status::ErrTimedOut) => continue,
            Err(error) => panic!("filesystem endpoint {error:?}"),
        };
        source.changed(endpoint_key(endpoint.channel));
        let channel = endpoint.channel;
        let volume = endpoint.volume;
        match dispatch_endpoint_message(
            &mut volumes[volume],
            endpoint,
            channel,
            volume,
            source,
            message,
        ) {
            DispatchResult::Keep => {}
            DispatchResult::Close => close.push(index),
            DispatchResult::Open(endpoint) => endpoints.push(endpoint),
        }
    }
    for index in close.into_iter().rev() {
        let endpoint = endpoints.remove(index);
        source.changed(endpoint_key(endpoint.channel));
        let _ = Memory::close(endpoint.channel.0);
    }
}

fn serve_mount_controls(
    volumes: &mut Vec<Volume>,
    endpoints: &mut Vec<Endpoint>,
    controls: &mut Vec<MountControl>,
    source: &mut bexos_userspace::live_migration::Source,
) {
    let mut close = Vec::new();
    for index in 0..controls.len() {
        let control = &mut controls[index];
        let message = match control.channel.try_recv() {
            Ok(message) => message,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                source.changed(endpoint_key(control.channel));
                close.push(index);
                continue;
            }
            Err(kernel_fidl::Status::ErrTimedOut) => continue,
            Err(error) => panic!("filesystem mount control {error:?}"),
        };
        source.changed(endpoint_key(control.channel));
        let channel = control.channel;
        let volume = control.volume;
        let result = dispatch_mount_control(volumes, endpoints, controls, channel, volume, message);
        if matches!(result, ControlResult::Close) {
            close.push(index);
        }
    }
    for index in close.into_iter().rev() {
        let control = controls.remove(index);
        source.changed(endpoint_key(control.channel));
        let _ = Memory::close(control.channel.0);
    }
}

fn dispatch_mount_control(
    volumes: &mut Vec<Volume>,
    endpoints: &mut Vec<Endpoint>,
    controls: &mut [MountControl],
    channel: Channel,
    volume_id: usize,
    message: bexos_userspace::Message,
) -> ControlResult {
    let (ordinal, request) = envelope(&message.bytes);
    let handles = refs(&message.handles);
    let Some(volume) = volumes.get_mut(volume_id) else {
        reply(
            channel,
            &MountedFilesystemGetCapacityResponse {
                status: FsStatus::BadState,
                virtual_size_bytes: 0,
                logical_used_bytes: 0,
                storage_allocated_bytes: 0,
            },
        );
        return ControlResult::Keep;
    };
    match ordinal {
        40 => {
            let usage = volume.fs.usage();
            reply(
                channel,
                &MountedFilesystemGetCapacityResponse {
                    status: FsStatus::Ok,
                    virtual_size_bytes: volume.device.num_blocks() * 4096,
                    logical_used_bytes: usage.size_bytes,
                    storage_allocated_bytes: usage.storage_allocated_bytes,
                },
            );
        }
        41 => {
            let request = MountedFilesystemPrepareResizeRequest::decode(request, &handles).unwrap();
            let result = if volume.prepared.is_some() || !volume.managed {
                Err(FsStatus::BadState)
            } else {
                prepare_resize(
                    volume,
                    request.block.raw,
                    request.label,
                    request.key_vmo.raw,
                )
            };
            let (status, token) = match result {
                Ok(prepared) => {
                    let token = prepared.token;
                    volume.prepared = Some(prepared);
                    (FsStatus::Ok, token)
                }
                Err(status) => (status, 0),
            };
            reply(
                channel,
                &MountedFilesystemPrepareResizeResponse { status, token },
            );
        }
        42 => {
            let request = MountedFilesystemCommitResizeRequest::decode(request, &handles).unwrap();
            let status = match volume.prepared.take() {
                Some(prepared) if prepared.token == request.token => {
                    let old_key = core::mem::replace(&mut volume.key_handle, prepared.key_handle);
                    let old_device = core::mem::replace(&mut volume.device, prepared.device);
                    let old_fs = core::mem::replace(&mut volume.fs, prepared.fs);
                    drop(old_fs);
                    drop(old_device);
                    let _ = Memory::close(old_key);
                    volume.owned = true;
                    FsStatus::Ok
                }
                Some(prepared) => {
                    volume.prepared = Some(prepared);
                    FsStatus::InvalidArgs
                }
                None => FsStatus::BadState,
            };
            reply(channel, &MountedFilesystemCommitResizeResponse { status });
        }
        43 => {
            let request = MountedFilesystemAbortResizeRequest::decode(request, &handles).unwrap();
            let status = match volume.prepared.take() {
                Some(prepared) if prepared.token == request.token => {
                    prepared.close();
                    FsStatus::Ok
                }
                Some(prepared) => {
                    volume.prepared = Some(prepared);
                    FsStatus::InvalidArgs
                }
                None => FsStatus::BadState,
            };
            reply(channel, &MountedFilesystemAbortResizeResponse { status });
        }
        44 => {
            let status = scoped_unmount(volumes, endpoints, controls, volume_id);
            reply(channel, &MountedFilesystemUnmountResponse { status });
            return ControlResult::Close;
        }
        _ => panic!("unknown mounted filesystem ordinal"),
    }
    ControlResult::Keep
}

fn scoped_unmount(
    volumes: &mut Vec<Volume>,
    endpoints: &mut Vec<Endpoint>,
    controls: &mut [MountControl],
    volume_id: usize,
) -> FsStatus {
    let Some(volume) = volumes.get_mut(volume_id) else {
        return FsStatus::BadState;
    };
    let status = volume
        .fs
        .sync(&mut volume.device)
        .map_err(status)
        .err()
        .unwrap_or(FsStatus::Ok);
    if status != FsStatus::Ok {
        return status;
    }
    endpoints.retain(|endpoint| {
        if endpoint.volume == volume_id {
            let _ = Memory::close(endpoint.channel.0);
            false
        } else {
            true
        }
    });
    volumes.remove(volume_id);
    for endpoint in endpoints {
        if endpoint.volume > volume_id {
            endpoint.volume -= 1;
        }
    }
    for control in controls {
        if control.volume > volume_id {
            control.volume -= 1;
        }
    }
    FsStatus::Ok
}

fn dispatch_endpoint_message(
    volume: &mut Volume,
    endpoint: &mut Endpoint,
    channel: Channel,
    volume_id: usize,
    source: &mut bexos_userspace::live_migration::Source,
    message: bexos_userspace::Message,
) -> DispatchResult {
    let (ordinal, request) = envelope(&message.bytes);
    let handles = refs(&message.handles);
    match ordinal {
        1 => {
            let attrs = match endpoint.file {
                Err(status) => Err(status),
                _ => volume.fs.attributes(endpoint.inode).map_err(status),
            };
            let status = attrs.as_ref().err().copied().unwrap_or(FsStatus::Ok);
            reply(
                channel,
                &NodeGetAttrResponse {
                    status,
                    attr: attrs.map(attributes).unwrap_or_else(|_| empty_attrs()),
                },
            );
        }
        2 => {
            let request = NodeSetAttrRequest::decode(request, &handles).unwrap();
            let result = match endpoint.file.as_ref() {
                Ok(Some(file)) => volume
                    .fs
                    .set_len(file, request.attr.size_bytes)
                    .map_err(status),
                Err(status) => Err(*status),
                _ => Err(FsStatus::IsDirectory),
            };
            reply(
                channel,
                &NodeSetAttrResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        3 => return DispatchResult::Close,
        10 => {
            let request = FileReadRequest::decode(request, &handles).unwrap();
            let result = match endpoint.file.as_mut() {
                Ok(Some(file)) => volume
                    .fs
                    .read(file, request.count.min(32768))
                    .map_err(status),
                Err(status) => Err(*status),
                _ => Err(FsStatus::IsDirectory),
            };
            let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
            reply(
                channel,
                &FileReadResponse {
                    status,
                    data: &result.unwrap_or_default(),
                },
            );
        }
        11 => {
            let request = FileWriteRequest::decode(request, &handles).unwrap();
            let result = match endpoint.file.as_mut() {
                Ok(Some(file)) => volume.fs.write(file, request.data).map_err(status),
                Err(status) => Err(*status),
                _ => Err(FsStatus::IsDirectory),
            };
            reply(
                channel,
                &FileWriteResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    written: result.unwrap_or(0),
                },
            );
        }
        12 => {
            let request = FileSeekRequest::decode(request, &handles).unwrap();
            let result = match endpoint.file.as_mut() {
                Ok(Some(file)) => volume
                    .fs
                    .seek(file, request.offset, request.whence)
                    .map_err(status),
                Err(status) => Err(*status),
                _ => Err(FsStatus::IsDirectory),
            };
            reply(
                channel,
                &FileSeekResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    new_offset: result.unwrap_or(0),
                },
            );
        }
        13 => {
            let result = backing_memory(volume, endpoint);
            reply(
                channel,
                &FileGetBackingMemoryResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    vmo: HandleRef {
                        raw: result.unwrap_or(0),
                    },
                },
            );
        }
        14 => {
            let result = match endpoint.file {
                Ok(Some(_)) => volume.fs.sync(&mut volume.device).map_err(status),
                Err(status) => Err(status),
                _ => Err(FsStatus::IsDirectory),
            };
            reply(
                channel,
                &FileSyncResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        20 => {
            let request = DirectoryOpenRequest::decode(request, &handles).unwrap();
            log(&alloc::format!(
                "bexfs: directory open path={}\n",
                request.path
            ));
            let file = match endpoint.file {
                Err(status) => Err(status),
                _ => volume
                    .fs
                    .open(endpoint.inode, request.path, request.flags.0)
                    .map(Some)
                    .map_err(status),
            };
            let inode = match file {
                Ok(Some(file)) => file.inode(),
                _ => endpoint.inode,
            };
            let channel = Channel(request.object.raw);
            source.changed(endpoint_key(channel));
            return DispatchResult::Open(Endpoint {
                channel,
                volume: volume_id,
                inode,
                file,
            });
        }
        21 => {
            let result = volume.fs.read_entries(endpoint.inode).map_err(status);
            let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
            let entries = result.unwrap_or_default();
            let entries: Vec<_> = entries
                .iter()
                .map(|entry| DirEntry {
                    name: &entry.name,
                    kind: if entry.kind == crate::NodeKind::File {
                        NodeKind::File
                    } else {
                        NodeKind::Directory
                    },
                    attr: attributes(entry.attributes),
                })
                .collect();
            reply(
                channel,
                &DirectoryReadEntriesResponse {
                    status,
                    entries: WireVector::from_slice(&entries),
                },
            );
        }
        22 => {
            let request = DirectoryUnlinkRequest::decode(request, &handles).unwrap();
            let result = volume
                .fs
                .unlink(endpoint.inode, request.name)
                .map_err(status);
            reply(
                channel,
                &DirectoryUnlinkResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        32 => {
            let result = volume.fs.sync(&mut volume.device).map_err(status);
            reply(
                channel,
                &FilesystemSyncResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        _ => panic!("unknown file ordinal"),
    }
    DispatchResult::Keep
}

fn backing_memory(volume: &mut Volume, endpoint: &Endpoint) -> Result<u64, FsStatus> {
    let mut file = endpoint.file?.ok_or(FsStatus::IsDirectory)?;
    volume.fs.seek(&mut file, 0, 0).map_err(status)?;
    let size = volume
        .fs
        .attributes(endpoint.inode)
        .map_err(status)?
        .size_bytes;
    if size == 0 {
        return Err(FsStatus::InvalidArgs);
    }
    let rounded = bexos_boot::page_round(size).ok_or(FsStatus::InvalidArgs)?;
    let handle = Memory::create(rounded, 0).map_err(|error| {
        log(&alloc::format!(
            "bexfs: backing memory create failed bytes={rounded} error={error:?}\n"
        ));
        FsStatus::NoSpace
    })?;
    let va = match Memory::map(handle, rounded, 6) {
        Ok(va) => va,
        Err(error) => {
            log(&alloc::format!(
                "bexfs: backing memory map failed error={error:?}\n"
            ));
            let _ = Memory::close(handle);
            return Err(FsStatus::AccessDenied);
        }
    };
    if let Err(error) = Memory::commit_range(va, rounded) {
        log(&alloc::format!(
            "bexfs: backing memory commit failed error={error:?}\n"
        ));
        let _ = Memory::unmap(va, rounded);
        let _ = Memory::close(handle);
        return Err(FsStatus::NoSpace);
    }
    let mut written = 0usize;
    while written < size as usize {
        let chunk = match volume.fs.read(&mut file, 32768).map_err(status) {
            Ok(chunk) if !chunk.is_empty() => chunk,
            Ok(_) => {
                log("bexfs: backing memory read eof early\n");
                let _ = Memory::unmap(va, rounded);
                let _ = Memory::close(handle);
                return Err(FsStatus::Io);
            }
            Err(status) => {
                log(&alloc::format!(
                    "bexfs: backing memory read failed status={status:?}\n"
                ));
                let _ = Memory::unmap(va, rounded);
                let _ = Memory::close(handle);
                return Err(status);
            }
        };
        if written.saturating_add(chunk.len()) > size as usize {
            let _ = Memory::unmap(va, rounded);
            let _ = Memory::close(handle);
            return Err(FsStatus::Io);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                chunk.as_ptr(),
                (va as *mut u8).add(written),
                chunk.len(),
            );
        }
        written += chunk.len();
    }
    Memory::unmap(va, rounded).map_err(|error| {
        log(&alloc::format!(
            "bexfs: backing memory unmap failed error={error:?}\n"
        ));
        FsStatus::Io
    })?;
    let immutable = Memory::duplicate(handle, 1 | 2 | 8 | 16 | 32).map_err(|error| {
        log(&alloc::format!(
            "bexfs: backing memory duplicate failed error={error:?}\n"
        ));
        FsStatus::Io
    })?;
    Memory::close(handle).unwrap();
    Ok(immutable)
}

enum DispatchResult {
    Keep,
    Close,
    Open(Endpoint),
}

enum ControlResult {
    Keep,
    Close,
}
