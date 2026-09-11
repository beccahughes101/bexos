mod migration;
mod service;
mod wire;

use alloc::vec;
use alloc::vec::Vec;

use archive_fidl as archive;
use archive_fidl::FidlDecode as ArchiveFidlDecode;
use bexos_userspace::{Channel, Memory, Startup, fs, log};
use fs_fidl::*;

use crate::{ArchiveFs, ArchiveFsError, FileHandle, NodeAttributes};

pub(crate) struct MountedArchive {
    fs: ArchiveFs,
}

pub(crate) struct Endpoint {
    channel: Channel,
    archive: usize,
    inode: u64,
    file: Result<Option<FileHandle>, FsStatus>,
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap();
    let state = if start.migration_target {
        match bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            start.migration_generation,
        ) {
            Ok(s) => s,
            Err(error) => {
                log(&alloc::format!(
                    "archivefs: candidate migration rejected error={error:?}\n"
                ));
                bexos_userspace::exit()
            }
        }
    } else {
        log("archivefs: EL0 archive filesystem service ready\n");
        Startup::ready(control).unwrap();
        migration::Runtime {
            control,
            migration: start.migration,
            archives: Vec::new(),
            endpoints: Vec::new(),
        }
    };
    service::serve(state).await
}
pub(crate) async fn serve_inner(mut state: migration::Runtime) -> ! {
    use bexos_userspace::live_migration::State;
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
        let old_archives = state.archives.len();
        let archives = &mut state.archives;
        let endpoints = &mut state.endpoints;
        if let Ok(m) = control.try_recv() {
            let (ordinal, req) = envelope(&m.bytes);
            let hs = archive_refs(&m.handles);
            match ordinal {
                41 => {
                    let q = archive::ArchiveManagerMountMemoryRequest::decode(req, &hs).unwrap();
                    log(&alloc::format!(
                        "archivefs: mount memory archive bytes={}\n",
                        q.length
                    ));
                    let result = (|| {
                        let rounded = bexos_boot::page_round(q.length)
                            .filter(|_| q.length > 0 && q.length <= 16 * 1024 * 1024)
                            .ok_or(ArchiveFsError::InvalidArgs)?;
                        let va = Memory::map(q.archive.raw, rounded, 2)
                            .map_err(|_| ArchiveFsError::AccessDenied)?;
                        let result = ArchiveFs::mount(
                            unsafe {
                                core::slice::from_raw_parts(va as *const u8, q.length as usize)
                            },
                            None,
                        );
                        let _ = Memory::unmap(va, rounded);
                        result
                    })();
                    let _ = Memory::close(q.archive.raw);
                    let (st, root) = match result {
                        Ok(fs) => {
                            let id = cached_archive(archives, fs);
                            let inode = archives[id].fs.root_inode();
                            let (client, server) = Channel::pair().unwrap();
                            endpoints.push(Endpoint {
                                channel: server,
                                archive: id,
                                inode,
                                file: Ok(None),
                            });
                            (FsStatus::Ok, client.0)
                        }
                        Err(e) => (status(e), 0),
                    };
                    archive_reply(
                        control,
                        &archive::ArchiveManagerMountMemoryResponse {
                            status: st as i32,
                            root_dir: archive::HandleRef { raw: root },
                        },
                    );
                }
                40 => {
                    let q = archive::ArchiveManagerMountPackageRequest::decode(req, &hs).unwrap();
                    let expected = expected_root(q.expected_merkle_root);
                    let file = Channel(q.package_file.raw);
                    let result =
                        match expected.and_then(|root| cached_archive_by_root(archives, root)) {
                            Some(id) => fs::close(file)
                                .map_err(|_| ArchiveFsError::Corrupt)
                                .map(|_| id),
                            None => mount_package(file, expected, archives),
                        };
                    let (status, root) = match result {
                        Ok(id) => {
                            let inode = archives[id].fs.root_inode();
                            let (client, server) = Channel::pair().unwrap();
                            endpoints.push(Endpoint {
                                channel: server,
                                archive: id,
                                inode,
                                file: Ok(None),
                            });
                            (FsStatus::Ok, client.0)
                        }
                        Err(error) => {
                            log(&alloc::format!(
                                "archivefs: mount package failed {error:?}\n"
                            ));
                            (status(error), 0)
                        }
                    };
                    archive_reply(
                        control,
                        &archive::ArchiveManagerMountPackageResponse {
                            status: status as i32,
                            root_dir: archive::HandleRef { raw: root },
                        },
                    );
                }
                _ => panic!("unknown archivefs ordinal"),
            }
        }

        let count = endpoints.len();
        let mut close = Vec::new();
        for i in 0..count {
            let ep = &mut endpoints[i];
            let m = match ep.channel.try_recv() {
                Ok(m) => m,
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    source.changed(migration::ENDPOINT | ep.channel.0);
                    close.push(i);
                    continue;
                }
                Err(kernel_fidl::Status::ErrTimedOut) => continue,
                Err(e) => panic!("archivefs endpoint {e:?}"),
            };
            source.changed(migration::ENDPOINT | ep.channel.0);
            let (ordinal, req) = envelope(&m.bytes);
            let hs = refs(&m.handles);
            let channel = ep.channel;
            let archive = ep.archive;
            let inode = ep.inode;
            let mounted = &mut archives[archive];
            match ordinal {
                1 => {
                    let attrs = match ep.file {
                        Err(e) => Err(e),
                        _ => mounted.fs.attributes(ep.inode).map_err(status),
                    };
                    fs_reply(
                        channel,
                        &NodeGetAttrResponse {
                            status: attrs.as_ref().err().copied().unwrap_or(FsStatus::Ok),
                            attr: attrs.map(attributes).unwrap_or_else(|_| empty_attrs()),
                        },
                    );
                }
                2 => fs_reply(
                    channel,
                    &NodeSetAttrResponse {
                        status: FsStatus::ReadOnly,
                    },
                ),
                3 => close.push(i),
                10 => {
                    let q = FileReadRequest::decode(req, &hs).unwrap();
                    let result = match ep.file.as_mut() {
                        Ok(Some(file)) => mounted.fs.read(file, q.count.min(32768)).map_err(status),
                        Err(e) => Err(*e),
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
                11 => fs_reply(
                    channel,
                    &FileWriteResponse {
                        status: FsStatus::ReadOnly,
                        written: 0,
                    },
                ),
                12 => {
                    let q = FileSeekRequest::decode(req, &hs).unwrap();
                    let result = match ep.file.as_mut() {
                        Ok(Some(file)) => mounted.fs.seek(file, q.offset, q.whence).map_err(status),
                        Err(e) => Err(*e),
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
                    let result = (|| {
                        let file = ep
                            .file
                            .as_ref()
                            .map_err(|e| *e)?
                            .as_ref()
                            .ok_or(FsStatus::IsDirectory)?;
                        let bytes = mounted.fs.backing_bytes(file).map_err(status)?;
                        source.changed(((archive as u64 + 1) << 40) | (inode << 16));
                        if bytes.is_empty() {
                            return Err(FsStatus::InvalidArgs);
                        }
                        let handle = Memory::from_bytes(&bytes).map_err(|_| FsStatus::NoSpace)?;
                        let immutable = Memory::duplicate(handle, 1 | 2 | 8 | 16 | 32)
                            .map_err(|_| FsStatus::Io)?;
                        Memory::close(handle).unwrap();
                        Ok(immutable)
                    })();
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
                        status: FsStatus::ReadOnly,
                    },
                ),
                20 => {
                    let q = DirectoryOpenRequest::decode(req, &hs).unwrap();
                    let opened = match ep.file {
                        Err(e) => Err(e),
                        _ => mounted.fs.open(inode, q.path, q.flags.0).map_err(status),
                    };
                    let (inode, file) = match opened {
                        Ok(opened) => (opened.inode, Ok(opened.file)),
                        Err(e) => (inode, Err(e)),
                    };
                    source.changed(migration::ENDPOINT | q.object.raw);
                    endpoints.push(Endpoint {
                        channel: Channel(q.object.raw),
                        archive,
                        inode,
                        file,
                    });
                }
                21 => {
                    let result = mounted.fs.read_entries(ep.inode).map_err(status);
                    let status_code = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
                    let entries = result.unwrap_or_default();
                    let wire = entries
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
                        .collect::<Vec<_>>();
                    fs_reply(
                        channel,
                        &DirectoryReadEntriesResponse {
                            status: status_code,
                            entries: WireVector::from_slice(&wire),
                        },
                    );
                }
                22 => fs_reply(
                    channel,
                    &DirectoryUnlinkResponse {
                        status: FsStatus::ReadOnly,
                    },
                ),
                _ => panic!("unknown archivefs file ordinal"),
            }
        }
        for i in close.into_iter().rev() {
            let e = endpoints.remove(i);
            let _ = Memory::close(e.channel.0);
        }
        release_unused_archives(archives, endpoints);
        if state.archives.len() != old_archives {
            source.changed(0);
            source.changed_keys(
                state
                    .keys()
                    .into_iter()
                    .filter(|k| *k & migration::ENDPOINT != 0 || *k >> 40 > old_archives as u64),
            );
        }
        bexos_userspace::yield_now();
    }
}

