use crate::{Channel, Memory, Rpc};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use fs_fidl::FsStatus;
use vfs_manager_fidl::*;

pub struct Response {
    pub bytes: Vec<u8>,
    pub handles: Vec<HandleRef>,
}

// VFS manager methods can perform nested filesystem RPCs. In particular, a
// directory request can queue behind BexFS's bounded atomic durability commit,
// so the outer request needs headroom beyond the inner filesystem deadline.
const VFS_RPC_TIMEOUT_SECONDS: u64 = 420;
const VFS_PACKAGE_DIRECTORY_TIMEOUT_SECONDS: u64 = 900;
const VFS_UPDATE_ARCHIVE_TIMEOUT_SECONDS: u64 = 660;

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
    let m = Rpc(channel)
        .call_raw_with_timeout(
            ordinal,
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
            reply,
            VFS_RPC_TIMEOUT_SECONDS,
        )
        .map_err(|_| FsStatus::Io)?;
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
            crate::log(&alloc::format!("vfs: RPC ordinal={ordinal} timed out\n"));
            return Err(FsStatus::Io);
        }
        crate::yield_now();
    }
}

fn check(s: FsStatus) -> Result<(), FsStatus> {
    if s == FsStatus::Ok { Ok(()) } else { Err(s) }
}

pub fn initialize_package_store(
    service: Channel,
    block: Channel,
    bexfs: Channel,
    user_bexfs: Channel,
    diskimage: Channel,
    archivefs: Channel,
    key: u64,
    storage_label: &str,
) -> Result<(), FsStatus> {
    let block = Memory::duplicate(block.0, 1 | 2 | 4 | 32).map_err(|e| {
        crate::log(&alloc::format!(
            "vfs-client: duplicate block failed {e:?}\n"
        ));
        FsStatus::Io
    })?;
    let bexfs = Memory::duplicate(bexfs.0, 1 | 2 | 4 | 32).map_err(|e| {
        crate::log(&alloc::format!(
            "vfs-client: duplicate bexfs failed {e:?}\n"
        ));
        FsStatus::Io
    })?;
    let user_bexfs = Memory::duplicate(user_bexfs.0, 1 | 2 | 4 | 32).map_err(|_| FsStatus::Io)?;
    let archivefs = Memory::duplicate(archivefs.0, 1 | 2 | 4 | 32).map_err(|e| {
        crate::log(&alloc::format!(
            "vfs-client: duplicate archivefs failed {e:?}\n"
        ));
        FsStatus::Io
    })?;
    let diskimage = Memory::duplicate(diskimage.0, 1 | 2 | 4 | 32).map_err(|e| {
        crate::log(&alloc::format!(
            "vfs-client: duplicate diskimage failed {e:?}\n"
        ));
        FsStatus::Io
    })?;
    let key = Memory::duplicate(key, 1 | 2 | 16 | 32).map_err(|e| {
        crate::log(&alloc::format!("vfs-client: duplicate key failed {e:?}\n"));
        FsStatus::Io
    })?;
    let r = call_with_timeout(
        service,
        50,
        &VfsManagerInitializePackageStoreRequest {
            block: HandleRef { raw: block },
            bexfs: HandleRef { raw: bexfs },
            user_bexfs: HandleRef { raw: user_bexfs },
            diskimage: HandleRef { raw: diskimage },
            archivefs: HandleRef { raw: archivefs },
            key_vmo: HandleRef { raw: key },
            storage_label,
        },
        // Enclose the filesystem driver's 420-second mount deadline with
        // enough time for archivefs/diskimage setup and RPC scheduling.
        660,
    )
    .map_err(|status| {
        crate::log(&alloc::format!(
            "vfs-client: initialize package store rpc failed {status:?}\n"
        ));
        status
    })?;
    let r = VfsManagerInitializePackageStoreResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))
}

pub fn initialize_tmp_manager(service: Channel, memfs: Channel) -> Result<(), FsStatus> {
    let memfs = Memory::duplicate(memfs.0, 1 | 2 | 4 | 32).map_err(|e| {
        crate::log(&alloc::format!(
            "vfs-client: duplicate memfs failed {e:?}\n"
        ));
        FsStatus::Io
    })?;
    let r = call(
        service,
        63,
        &VfsManagerInitializeTmpManagerRequest {
            memfs: HandleRef { raw: memfs },
        },
        true,
    )
    .map_err(|status| {
        crate::log(&alloc::format!(
            "vfs-client: initialize tmp manager rpc failed {status:?}\n"
        ));
        status
    })?;
    let r = VfsManagerInitializeTmpManagerResponse::decode(&r.bytes, &r.handles).map_err(|_| {
        crate::log("vfs-client: initialize tmp manager decode failed\n");
        FsStatus::Io
    })?;
    if r.status != FsStatus::Ok as i32 {
        crate::log(&alloc::format!(
            "vfs-client: initialize tmp manager status={:?}\n",
            status(r.status)
        ));
    }
    check(status(r.status))
}

