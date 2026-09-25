pub mod migration;

use alloc::vec;
use alloc::vec::Vec;

use bexos_userspace::{Channel, Memory, Startup, log};
use fs_fidl::*;
use memfs_fidl as memfs;
use memfs_fidl::FidlDecode as MemfsDecode;

use crate::{MemFs, MemFsError, NodeAttributes, NodeKind, OpenedNode, ROOT_INODE};

pub(crate) struct TmpInstance {
    pub fs: MemFs,
}

pub(crate) struct Endpoint {
    pub channel: Channel,
    pub instance: usize,
    pub inode: u64,
    pub opened: Result<Option<OpenedNode>, FsStatus>,
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap();
    let state = if start.migration_target {
        match bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            start.migration_generation,
        ) {
            Ok(state) => state,
            Err(_) => bexos_userspace::exit(),
        }
    } else {
        log("memfs: D1 memory filesystem driver ready\n");
        Startup::ready(control).unwrap();
        migration::Runtime {
            control,
            migration: start.migration,
            instances: Vec::new(),
            endpoints: Vec::new(),
        }
    };
    serve(state).await
}

pub(crate) async fn serve(mut state: migration::Runtime) -> ! {
    let control = state.control;
    let mut source = bexos_userspace::live_migration::Source::new(state.migration);
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        let old_instances = state.instances.len();
        if let Ok(message) = control.try_recv() {
            let (ordinal, request) = envelope(&message.bytes);
            let handles = memfs_refs(&message.handles);
            match ordinal {
                70 => {
                    let _request =
                        memfs::MemfsManagerCreateTmpDirectoryRequest::decode(request, &handles)
                            .unwrap();
                    let id = state.instances.len();
                    state.instances.push(TmpInstance { fs: MemFs::new() });
                    let (client, server) = Channel::pair().unwrap();
                    state.endpoints.push(Endpoint {
                        channel: server,
                        instance: id,
                        inode: ROOT_INODE,
                        opened: Ok(None),
                    });
                    source.changed(0);
                    source.changed(migration::endpoint_key(server));
                    memfs_reply(
                        control,
                        &memfs::MemfsManagerCreateTmpDirectoryResponse {
                            status: FsStatus::Ok as i32,
                            root_dir: memfs::HandleRef { raw: client.0 },
                        },
                    );
                }
                _ => panic!("unknown memfs manager ordinal"),
            }
        }
        serve_endpoint_messages(&mut state.instances, &mut state.endpoints, &mut source);
        release_unused_instances(&mut state.instances, &mut state.endpoints);
        if state.instances.len() != old_instances {
            source.changed(0);
        }
        bexos_userspace::yield_now();
    }
}

fn serve_endpoint_messages(
    instances: &mut [TmpInstance],
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
                source.changed(migration::endpoint_key(endpoint.channel));
                close.push(index);
                continue;
            }
            Err(kernel_fidl::Status::ErrTimedOut) => continue,
            Err(error) => panic!("memfs endpoint {error:?}"),
        };
        source.changed(migration::endpoint_key(endpoint.channel));
        let channel = endpoint.channel;
        let instance = endpoint.instance;
        match dispatch_endpoint_message(
            &mut instances[instance],
            endpoint,
            channel,
            instance,
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
        source.changed(migration::endpoint_key(endpoint.channel));
        let _ = Memory::close(endpoint.channel.0);
    }
}