const MAX_CACHED_ARCHIVES: usize = 16;

fn cached_archive(archives: &mut Vec<MountedArchive>, fs: ArchiveFs) -> usize {
    if let Some(index) = archives
        .iter()
        .position(|mounted| mounted.fs.same_archive(&fs))
    {
        return index;
    }
    let index = archives.len();
    archives.push(MountedArchive { fs });
    index
}

fn cached_archive_by_root(archives: &[MountedArchive], root: [u8; 32]) -> Option<usize> {
    archives
        .iter()
        .position(|mounted| mounted.fs.content_root() == root)
}

fn release_unused_archives(archives: &mut Vec<MountedArchive>, endpoints: &mut [Endpoint]) {
    if archives.len() <= MAX_CACHED_ARCHIVES {
        return;
    }
    let mut used = vec![false; archives.len()];
    for endpoint in endpoints.iter() {
        if let Some(slot) = used.get_mut(endpoint.archive) {
            *slot = true;
        }
    }
    for archive in (0..archives.len()).rev() {
        if archives.len() <= MAX_CACHED_ARCHIVES {
            break;
        }
        if used[archive] {
            continue;
        }
        log(&alloc::format!(
            "archivefs: release archive index={archive}\n"
        ));
        archives.remove(archive);
        for endpoint in endpoints.iter_mut() {
            if endpoint.archive > archive {
                endpoint.archive -= 1;
            }
        }
    }
}