pub fn get_tmp_directory(
    service: Channel,
    uid: u64,
    package_id: &str,
    process_name: &str,
) -> Result<Channel, FsStatus> {
    let r = call(
        service,
        64,
        &VfsManagerGetTmpDirectoryRequest {
            uid,
            package_id,
            process_name,
        },
        true,
    )?;
    let r = VfsManagerGetTmpDirectoryResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    if let Err(status) = check(status(r.status)) {
        let _ = Memory::close(r.dir.raw);
        return Err(status);
    }
    Ok(Channel(r.dir.raw))
}

pub fn get_package_directory(service: Channel, package_id: &str) -> Result<Channel, FsStatus> {
    let r = call_with_timeout(
        service,
        51,
        &VfsManagerGetPackageDirectoryRequest { package_id },
        // This composite operation opens the package file through BexFS and
        // then asks archivefs to mount it. Either nested service may first wait
        // behind BexFS's durable commit, so use a deadline that encloses both.
        VFS_PACKAGE_DIRECTORY_TIMEOUT_SECONDS,
    )?;
    let r = VfsManagerGetPackageDirectoryResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    if let Err(status) = check(status(r.status)) {
        let _ = Memory::close(r.dir.raw);
        return Err(status);
    }
    Ok(Channel(r.dir.raw))
}

pub fn read_package_archive(service: Channel, package_id: &str) -> Result<Vec<u8>, FsStatus> {
    let r = call(
        service,
        52,
        &VfsManagerReadPackageArchiveRequest { package_id },
        true,
    )?;
    let r = VfsManagerReadPackageArchiveResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))?;
    let rounded = bexos_boot::page_round(r.archive_len).ok_or(FsStatus::InvalidArgs)?;
    let va = Memory::map(r.archive.raw, rounded, 2).map_err(|_| FsStatus::AccessDenied)?;
    let len = usize::try_from(r.archive_len).map_err(|_| FsStatus::InvalidArgs)?;
    let mut bytes = Vec::with_capacity(len);
    #[cfg(bexos_guest)]
    if len != 0 {
        if Memory::commit_range(bytes.as_mut_ptr() as u64, len as u64).is_err() {
            let _ = Memory::unmap(va, rounded);
            let _ = Memory::close(r.archive.raw);
            return Err(FsStatus::NoSpace);
        }
    }
    unsafe {
        core::ptr::copy_nonoverlapping(va as *const u8, bytes.as_mut_ptr(), len);
        bytes.set_len(len);
    }
    Memory::unmap(va, rounded).map_err(|_| FsStatus::Io)?;
    Memory::close(r.archive.raw).map_err(|_| FsStatus::Io)?;
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PackageArchiveAttributes {
    pub logical_size_bytes: u64,
    pub storage_allocated_bytes: u64,
}

pub fn get_package_archive_attributes(
    service: Channel,
    package_id: &str,
) -> Result<PackageArchiveAttributes, FsStatus> {
    let r = call(
        service,
        67,
        &VfsManagerGetPackageArchiveAttributesRequest { package_id },
        true,
    )?;
    let r = VfsManagerGetPackageArchiveAttributesResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))?;
    Ok(PackageArchiveAttributes {
        logical_size_bytes: r.logical_size_bytes,
        storage_allocated_bytes: r.storage_allocated_bytes,
    })
}

pub fn read_update_archive(service: Channel, archive_id: &str) -> Result<Vec<u8>, FsStatus> {
    let r = call_with_timeout(
        service,
        66,
        &VfsManagerReadUpdateArchiveRequest { archive_id },
        VFS_UPDATE_ARCHIVE_TIMEOUT_SECONDS,
    )?;
    let r = VfsManagerReadUpdateArchiveResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))?;
    let rounded = bexos_boot::page_round(r.archive_len).ok_or(FsStatus::InvalidArgs)?;
    let va = Memory::map(r.archive.raw, rounded, 2).map_err(|_| FsStatus::AccessDenied)?;
    let len = usize::try_from(r.archive_len).map_err(|_| FsStatus::InvalidArgs)?;
    let mut bytes = Vec::with_capacity(len);
    #[cfg(bexos_guest)]
    if len != 0 {
        if Memory::commit_range(bytes.as_mut_ptr() as u64, len as u64).is_err() {
            let _ = Memory::unmap(va, rounded);
            let _ = Memory::close(r.archive.raw);
            return Err(FsStatus::NoSpace);
        }
    }
    unsafe {
        core::ptr::copy_nonoverlapping(va as *const u8, bytes.as_mut_ptr(), len);
        bytes.set_len(len);
    }
    Memory::unmap(va, rounded).map_err(|_| FsStatus::Io)?;
    Memory::close(r.archive.raw).map_err(|_| FsStatus::Io)?;
    Ok(bytes)
}

