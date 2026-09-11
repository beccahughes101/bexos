use crate::{Channel, Memory, Rpc};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use archive_fidl as archive;
use archive_fidl::{FidlDecode as ArchiveFidlDecode, FidlEncode as ArchiveFidlEncode};
use fs_fidl::*;
pub struct Response {
    pub bytes: Vec<u8>,
    pub handles: Vec<HandleRef>,
}

pub struct OwnedDirEntry {
    pub name: String,
    pub kind: NodeKind,
    pub attr: FileAttributes,
}
// Filesystem servers serialize an atomic namespace-slot commit with ordinary
// requests. On slow emulated block devices, callers queued behind that commit
// need headroom beyond the commit's own 300-second deadline for queueing and
// request dispatch on emulated storage.
const FS_RPC_TIMEOUT_SECONDS: u64 = 420;

fn call<Q: FidlEncode>(
    channel: Channel,
    ordinal: u64,
    q: &Q,
    reply: bool,
) -> Result<Response, FsStatus> {
    let mut bytes = vec![0; 65500];
    let mut hs = [HandleRef { raw: 0 }; 32];
    let e = q
        .encode(&mut bytes, &mut hs)
        .map_err(|_| FsStatus::InvalidArgs)?;
    let handles = hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>();
    let m = Rpc(channel)
        .call_raw_with_timeout(
            ordinal,
            &bytes[..e.bytes],
            &handles,
            reply,
            FS_RPC_TIMEOUT_SECONDS,
        )
        .map_err(|status| {
            crate::log(&alloc::format!(
                "fs: call ordinal={ordinal} failed status={status:?}\n"
            ));
            FsStatus::Io
        })?;
    Ok(Response {
        bytes: m.bytes,
        handles: m.handles.iter().map(|h| HandleRef { raw: *h }).collect(),
    })
}
fn call_with_timeout<Q: FidlEncode>(
    channel: Channel,
    ordinal: u64,
    q: &Q,
    timeout_seconds: u64,
) -> Result<Response, FsStatus> {
    let mut bytes = vec![0; 65500];
    let mut hs = [HandleRef { raw: 0 }; 32];
    let e = q
        .encode(&mut bytes, &mut hs)
        .map_err(|_| FsStatus::InvalidArgs)?;
    let mut request = Vec::with_capacity(8 + e.bytes);
    request.extend_from_slice(&ordinal.to_le_bytes());
    request.extend_from_slice(&bytes[..e.bytes]);
    channel
        .send(
            &request,
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .map_err(|_| FsStatus::Io)?;
    let start = crate::syscall::ticks();
    loop {
        match channel.try_recv() {
            Ok(m) => {
                return Ok(Response {
                    bytes: m.bytes,
                    handles: m.handles.iter().map(|h| HandleRef { raw: *h }).collect(),
                });
            }
            Err(kernel_fidl::Status::ErrTimedOut) => {}
            Err(_) => return Err(FsStatus::Io),
        }
        if crate::syscall::ticks().wrapping_sub(start)
            > crate::syscall::frequency().saturating_mul(timeout_seconds)
        {
            return Err(FsStatus::Io);
        }
        crate::yield_now();
    }
}
fn check(s: FsStatus) -> Result<(), FsStatus> {
    if s == FsStatus::Ok { Ok(()) } else { Err(s) }
}
pub fn mount(
    service: Channel,
    block: Channel,
    label: &str,
    key: u64,
    read_only: bool,
) -> Result<Channel, FsStatus> {
    let block = Memory::duplicate(block.0, 1 | 2 | 4 | 32).map_err(|e| {
        crate::log(&alloc::format!("fs: mount duplicate block failed {e:?}\n"));
        FsStatus::Io
    })?;
    let key = Memory::duplicate(key, 1 | 2 | 16).map_err(|e| {
        crate::log(&alloc::format!("fs: mount duplicate key failed {e:?}\n"));
        FsStatus::Io
    })?;
    let r = call_with_timeout(
        service,
        30,
        &FilesystemMountRequest {
            block: HandleRef { raw: block },
            partition_label: label,
            key_vmo: HandleRef { raw: key },
            read_only,
        },
        // A large authenticated BexFS namespace can take several minutes to
        // load under x86 TCG before the root channel is returned.
        420,
    )
    .map_err(|status| {
        crate::log(&alloc::format!("fs: mount call failed {status:?}\n"));
        status
    })?;
    let r = FilesystemMountResponse::decode(&r.bytes, &r.handles).map_err(|_| {
        crate::log("fs: mount decode failed\n");
        FsStatus::Io
    })?;
    if r.status != FsStatus::Ok {
        crate::log(&alloc::format!(
            "fs: mount response status={:?}\n",
            r.status
        ));
    }
    check(r.status)?;
    Ok(Channel(r.root.raw))
}
pub fn sync(service: Channel) -> Result<(), FsStatus> {
    let r = call_with_timeout(
        service,
        32,
        &FilesystemSyncRequest {},
        FS_RPC_TIMEOUT_SECONDS,
    )?;
    check(
        FilesystemSyncResponse::decode(&r.bytes, &r.handles)
            .map_err(|_| FsStatus::Io)?
            .status,
    )
}
pub fn unmount(service: Channel) -> Result<(), FsStatus> {
    let r = call(service, 31, &FilesystemUnmountRequest {}, true)?;
    check(
        FilesystemUnmountResponse::decode(&r.bytes, &r.handles)
            .map_err(|_| FsStatus::Io)?
            .status,
    )
}
pub fn open(root: Channel, path: &str, flags: u32) -> Result<Channel, FsStatus> {
    let (client, server) = Channel::pair().map_err(|_| FsStatus::Io)?;
    call(
        root,
        20,
        &DirectoryOpenRequest {
            path,
            flags: OpenFlags(flags),
            object: HandleRef { raw: server.0 },
        },
        false,
    )?;
    if let Err(e) = attributes(client) {
        let _ = Memory::close(client.0);
        return Err(e);
    }
    Ok(client)
}
pub fn attributes(file: Channel) -> Result<FileAttributes, FsStatus> {
    let r = call(file, 1, &NodeGetAttrRequest {}, true)?;
    let r = NodeGetAttrResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
    check(r.status)?;
    Ok(r.attr)
}

pub fn set_len(file: Channel, len: u64) -> Result<(), FsStatus> {
    let mut attr = attributes(file)?;
    attr.size_bytes = len;
    let r = call(file, 2, &NodeSetAttrRequest { attr }, true)?;
    let r = NodeSetAttrResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
    check(r.status)
}

pub fn read_entries(directory: Channel) -> Result<Vec<OwnedDirEntry>, FsStatus> {
    let r = call(directory, 21, &DirectoryReadEntriesRequest {}, true)?;
    let r = DirectoryReadEntriesResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
    check(r.status)?;
    let mut entries = Vec::new();
    for index in 0..r.entries.len() {
        let entry = r.entries.get(index).map_err(|_| FsStatus::Io)?;
        entries.push(OwnedDirEntry {
            name: String::from(entry.name),
            kind: entry.kind,
            attr: entry.attr,
        });
    }
    Ok(entries)
}
pub fn read(file: Channel, count: u64) -> Result<Vec<u8>, FsStatus> {
    const MAX_READ_CHUNK: u64 = 32768;
    let mut out = Vec::new();
    let mut remaining = count;
    while remaining > 0 {
        let chunk_count = remaining.min(MAX_READ_CHUNK);
        let r = call(file, 10, &FileReadRequest { count: chunk_count }, true)?;
        let r = FileReadResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
        check(r.status)?;
        if r.data.is_empty() {
            break;
        }
        remaining = remaining.saturating_sub(r.data.len() as u64);
        out.extend_from_slice(r.data);
        if r.data.len() as u64 != chunk_count {
            break;
        }
    }
    Ok(out)
}
pub fn write(file: Channel, bytes: &[u8]) -> Result<(), FsStatus> {
    for chunk in bytes.chunks(32768) {
        let r = call(file, 11, &FileWriteRequest { data: chunk }, true)?;
        let r = FileWriteResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
        check(r.status)?;
        if r.written != chunk.len() as u64 {
            return Err(FsStatus::Io);
        }
    }
    Ok(())
}

pub fn unlink(directory: Channel, name: &str) -> Result<(), FsStatus> {
    let r = call(directory, 22, &DirectoryUnlinkRequest { name }, true)?;
    let r = DirectoryUnlinkResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
    check(r.status)
}
pub fn seek(file: Channel, offset: i64) -> Result<(), FsStatus> {
    seek_with_whence(file, offset, 0).map(|_| ())
}

pub fn seek_with_whence(file: Channel, offset: i64, whence: u8) -> Result<u64, FsStatus> {
    let r = call(file, 12, &FileSeekRequest { offset, whence }, true)?;
    let r = FileSeekResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
    check(r.status)?;
    Ok(r.new_offset)
}
pub fn backing(file: Channel) -> Result<(u64, u64), FsStatus> {
    let size = attributes(file)?.size_bytes;
    let r = call(file, 13, &FileGetBackingMemoryRequest {}, true)?;
    let r = FileGetBackingMemoryResponse::decode(&r.bytes, &r.handles).map_err(|_| FsStatus::Io)?;
    check(r.status)?;
    Ok((r.vmo.raw, size))
}
pub fn sync_file(file: Channel) -> Result<(), FsStatus> {
    // BexFS commits the complete inactive namespace slot before publishing its
    // new header. Large package-store namespaces can therefore take longer
    // than the generic RPC deadline on emulated storage.
    let r = call_with_timeout(file, 14, &FileSyncRequest {}, FS_RPC_TIMEOUT_SECONDS)?;
    check(
        FileSyncResponse::decode(&r.bytes, &r.handles)
            .map_err(|_| FsStatus::Io)?
            .status,
    )
}
pub fn mount_archive(
    service: Channel,
    package_file: Channel,
    expected_root: Option<[u8; 32]>,
) -> Result<Channel, FsStatus> {
    let file = Memory::duplicate(package_file.0, 1 | 2 | 4 | 32).map_err(|_| FsStatus::Io)?;
    let root = expected_root.unwrap_or([0; 32]);
    let expected_merkle_root = if expected_root.is_some() {
        &root[..]
    } else {
        &[]
    };
    let mut bytes = vec![0; 65500];
    let mut hs = [archive::HandleRef { raw: 0 }; 32];
    let request = archive::ArchiveManagerMountPackageRequest {
        package_file: archive::HandleRef { raw: file },
        expected_merkle_root,
    };
    let encoded = request
        .encode(&mut bytes, &mut hs)
        .map_err(|_| FsStatus::InvalidArgs)?;
    let m = Rpc(service)
        .call_raw_with_timeout(
            40,
            &bytes[..encoded.bytes],
            &hs[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
            true,
            180,
        )
        .map_err(|_| FsStatus::Io)?;
    let handles = m
        .handles
        .iter()
        .map(|h| archive::HandleRef { raw: *h })
        .collect::<Vec<_>>();
    let response = archive::ArchiveManagerMountPackageResponse::decode(&m.bytes, &handles)
        .map_err(|_| FsStatus::Io)?;
    let status = archive_status(response.status);
    check(status)?;
    Ok(Channel(response.root_dir.raw))
}
pub fn close(file: Channel) -> Result<(), FsStatus> {
    call(file, 3, &NodeCloseRequest {}, false)?;
    Memory::close(file.0).map_err(|_| FsStatus::Io)
}

fn archive_status(status: i32) -> FsStatus {
    match status {
        0 => FsStatus::Ok,
        -1 => FsStatus::NotFound,
        -2 => FsStatus::NotDirectory,
        -3 => FsStatus::IsDirectory,
        -4 => FsStatus::NotEmpty,
        -5 => FsStatus::NoSpace,
        -6 => FsStatus::Io,
        -7 => FsStatus::Corrupt,
        -8 => FsStatus::Locked,
        -9 => FsStatus::ReadOnly,
        -10 => FsStatus::AccessDenied,
        -11 => FsStatus::InvalidArgs,
        -12 => FsStatus::AlreadyExists,
        -13 => FsStatus::BadState,
        _ => FsStatus::Io,
    }
}

pub fn mount_archive_memory(
    service: Channel,
    archive_handle: u64,
    length: u64,
) -> Result<Channel, FsStatus> {
    let handle = Memory::duplicate(archive_handle, 1 | 2 | 16 | 32).map_err(|_| FsStatus::Io)?;
    let mut bytes = vec![0; 65500];
    let mut hs = [archive::HandleRef { raw: 0 }; 32];
    let request = archive::ArchiveManagerMountMemoryRequest {
        archive: archive::HandleRef { raw: handle },
        length,
    };
    let encoded = request
        .encode(&mut bytes, &mut hs)
        .map_err(|_| FsStatus::InvalidArgs)?;
    let m = Rpc(service)
        .call_raw(
            41,
            &bytes[..encoded.bytes],
            &hs[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
            true,
        )
        .map_err(|_| FsStatus::Io)?;
    let handles = m
        .handles
        .iter()
        .map(|h| archive::HandleRef { raw: *h })
        .collect::<Vec<_>>();
    let response = archive::ArchiveManagerMountMemoryResponse::decode(&m.bytes, &handles)
        .map_err(|_| FsStatus::Io)?;
    let status = archive_status(response.status);
    check(status)?;
    Ok(Channel(response.root_dir.raw))
}