fn dispatch_endpoint_message(
    instance: &mut TmpInstance,
    endpoint: &mut Endpoint,
    channel: Channel,
    instance_id: usize,
    source: &mut bexos_userspace::live_migration::Source,
    message: bexos_userspace::Message,
) -> DispatchResult {
    let (ordinal, request) = envelope(&message.bytes);
    let handles = refs(&message.handles);
    match ordinal {
        1 => {
            let attrs = match endpoint.opened {
                Err(status) => Err(status),
                _ => instance.fs.attributes(endpoint.inode).map_err(status),
            };
            fs_reply(
                channel,
                &NodeGetAttrResponse {
                    status: attrs.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    attr: attrs.map(attributes).unwrap_or_else(|_| empty_attrs()),
                },
            );
        }
        2 => {
            let request = NodeSetAttrRequest::decode(request, &handles).unwrap();
            let metadata = crate::NodeAttributes {
                size_bytes: request.attr.size_bytes,
                storage_allocated_bytes: request.attr.storage_allocated_bytes,
                creation_time_nanos: request.attr.creation_time_nanos,
                modification_time_nanos: request.attr.modification_time_nanos,
                mode: request.attr.mode,
                uid: request.attr.uid,
                gid: request.attr.gid,
            };
            let result = match endpoint.opened.as_ref() {
                Ok(Some(opened)) => instance
                    .fs
                    .set_len(opened, request.attr.size_bytes)
                    .and_then(|()| instance.fs.set_metadata(endpoint.inode, metadata))
                    .map_err(status),
                Ok(None) => instance
                    .fs
                    .set_metadata(endpoint.inode, metadata)
                    .map_err(status),
                Err(status) => Err(*status),
            };
            fs_reply(
                channel,
                &NodeSetAttrResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        3 => return DispatchResult::Close,
        4 => {
            let request = NodeGetXattrRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .get_xattr(endpoint.inode, request.name)
                .map_err(status);
            fs_reply(
                channel,
                &NodeGetXattrResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    value: &result.unwrap_or_default(),
                },
            );
        }
        5 => {
            let request = NodeSetXattrRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .set_xattr(endpoint.inode, request.name, request.value, request.flags)
                .map_err(status);
            fs_reply(
                channel,
                &NodeSetXattrResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        6 => {
            let result = instance.fs.list_xattrs(endpoint.inode).map_err(status);
            let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
            let names = result.unwrap_or_default();
            let names = names.iter().map(|name| name.as_str()).collect::<Vec<_>>();
            fs_reply(
                channel,
                &NodeListXattrsResponse {
                    status,
                    names: WireStringVector::from_slice(&names),
                },
            );
        }
        7 => {
            let request = NodeRemoveXattrRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .remove_xattr(endpoint.inode, request.name)
                .map_err(status);
            fs_reply(
                channel,
                &NodeRemoveXattrResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        10 => {
            let request = FileReadRequest::decode(request, &handles).unwrap();
            let result = match endpoint.opened.as_mut() {
                Ok(Some(opened)) => instance
                    .fs
                    .read(opened, request.count.min(32768))
                    .map_err(status),
                Err(status) => Err(*status),
                _ => Err(FsStatus::IsDirectory),
            };
            fs_reply(
                channel,
                &FileReadResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    data: &result.unwrap_or_default(),
                },
            );
        }
        11 => {
            let request = FileWriteRequest::decode(request, &handles).unwrap();
            let result = match endpoint.opened.as_mut() {
                Ok(Some(opened)) => instance.fs.write(opened, request.data).map_err(status),
                Err(status) => Err(*status),
                _ => Err(FsStatus::IsDirectory),
            };
            fs_reply(
                channel,
                &FileWriteResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    written: result.unwrap_or(0),
                },
            );
        }
        12 => {
            let request = FileSeekRequest::decode(request, &handles).unwrap();
            let result = match endpoint.opened.as_mut() {
                Ok(Some(opened)) => instance
                    .fs
                    .seek(opened, request.offset, request.whence)
                    .map_err(status),
                Err(status) => Err(*status),
                _ => Err(FsStatus::IsDirectory),
            };
            fs_reply(
                channel,
                &FileSeekResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    new_offset: result.unwrap_or(0),
                },
            );
        }
        13 => {
            let result = backing_memory(instance, endpoint);
            fs_reply(
                channel,
                &FileGetBackingMemoryResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    vmo: HandleRef {
                        raw: result.unwrap_or(0),
                    },
                },
            );
        }
        14 => fs_reply(
            channel,
            &FileSyncResponse {
                status: FsStatus::Ok,
            },
        ),
        20 => {
            let request = DirectoryOpenRequest::decode(request, &handles).unwrap();
            let opened = match endpoint.opened {
                Err(status) => Err(status),
                _ => instance
                    .fs
                    .open(endpoint.inode, request.path, request.flags.0)
                    .map(Some)
                    .map_err(status),
            };
            let inode = match opened {
                Ok(Some(opened)) => opened.inode(),
                _ => endpoint.inode,
            };
            let channel = Channel(request.object.raw);
            source.changed(migration::endpoint_key(channel));
            return DispatchResult::Open(Endpoint {
                channel,
                instance: instance_id,
                inode,
                opened,
            });
        }
        21 => {
            let result = instance.fs.read_entries(endpoint.inode).map_err(status);
            let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
            let entries = result.unwrap_or_default();
            let entries: Vec<_> = entries
                .iter()
                .map(|entry| DirEntry {
                    name: &entry.name,
                    kind: match entry.kind {
                        NodeKind::File => fs_fidl::NodeKind::File,
                        NodeKind::Directory => fs_fidl::NodeKind::Directory,
                        NodeKind::Symlink => fs_fidl::NodeKind::Symlink,
                    },
                    attr: attributes(entry.attributes),
                })
                .collect();
            fs_reply(
                channel,
                &DirectoryReadEntriesResponse {
                    status,
                    entries: WireVector::from_slice(&entries),
                },
            );
        }
        22 => {
            let request = DirectoryUnlinkRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .unlink(endpoint.inode, request.name)
                .map_err(status);
            fs_reply(
                channel,
                &DirectoryUnlinkResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        23 => {
            let request = DirectoryRenameRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .rename(endpoint.inode, request.source, request.target)
                .map_err(status);
            fs_reply(
                channel,
                &DirectoryRenameResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        24 => {
            let request = DirectoryLinkRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .link(endpoint.inode, request.source, request.target)
                .map_err(status);
            fs_reply(
                channel,
                &DirectoryLinkResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        25 => {
            let request = DirectorySymlinkRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .symlink(endpoint.inode, request.target, request.link_path)
                .map_err(status);
            fs_reply(
                channel,
                &DirectorySymlinkResponse {
                    status: result.err().unwrap_or(FsStatus::Ok),
                },
            );
        }
        26 => {
            let request = DirectoryReadLinkRequest::decode(request, &handles).unwrap();
            let result = instance
                .fs
                .readlink(endpoint.inode, request.path)
                .map_err(status);
            fs_reply(
                channel,
                &DirectoryReadLinkResponse {
                    status: result.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                    target: result.as_deref().unwrap_or(""),
                },
            );
        }
        32 => fs_reply(
            channel,
            &FilesystemSyncResponse {
                status: FsStatus::Ok,
            },
        ),
        _ => panic!("unknown memfs file ordinal"),
    }
    DispatchResult::Keep
}

fn backing_memory(instance: &TmpInstance, endpoint: &Endpoint) -> Result<u64, FsStatus> {
    let opened = endpoint.opened?.ok_or(FsStatus::IsDirectory)?;
    let bytes = instance.fs.backing_bytes(&opened).map_err(status)?;
    if bytes.is_empty() {
        return Err(FsStatus::InvalidArgs);
    }
    let handle = Memory::from_bytes(bytes).map_err(|_| FsStatus::NoSpace)?;
    let immutable = Memory::duplicate(handle, 1 | 2 | 8 | 16 | 32).map_err(|_| FsStatus::Io)?;
    Memory::close(handle).unwrap();
    Ok(immutable)
}

fn release_unused_instances(instances: &mut Vec<TmpInstance>, endpoints: &mut [Endpoint]) {
    let mut used = vec![false; instances.len()];
    for endpoint in endpoints.iter() {
        if let Some(slot) = used.get_mut(endpoint.instance) {
            *slot = true;
        }
    }
    for instance in (0..instances.len()).rev() {
        if used[instance] {
            continue;
        }
        log(&alloc::format!(
            "memfs: release tmp instance index={instance}\n"
        ));
        instances.remove(instance);
        for endpoint in endpoints.iter_mut() {
            if endpoint.instance > instance {
                endpoint.instance -= 1;
            }
        }
    }
}

pub(crate) fn attributes(a: NodeAttributes) -> FileAttributes {
    FileAttributes {
        size_bytes: a.size_bytes,
        storage_allocated_bytes: a.storage_allocated_bytes,
        creation_time_nanos: a.creation_time_nanos,
        modification_time_nanos: a.modification_time_nanos,
        mode: a.mode,
        uid: a.uid,
        gid: a.gid,
    }
}

pub(crate) fn empty_attrs() -> FileAttributes {
    FileAttributes {
        size_bytes: 0,
        storage_allocated_bytes: 0,
        creation_time_nanos: 0,
        modification_time_nanos: 0,
        mode: 0,
        uid: 0,
        gid: 0,
    }
}

pub(crate) fn status(error: MemFsError) -> FsStatus {
    match error {
        MemFsError::Io => FsStatus::Io,
        MemFsError::NoSpace => FsStatus::NoSpace,
        MemFsError::NotFound => FsStatus::NotFound,
        MemFsError::NotDirectory => FsStatus::NotDirectory,
        MemFsError::IsDirectory => FsStatus::IsDirectory,
        MemFsError::NotEmpty => FsStatus::NotEmpty,
        MemFsError::AlreadyExists => FsStatus::AlreadyExists,
        MemFsError::AccessDenied => FsStatus::AccessDenied,
        MemFsError::InvalidArgs => FsStatus::InvalidArgs,
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn refs(hs: &[u64]) -> Vec<HandleRef> {
    hs.iter().map(|h| HandleRef { raw: *h }).collect()
}

fn memfs_refs(hs: &[u64]) -> Vec<memfs::HandleRef> {
    hs.iter().map(|h| memfs::HandleRef { raw: *h }).collect()
}

fn fs_reply<Q: FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = vec![0; 65500];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let encoded = q.encode(&mut bytes, &mut handles).expect("memfs encode");
    channel
        .send(
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
        )
        .expect("memfs reply");
}

fn memfs_reply<Q: memfs::FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = vec![0; 65500];
    let mut handles = [memfs::HandleRef { raw: 0 }; 16];
    let encoded = q
        .encode(&mut bytes, &mut handles)
        .expect("memfs manager encode");
    channel
        .send(
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
        )
        .expect("memfs manager reply");
}

enum DispatchResult {
    Keep,
    Close,
    Open(Endpoint),
}