fn mount_package(
    file: Channel,
    expected_root: Option<[u8; 32]>,
    archives: &mut Vec<MountedArchive>,
) -> Result<usize, ArchiveFsError> {
    let backing = fs::backing(file).map_err(|_| ArchiveFsError::AccessDenied);
    let close_file = fs::close(file).map_err(|_| ArchiveFsError::Corrupt);
    let (vmo, size) = backing?;
    close_file?;
    let len = usize::try_from(size).map_err(|_| ArchiveFsError::InvalidArgs)?;
    let rounded = bexos_boot::page_round(size).ok_or(ArchiveFsError::InvalidArgs)?;
    let va = match Memory::map(vmo, rounded, 2) {
        Ok(va) => va,
        Err(_) => {
            let _ = Memory::close(vmo);
            return Err(ArchiveFsError::AccessDenied);
        }
    };
    let mut owned = Vec::with_capacity(len);
    if len != 0 && Memory::commit_range(owned.as_mut_ptr() as u64, len as u64).is_err() {
        let _ = Memory::unmap(va, rounded);
        let _ = Memory::close(vmo);
        return Err(ArchiveFsError::AccessDenied);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(va as *const u8, owned.as_mut_ptr(), len);
        owned.set_len(len);
    }
    let unmap = Memory::unmap(va, rounded).map_err(|_| ArchiveFsError::Corrupt);
    let close_vmo = Memory::close(vmo).map_err(|_| ArchiveFsError::Corrupt);
    unmap?;
    close_vmo?;
    if let Some(index) = archives
        .iter()
        .position(|mounted| mounted.fs.matches_verified_archive(&owned, expected_root))
    {
        return Ok(index);
    }
    ArchiveFs::mount_owned(owned, expected_root).map(|fs| cached_archive(archives, fs))
}

fn expected_root(root: &[u8]) -> Option<[u8; 32]> {
    if root.len() != 32 {
        return None;
    }
    let mut out = [0; 32];
    out.copy_from_slice(root);
    Some(out)
}

fn status(error: ArchiveFsError) -> FsStatus {
    match error {
        ArchiveFsError::Corrupt => FsStatus::Corrupt,
        ArchiveFsError::NotFound => FsStatus::NotFound,
        ArchiveFsError::NotDirectory => FsStatus::NotDirectory,
        ArchiveFsError::IsDirectory => FsStatus::IsDirectory,
        ArchiveFsError::ReadOnly => FsStatus::ReadOnly,
        ArchiveFsError::AccessDenied => FsStatus::AccessDenied,
        ArchiveFsError::InvalidArgs => FsStatus::InvalidArgs,
    }
}

fn attributes(a: NodeAttributes) -> FileAttributes {
    FileAttributes {
        size_bytes: a.size_bytes,
        storage_allocated_bytes: a.storage_allocated_bytes,
        creation_time_nanos: a.creation_time_nanos,
        modification_time_nanos: a.modification_time_nanos,
        mode: a.mode,
    }
}

fn empty_attrs() -> FileAttributes {
    FileAttributes {
        size_bytes: 0,
        storage_allocated_bytes: 0,
        creation_time_nanos: 0,
        modification_time_nanos: 0,
        mode: 0,
    }
}

fn refs(hs: &[u64]) -> Vec<HandleRef> {
    hs.iter().map(|h| HandleRef { raw: *h }).collect()
}

fn archive_refs(hs: &[u64]) -> Vec<archive::HandleRef> {
    hs.iter().map(|h| archive::HandleRef { raw: *h }).collect()
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    wire::envelope(bytes)
}

fn fs_reply<Q: FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = vec![0; 65500];
    let mut hs = [HandleRef { raw: 0 }; 16];
    let e = q.encode(&mut bytes, &mut hs).expect("archivefs encode");
    channel
        .send(
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .expect("archivefs reply");
}

fn archive_reply<Q: archive::FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = vec![0; 65500];
    let mut hs = [archive::HandleRef { raw: 0 }; 16];
    let e = q
        .encode(&mut bytes, &mut hs)
        .expect("archivefs control encode");
    channel
        .send(
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .expect("archivefs control reply");
}