pub fn write_package_archive(
    service: Channel,
    package_id: &str,
    bytes: &[u8],
) -> Result<(), FsStatus> {
    let archive = Memory::from_bytes(bytes).map_err(|_| FsStatus::NoSpace)?;
    let r = call(
        service,
        53,
        &VfsManagerWritePackageArchiveRequest {
            package_id,
            archive: HandleRef { raw: archive },
            archive_len: bytes.len() as u64,
        },
        true,
    )?;
    let r = VfsManagerWritePackageArchiveResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))
}

pub fn delete_package_archive(service: Channel, package_id: &str) -> Result<(), FsStatus> {
    let r = call(
        service,
        54,
        &VfsManagerDeletePackageArchiveRequest { package_id },
        true,
    )?;
    let r = VfsManagerDeletePackageArchiveResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))
}

pub fn list_package_archives(service: Channel) -> Result<Vec<String>, FsStatus> {
    let r = call(service, 55, &VfsManagerListPackageArchivesRequest {}, true)?;
    let r = VfsManagerListPackageArchivesResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))?;
    let mut packages = Vec::new();
    for index in 0..r.package_ids.len() {
        packages.push(
            r.package_ids
                .get(index)
                .map_err(|_| FsStatus::Io)?
                .to_string(),
        );
    }
    Ok(packages)
}

pub fn get_user_data_directory(
    service: Channel,
    uid: u64,
    package_id: &str,
) -> Result<Channel, FsStatus> {
    let r = call(
        service,
        56,
        &VfsManagerGetUserDataDirectoryRequest { uid, package_id },
        true,
    )?;
    let r = VfsManagerGetUserDataDirectoryResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    if let Err(status) = check(status(r.status)) {
        let _ = Memory::close(r.dir.raw);
        return Err(status);
    }
    Ok(Channel(r.dir.raw))
}

pub fn get_system_data_directory(service: Channel, package_id: &str) -> Result<Channel, FsStatus> {
    let r = call(
        service,
        61,
        &VfsManagerGetSystemDataDirectoryRequest { package_id },
        true,
    )?;
    let r = VfsManagerGetSystemDataDirectoryResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    if let Err(status) = check(status(r.status)) {
        let _ = Memory::close(r.dir.raw);
        return Err(status);
    }
    Ok(Channel(r.dir.raw))
}

pub fn get_user_home_directory(service: Channel, uid: u64) -> Result<Channel, FsStatus> {
    let r = call(
        service,
        62,
        &VfsManagerGetUserHomeDirectoryRequest { uid },
        true,
    )?;
    let r = VfsManagerGetUserHomeDirectoryResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    if let Err(status) = check(status(r.status)) {
        let _ = Memory::close(r.dir.raw);
        return Err(status);
    }
    Ok(Channel(r.dir.raw))
}

pub fn get_shared_vault_directory(
    service: Channel,
    uid: u64,
    domain: &str,
    vault_name: &str,
    system: bool,
) -> Result<Channel, FsStatus> {
    let r = call(
        service,
        65,
        &VfsManagerGetSharedVaultDirectoryRequest {
            uid,
            domain,
            vault_name,
            system,
        },
        true,
    )?;
    let r = VfsManagerGetSharedVaultDirectoryResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    if let Err(status) = check(status(r.status)) {
        let _ = Memory::close(r.dir.raw);
        return Err(status);
    }
    Ok(Channel(r.dir.raw))
}

pub fn create_user_filesystem(service: Channel, uid: u64) -> Result<(), FsStatus> {
    let r = call(
        service,
        57,
        &VfsManagerCreateUserFilesystemRequest { uid },
        true,
    )?;
    let r = VfsManagerCreateUserFilesystemResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))
}

pub fn remove_user_filesystem(service: Channel, uid: u64) -> Result<(), FsStatus> {
    let r = call(
        service,
        58,
        &VfsManagerRemoveUserFilesystemRequest { uid },
        true,
    )?;
    let r = VfsManagerRemoveUserFilesystemResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))
}

pub fn unlock_user_filesystem(service: Channel, uid: u64, ukek_vmo: u64) -> Result<(), FsStatus> {
    let ukek_vmo = Memory::duplicate(ukek_vmo, 1 | 2 | 16 | 32).map_err(|_| FsStatus::Io)?;
    let r = call(
        service,
        59,
        &VfsManagerUnlockUserFilesystemRequest {
            uid,
            ukek_vmo: HandleRef { raw: ukek_vmo },
        },
        true,
    )?;
    let r = VfsManagerUnlockUserFilesystemResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))
}

pub fn lock_user_filesystem(service: Channel, uid: u64) -> Result<(), FsStatus> {
    let r = call(
        service,
        60,
        &VfsManagerLockUserFilesystemRequest { uid },
        true,
    )?;
    let r = VfsManagerLockUserFilesystemResponse::decode(&r.bytes, &r.handles)
        .map_err(|_| FsStatus::Io)?;
    check(status(r.status))
}

fn status(status: i32) -> FsStatus {
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
