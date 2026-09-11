mod migration;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bexos_userspace::live_migration::Source;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Memory, Rpc, Startup, fs, log};
use diskimage_fidl as diskimage;
use diskimage_fidl::{FidlDecode as DiskImageDecode, FidlEncode as DiskImageEncode};
use fs_fidl::FsStatus;
use fs_fidl::{FidlDecode as FsDecode, FidlEncode as FsEncode};
use memfs_fidl as memfs;
use memfs_fidl::{FidlDecode as MemfsDecode, FidlEncode as MemfsEncode};
use migration::Runtime;
use vfs_manager_fidl::*;

use crate::{
    app_data_directory_path, is_staged_generation_archive, package_archive_path,
    shared_vault_directory_path, system_data_directory_path, update_archive_path,
    user_filesystem_path,
};

struct PackageStore {
    archivefs: Channel,
    // Separate raw-volume service; backing files belong to the primary BexFS.
    // This owned channel is included in the existing migration record.
    bexfs: Channel,
    diskimage: Channel,
    storage_root: Channel,
    root: Channel,
    users: Vec<UserMount>,
}

struct UserMount {
    uid: u64,
    root: Channel,
    control: Channel,
    ukek_vmo: u64,
    active_slot: u8,
    virtual_size_bytes: u64,
}

const INITIAL_USER_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const GROW_TRIGGER_PERCENT: u64 = 60;
const GROW_TARGET_PERCENT: u64 = 30;

#[derive(Clone, Copy)]
struct UserImageState {
    generation: u64,
    active_slot: u8,
    candidate_slot: u8,
    phase: ResizePhase,
    virtual_size_bytes: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ResizePhase {
    Stable,
    Preparing,
    Committed,
}

struct TmpManager {
    memfs: Channel,
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).unwrap();
    let state = if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(s) => s,
            Err(_) => bexos_userspace::exit(),
        }
    } else {
        log("vfsd: EL0 storage broker ready\n");
        Startup::ready(control).unwrap();
        Runtime {
            control,
            migration: startup.migration,
            store: None,
            tmp: None,
            clients: Vec::new(),
        }
    };
    serve(state).await
}

async fn serve(mut state: Runtime) -> ! {
    let control = state.control;
    let mut source = Source::new(state.migration);
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace_async::yield_once().await;
            continue;
        }
        let mut pending = None;
        if let Ok(message) = control.try_recv() {
            let binding = message
                .handles
                .first()
                .copied()
                .zip(core::str::from_utf8(&message.bytes).ok())
                .and_then(|(endpoint, metadata)| {
                    ServiceBinding::parse(metadata).map(|binding| (endpoint, binding))
                });
            if let Some((endpoint, binding)) = binding {
                if binding.protocol_is("VfsManager") {
                    state.clients.push(BoundServiceEndpoint::new(
                        Channel(endpoint),
                        binding.method_ordinals,
                    ));
                    source.changed(0);
                } else {
                    let _ = Memory::close(endpoint);
                }
            } else {
                pending = Some((control, message));
            }
        }
        state.clients.retain(|client| {
            if pending.is_some() {
                return true;
            }
            match client.channel.try_recv() {
                Ok(message) => {
                    let ordinal = envelope(&message.bytes).0;
                    if client.allows(ordinal) {
                        pending = Some((client.channel, message));
                    } else {
                        for handle in message.handles {
                            let _ = Memory::close(handle);
                        }
                    }
                    true
                }
                Err(kernel_fidl::Status::ErrTimedOut) => true,
                Err(kernel_fidl::Status::ErrPeerClosed) => false,
                Err(_) => true,
            }
        });
        if let Some((control, m)) = pending {
            source.changed(0);
            let (ordinal, req) = envelope(&m.bytes);
            bexos_trace::trace_counter!(
                bexos_trace::CATEGORY_VFS_IO,
                "vfsd:request_ordinal",
                ordinal as i64
            );
            let hs = refs(&m.handles);
            match ordinal {
                50 => {
                    bexos_trace::trace_scope!(
                        bexos_trace::CATEGORY_VFS_IO,
                        "vfsd:initialize_package_store"
                    );
                    let q = VfsManagerInitializePackageStoreRequest::decode(req, &hs).unwrap();
                    let status = initialize_package_store(q, &mut state.store);
                    reply(
                        control,
                        &VfsManagerInitializePackageStoreResponse {
                            status: status as i32,
                        },
                    );
                }
                51 => {
                    bexos_trace::trace_scope!(
                        bexos_trace::CATEGORY_VFS_IO,
                        "vfsd:get_package_directory"
                    );
                    let q = VfsManagerGetPackageDirectoryRequest::decode(req, &hs).unwrap();
                    let result = get_package_directory(&state.store, q.package_id);
                    let (status, dir) = response_channel(result);
                    reply(
                        control,
                        &VfsManagerGetPackageDirectoryResponse {
                            status: status as i32,
                            dir: HandleRef { raw: dir.0 },
                        },
                    );
                }
                52 => {
                    bexos_trace::trace_scope!(bexos_trace::CATEGORY_VFS_IO, "vfsd:mount_archive");
                    let q = VfsManagerReadPackageArchiveRequest::decode(req, &hs).unwrap();
                    let result = package_archive_backing(&state.store, q.package_id);
                    let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
                    let (archive, archive_len) = match result {
                        Ok((handle, len)) => (handle, len),
                        Err(_) => (0, 0),
                    };
                    reply(
                        control,
                        &VfsManagerReadPackageArchiveResponse {
                            status: if archive == 0 && status == FsStatus::Ok {
                                FsStatus::NoSpace
                            } else {
                                status
                            } as i32,
                            archive: HandleRef { raw: archive },
                            archive_len,
                        },
                    );
                }
                53 => {
                    let q = VfsManagerWritePackageArchiveRequest::decode(req, &hs).unwrap();
                    let status = write_package_archive(
                        &state.store,
                        q.package_id,
                        q.archive.raw,
                        q.archive_len,
                    );
                    reply(
                        control,
                        &VfsManagerWritePackageArchiveResponse {
                            status: status.err().unwrap_or(FsStatus::Ok) as i32,
                        },
                    );
                }
                54 => {
                    let q = VfsManagerDeletePackageArchiveRequest::decode(req, &hs).unwrap();
                    let status = delete_package_archive(&state.store, q.package_id);
                    reply(
                        control,
                        &VfsManagerDeletePackageArchiveResponse {
                            status: status.err().unwrap_or(FsStatus::Ok) as i32,
                        },
                    );
                }
                55 => {
                    let result = list_package_archives(&state.store);
                    let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
                    let package_ids = result.unwrap_or_default();
                    let refs = package_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>();
                    reply(
                        control,
                        &VfsManagerListPackageArchivesResponse {
                            status: status as i32,
                            package_ids: WireStringVector::from_slice(&refs),
                        },
                    );
                }
                56 => {
                    let q = VfsManagerGetUserDataDirectoryRequest::decode(req, &hs).unwrap();
                    let result = get_user_data_directory(&state.store, q.uid, q.package_id);
                    let (status, dir) = response_channel(result);
                    reply(
                        control,
                        &VfsManagerGetUserDataDirectoryResponse {
                            status: status as i32,
                            dir: HandleRef { raw: dir.0 },
                        },
                    );
                }
                57 => {
                    let q = VfsManagerCreateUserFilesystemRequest::decode(req, &hs).unwrap();
                    let status = create_user_filesystem(&state.store, q.uid);
                    reply(
                        control,
                        &VfsManagerCreateUserFilesystemResponse {
                            status: status.err().unwrap_or(FsStatus::Ok) as i32,
                        },
                    );
                }
                58 => {
                    let q = VfsManagerRemoveUserFilesystemRequest::decode(req, &hs).unwrap();
                    let status = remove_user_filesystem(&state.store, q.uid);
                    reply(
                        control,
                        &VfsManagerRemoveUserFilesystemResponse {
                            status: status.err().unwrap_or(FsStatus::Ok) as i32,
                        },
                    );
                }
                59 => {
                    let q = VfsManagerUnlockUserFilesystemRequest::decode(req, &hs).unwrap();
                    let status = unlock_user_filesystem(&mut state.store, q.uid, q.ukek_vmo.raw);
                    reply(
                        control,
                        &VfsManagerUnlockUserFilesystemResponse {
                            status: status.err().unwrap_or(FsStatus::Ok) as i32,
                        },
                    );
                }
                60 => {
                    let q = VfsManagerLockUserFilesystemRequest::decode(req, &hs).unwrap();
                    let status = lock_user_filesystem(&mut state.store, q.uid);
                    reply(
                        control,
                        &VfsManagerLockUserFilesystemResponse {
                            status: status.err().unwrap_or(FsStatus::Ok) as i32,
                        },
                    );
                }
                61 => {
                    let q = VfsManagerGetSystemDataDirectoryRequest::decode(req, &hs).unwrap();
                    let result = get_system_data_directory(&state.store, q.package_id);
                    let (status, dir) = response_channel(result);
                    reply(
                        control,
                        &VfsManagerGetSystemDataDirectoryResponse {
                            status: status as i32,
                            dir: HandleRef { raw: dir.0 },
                        },
                    );
                }
                62 => {
                    let q = VfsManagerGetUserHomeDirectoryRequest::decode(req, &hs).unwrap();
                    let result = get_user_home_directory(&state.store, q.uid);
                    let (status, dir) = response_channel(result);
                    reply(
                        control,
                        &VfsManagerGetUserHomeDirectoryResponse {
                            status: status as i32,
                            dir: HandleRef { raw: dir.0 },
                        },
                    );
                }
                63 => {
                    let q = VfsManagerInitializeTmpManagerRequest::decode(req, &hs).unwrap();
                    let status = initialize_tmp_manager(q, &mut state.tmp);
                    reply(
                        control,
                        &VfsManagerInitializeTmpManagerResponse {
                            status: status as i32,
                        },
                    );
                }
                64 => {
                    let q = VfsManagerGetTmpDirectoryRequest::decode(req, &hs).unwrap();
                    let result = get_tmp_directory(&state.tmp, q.uid, q.package_id, q.process_name);
                    let (status, dir) = response_channel(result);
                    reply(
                        control,
                        &VfsManagerGetTmpDirectoryResponse {
                            status: status as i32,
                            dir: HandleRef { raw: dir.0 },
                        },
                    );
                }
                65 => {
                    let q = VfsManagerGetSharedVaultDirectoryRequest::decode(req, &hs).unwrap();
                    let result = get_shared_vault_directory(
                        &state.store,
                        q.uid,
                        q.domain,
                        q.vault_name,
                        q.system,
                    );
                    let (status, dir) = response_channel(result);
                    reply(
                        control,
                        &VfsManagerGetSharedVaultDirectoryResponse {
                            status: status as i32,
                            dir: HandleRef { raw: dir.0 },
                        },
                    );
                }
                66 => {
                    let q = VfsManagerReadUpdateArchiveRequest::decode(req, &hs).unwrap();
                    let result = update_archive_backing(&state.store, q.archive_id);
                    let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
                    let (archive, archive_len) = match result {
                        Ok((handle, len)) => (handle, len),
                        Err(_) => (0, 0),
                    };
                    reply(
                        control,
                        &VfsManagerReadUpdateArchiveResponse {
                            status: if archive == 0 && status == FsStatus::Ok {
                                FsStatus::NoSpace
                            } else {
                                status
                            } as i32,
                            archive: HandleRef { raw: archive },
                            archive_len,
                        },
                    );
                }
                67 => {
                    let q = VfsManagerGetPackageArchiveAttributesRequest::decode(req, &hs).unwrap();
                    let result = package_archive_attributes(&state.store, q.package_id);
                    let status = result.as_ref().err().copied().unwrap_or(FsStatus::Ok);
                    let attrs = result.unwrap_or_default();
                    reply(
                        control,
                        &VfsManagerGetPackageArchiveAttributesResponse {
                            status: status as i32,
                            logical_size_bytes: attrs.logical_size_bytes,
                            storage_allocated_bytes: attrs.storage_allocated_bytes,
                        },
                    );
                }
                _ => panic!("unknown vfsd ordinal"),
            }
        }
        maybe_resize_users(&mut state.store);
        bexos_userspace_async::yield_once().await;
    }
}

fn initialize_package_store(
    q: VfsManagerInitializePackageStoreRequest<'_>,
    store: &mut Option<PackageStore>,
) -> FsStatus {
    if store.is_some() {
        let _ = Memory::close(q.block.raw);
        let _ = Memory::close(q.bexfs.raw);
        let _ = Memory::close(q.user_bexfs.raw);
        let _ = Memory::close(q.diskimage.raw);
        let _ = Memory::close(q.archivefs.raw);
        let _ = Memory::close(q.key_vmo.raw);
        return FsStatus::AlreadyExists;
    }

    log("vfsd: package store mount request begin\n");
    let mounted = fs::mount(
        Channel(q.bexfs.raw),
        Channel(q.block.raw),
        q.storage_label,
        q.key_vmo.raw,
        false,
    );
    let _ = Memory::close(q.block.raw);
    let _ = Memory::close(q.key_vmo.raw);

    let _ = Memory::close(q.bexfs.raw);

    let root = match mounted {
        Ok(root) => root,
        Err(status) => {
            log(&alloc::format!(
                "vfsd: package store mount failed status={status:?}\n"
            ));
            let _ = Memory::close(q.user_bexfs.raw);
            let _ = Memory::close(q.diskimage.raw);
            let _ = Memory::close(q.archivefs.raw);
            return status;
        }
    };
    log("vfsd: package store mount returned root\n");
    *store = Some(PackageStore {
        archivefs: Channel(q.archivefs.raw),
        bexfs: Channel(q.user_bexfs.raw),
        diskimage: Channel(q.diskimage.raw),
        storage_root: root,
        root,
        users: Vec::new(),
    });
    FsStatus::Ok
}

fn initialize_tmp_manager(
    q: VfsManagerInitializeTmpManagerRequest,
    tmp: &mut Option<TmpManager>,
) -> FsStatus {
    if tmp.is_some() {
        let _ = Memory::close(q.memfs.raw);
        return FsStatus::AlreadyExists;
    }
    log("vfsd: tmp manager initialized\n");
    *tmp = Some(TmpManager {
        memfs: Channel(q.memfs.raw),
    });
    FsStatus::Ok
}

fn get_tmp_directory(
    tmp: &Option<TmpManager>,
    uid: u64,
    package_id: &str,
    process_name: &str,
) -> Result<Channel, FsStatus> {
    let _ = crate::user_filesystem_path(uid)?;
    let _ = crate::app_data_directory_path(uid, package_id)?;
    validate_process_name(process_name)?;
    let tmp = tmp.as_ref().ok_or(FsStatus::BadState)?;
    let mut bytes = vec![0; 65500];
    let mut hs = [memfs::HandleRef { raw: 0 }; 16];
    let request = memfs::MemfsManagerCreateTmpDirectoryRequest {
        uid,
        package_id,
        process_name,
    };
    let encoded = request
        .encode(&mut bytes, &mut hs)
        .map_err(|_| FsStatus::InvalidArgs)?;
    let response = Rpc(tmp.memfs)
        .call_raw(
            70,
            &bytes[..encoded.bytes],
            &hs[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
            true,
        )
        .map_err(|_| FsStatus::Io)?;
    let refs = response
        .handles
        .iter()
        .map(|h| memfs::HandleRef { raw: *h })
        .collect::<Vec<_>>();
    let response = memfs::MemfsManagerCreateTmpDirectoryResponse::decode(&response.bytes, &refs)
        .map_err(|_| FsStatus::Io)?;
    let status = status(response.status);
    if status != FsStatus::Ok {
        let _ = Memory::close(response.root_dir.raw);
        return Err(status);
    }
    Ok(Channel(response.root_dir.raw))
}

fn get_package_directory(
    store: &Option<PackageStore>,
    package_id: &str,
) -> Result<Channel, FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let archive_path = package_archive_path(package_id)?;
    log(&alloc::format!(
        "vfsd: package directory open package={package_id} path={archive_path}\n"
    ));
    let archive_file = fs::open(store.root, &archive_path, 1)?;
    let mounted = fs::mount_archive(store.archivefs, archive_file, None);
    let _ = Memory::close(archive_file.0);
    mounted
}

fn package_archive_backing(
    store: &Option<PackageStore>,
    package_id: &str,
) -> Result<(u64, u64), FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let archive_path = package_archive_path(package_id)?;
    let archive_file = fs::open(store.root, &archive_path, 1)?;
    let backing = fs::backing(archive_file);
    let close = Memory::close(archive_file.0).map_err(|_| FsStatus::Io);
    let backing = backing?;
    close?;
    Ok(backing)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PackageArchiveAttributes {
    logical_size_bytes: u64,
    storage_allocated_bytes: u64,
}

fn package_archive_attributes(
    store: &Option<PackageStore>,
    package_id: &str,
) -> Result<PackageArchiveAttributes, FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let archive_path = package_archive_path(package_id)?;
    let archive_file = fs::open(store.root, &archive_path, 1)?;
    let attrs = fs::attributes(archive_file);
    let close = Memory::close(archive_file.0).map_err(|_| FsStatus::Io);
    let attrs = attrs?;
    close?;
    Ok(PackageArchiveAttributes {
        logical_size_bytes: attrs.size_bytes,
        storage_allocated_bytes: attrs.storage_allocated_bytes,
    })
}

fn update_archive_backing(
    store: &Option<PackageStore>,
    archive_id: &str,
) -> Result<(u64, u64), FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let archive_path = update_archive_path(archive_id)?;
    log(&alloc::format!(
        "vfsd: read update archive id={} path={}\n",
        archive_id,
        archive_path
    ));
    let root = Memory::duplicate(store.root.0, 1 | 2 | 4 | 32)
        .map(Channel)
        .map_err(|_| {
            log("vfsd: read update archive root duplicate failed\n");
            FsStatus::Io
        })?;
    let archive_file = match fs::open(root, &archive_path, 1) {
        Ok(file) => file,
        Err(status) => {
            let _ = Memory::close(root.0);
            log(&alloc::format!(
                "vfsd: read update archive open failed status={status:?}\n"
            ));
            return Err(status);
        }
    };
    Memory::close(root.0).map_err(|_| {
        let _ = Memory::close(archive_file.0);
        log("vfsd: read update archive root close failed\n");
        FsStatus::Io
    })?;
    let backing = fs::backing(archive_file).map_err(|status| {
        log(&alloc::format!(
            "vfsd: read update archive backing failed status={status:?}\n"
        ));
        status
    })?;
    Memory::close(archive_file.0).map_err(|_| {
        log("vfsd: read update archive close failed\n");
        FsStatus::Io
    })?;
    log(&alloc::format!(
        "vfsd: read update archive complete bytes={}\n",
        backing.1
    ));
    Ok(backing)
}

fn write_package_archive(
    store: &Option<PackageStore>,
    package_id: &str,
    archive: u64,
    archive_len: u64,
) -> Result<(), FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let archive_path = package_archive_path(package_id)?;
    let rounded = bexos_boot::page_round(archive_len).ok_or(FsStatus::InvalidArgs)?;
    let va = Memory::map(archive, rounded, 2).map_err(|_| FsStatus::AccessDenied)?;
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, archive_len as usize) };
    if let Ok((parent_path, _)) = split_parent_leaf(&archive_path) {
        let parent = ensure_directory_path(store.root, parent_path)?;
        Memory::close(parent.0).map_err(|_| FsStatus::Io)?;
    }
    let archive_file = match fs::open(store.root, &archive_path, 1 | 2 | 8 | 16) {
        Ok(file) => file,
        Err(status) => {
            let _ = Memory::unmap(va, rounded);
            let _ = Memory::close(archive);
            return Err(status);
        }
    };
    let staged_generation = is_staged_generation_archive(package_id);
    // Appd writes a generation archive before cutover, then commits that file
    // and its activated registry record in the same BexFS namespace sync. Do
    // not start a separate full-namespace commit here: on emulated storage it
    // can consume the complete migration preparation window, while a crash
    // before appd's later commit still leaves both old durable records active.
    let result = fs::write(archive_file, bytes).and_then(|_| {
        if staged_generation {
            Ok(())
        } else {
            fs::sync_file(archive_file)
        }
    });
    Memory::unmap(va, rounded).map_err(|_| FsStatus::Io)?;
    Memory::close(archive).map_err(|_| FsStatus::Io)?;
    let close = fs::close(archive_file);
    result?;
    close?;
    if staged_generation {
        Ok(())
    } else {
        fs::sync(store.root)
    }
}

fn delete_package_archive(store: &Option<PackageStore>, package_id: &str) -> Result<(), FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let archive_path = package_archive_path(package_id)?;
    let (parent_path, leaf) = split_parent_leaf(&archive_path)?;
    let parent = fs::open(store.root, parent_path, 1 | 2 | 32)?;
    let result = fs::unlink(parent, leaf).and_then(|()| fs::sync(parent));
    let closed = Memory::close(parent.0).map_err(|_| FsStatus::Io);
    result?;
    closed
}

fn list_package_archives(store: &Option<PackageStore>) -> Result<Vec<String>, FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let pkg = fs::open(store.root, "pkg", 1 | 32)?;
    let mut packages = Vec::new();
    let entries = fs::read_entries(pkg);
    let close = Memory::close(pkg.0).map_err(|_| FsStatus::Io);
    for entry in entries? {
        if entry.kind == fs_fidl::NodeKind::File {
            if let Some(package) = entry.name.strip_suffix(".bex") {
                packages.push(package.into());
            }
        }
    }
    close?;
    Ok(packages)
}

fn get_user_data_directory(
    store: &Option<PackageStore>,
    uid: u64,
    package_id: &str,
) -> Result<Channel, FsStatus> {
    let root = user_root(store, uid)?;
    let app_dir = user_app_data_path(package_id)?;
    ensure_directory_path(root, &app_dir)
}

fn get_system_data_directory(
    store: &Option<PackageStore>,
    package_id: &str,
) -> Result<Channel, FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let system_dir = system_data_directory_path(package_id)?;
    ensure_directory_path(store.storage_root, &system_dir)
}

fn get_user_home_directory(store: &Option<PackageStore>, uid: u64) -> Result<Channel, FsStatus> {
    let root = user_root(store, uid)?;
    Memory::duplicate(root.0, 1 | 2 | 4 | 32)
        .map(Channel)
        .map_err(|_| FsStatus::Io)
}

fn get_shared_vault_directory(
    store: &Option<PackageStore>,
    uid: u64,
    domain: &str,
    vault_name: &str,
    system: bool,
) -> Result<Channel, FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let path = shared_vault_directory_path(uid, domain, vault_name, system)?;
    if system {
        ensure_directory_path(store.storage_root, &path)
    } else {
        let root = store
            .users
            .iter()
            .find(|user| user.uid == uid)
            .map(|user| user.root)
            .ok_or(FsStatus::Locked)?;
        let relative = path
            .strip_prefix(&alloc::format!("data/users/{uid}/"))
            .ok_or(FsStatus::InvalidArgs)?;
        ensure_directory_path(root, relative)
    }
}

fn create_user_filesystem(store: &Option<PackageStore>, uid: u64) -> Result<(), FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let _ = user_filesystem_path(uid)?;
    let path = user_vault_path(uid)?;
    let dir = ensure_directory_path(store.storage_root, &path)?;
    fs::close(dir)
}

fn remove_user_filesystem(store: &Option<PackageStore>, uid: u64) -> Result<(), FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let _ = user_filesystem_path(uid)?;
    let path = user_vault_path(uid)?;
    remove_directory_tree(store.storage_root, &path)
}

fn unlock_user_filesystem(
    store: &mut Option<PackageStore>,
    uid: u64,
    ukek_vmo: u64,
) -> Result<(), FsStatus> {
    let store_ref = store.as_mut().ok_or(FsStatus::BadState)?;
    let _ = user_filesystem_path(uid)?;
    if store_ref.users.iter().any(|user| user.uid == uid) {
        let _ = Memory::close(ukek_vmo);
        return Ok(());
    }
    ensure_user_vault(store_ref.storage_root, uid)?;
    let state = read_user_image_state(store_ref.storage_root, uid).unwrap_or(UserImageState {
        generation: 0,
        active_slot: 0,
        candidate_slot: 1,
        phase: ResizePhase::Stable,
        virtual_size_bytes: INITIAL_USER_IMAGE_BYTES,
    });
    let state = recover_user_image_state(state);
    let slot_path = user_slot_path(uid, state.active_slot)?;
    let mounted = match fs::open(store_ref.storage_root, &slot_path, 1 | 2 | 8) {
        Ok(file) if state.generation == 0 => {
            let block = diskimage_create(
                store_ref.diskimage,
                file,
                ukek_vmo,
                user_image_uuid(uid, state.active_slot),
                state.virtual_size_bytes,
            )?;
            mount_user_volume(store_ref, uid, block, ukek_vmo, true, state)
        }
        Ok(file) => {
            let block = diskimage_open(store_ref.diskimage, file, ukek_vmo, false)?;
            mount_user_volume(store_ref, uid, block, ukek_vmo, false, state)
        }
        Err(status) => {
            let _ = Memory::close(ukek_vmo);
            Err(status)
        }
    }?;
    store_ref.users.push(mounted);
    Ok(())
}

fn lock_user_filesystem(store: &mut Option<PackageStore>, uid: u64) -> Result<(), FsStatus> {
    let store = store.as_mut().ok_or(FsStatus::BadState)?;
    let _ = user_filesystem_path(uid)?;
    if let Some(index) = store.users.iter().position(|user| user.uid == uid) {
        let mount = store.users.remove(index);
        let status = mounted_unmount(mount.control);
        let _ = Memory::close(mount.root.0);
        let _ = Memory::close(mount.control.0);
        let _ = Memory::close(mount.ukek_vmo);
        return status;
    }
    Ok(())
}

fn remove_directory_tree(root: Channel, path: &str) -> Result<(), FsStatus> {
    let (parent_path, leaf) = split_parent_leaf(path)?;
    let parent = ensure_directory_path(root, parent_path)?;
    let target = match fs::open(parent, leaf, 1 | 2 | 32) {
        Ok(target) => target,
        Err(status) => {
            let _ = Memory::close(parent.0);
            return Err(status);
        }
    };
    let result = remove_directory_contents(target).and_then(|()| fs::unlink(parent, leaf));
    let close_target = Memory::close(target.0).map_err(|_| FsStatus::Io);
    let close_parent = Memory::close(parent.0).map_err(|_| FsStatus::Io);
    result?;
    close_target?;
    close_parent
}

fn remove_directory_contents(directory: Channel) -> Result<(), FsStatus> {
    let entries = fs::read_entries(directory)?;
    for entry in entries {
        if entry.kind == fs_fidl::NodeKind::Directory {
            let child = fs::open(directory, &entry.name, 1 | 2 | 32)?;
            let result = remove_directory_contents(child);
            let close = Memory::close(child.0).map_err(|_| FsStatus::Io);
            result?;
            close?;
        }
        fs::unlink(directory, &entry.name)?;
    }
    Ok(())
}

fn user_root(store: &Option<PackageStore>, uid: u64) -> Result<Channel, FsStatus> {
    let store = store.as_ref().ok_or(FsStatus::BadState)?;
    let _ = user_filesystem_path(uid)?;
    store
        .users
        .iter()
        .find(|user| user.uid == uid)
        .map(|user| user.root)
        .ok_or(FsStatus::Locked)
}

fn user_app_data_path(package_id: &str) -> Result<String, FsStatus> {
    let path = app_data_directory_path(1, package_id)?;
    path.strip_prefix("data/users/1/")
        .map(ToString::to_string)
        .ok_or(FsStatus::InvalidArgs)
}

fn user_vault_path(uid: u64) -> Result<String, FsStatus> {
    let _ = user_filesystem_path(uid)?;
    Ok(alloc::format!("vault/users/{uid}"))
}

fn user_state_path(uid: u64) -> Result<String, FsStatus> {
    Ok(alloc::format!("{}/state.bin", user_vault_path(uid)?))
}

fn user_slot_path(uid: u64, slot: u8) -> Result<String, FsStatus> {
    let name = match slot {
        0 => "slot_a.img",
        1 => "slot_b.img",
        _ => return Err(FsStatus::InvalidArgs),
    };
    Ok(alloc::format!("{}/{name}", user_vault_path(uid)?))
}

fn ensure_user_vault(root: Channel, uid: u64) -> Result<(), FsStatus> {
    let dir = ensure_directory_path(root, &user_vault_path(uid)?)?;
    fs::close(dir)
}

fn read_user_image_state(root: Channel, uid: u64) -> Result<UserImageState, FsStatus> {
    let file = fs::open(root, &user_state_path(uid)?, 1)?;
    let size = fs::attributes(file)?.size_bytes;
    let bytes = fs::read(file, size)?;
    fs::close(file)?;
    decode_user_image_state(&bytes)
}

fn write_user_image_state(root: Channel, uid: u64, state: UserImageState) -> Result<(), FsStatus> {
    let bytes = encode_user_image_state(state);
    let file = fs::open(root, &user_state_path(uid)?, 1 | 2 | 8 | 16)?;
    let write = fs::write(file, &bytes);
    let sync = fs::sync_file(file);
    let close = fs::close(file);
    write?;
    sync?;
    close
}

fn encode_user_image_state(state: UserImageState) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BEXUS001");
    bytes.extend_from_slice(&state.generation.to_le_bytes());
    bytes.push(state.active_slot);
    bytes.push(state.candidate_slot);
    bytes.push(match state.phase {
        ResizePhase::Stable => 0,
        ResizePhase::Preparing => 1,
        ResizePhase::Committed => 2,
    });
    bytes.extend_from_slice(&[0; 5]);
    bytes.extend_from_slice(&state.virtual_size_bytes.to_le_bytes());
    let checksum = bexos_migration::codec::checksum(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    bytes
}

fn decode_user_image_state(bytes: &[u8]) -> Result<UserImageState, FsStatus> {
    if bytes.len() != 40 || bytes.get(..8) != Some(b"BEXUS001") {
        return Err(FsStatus::Corrupt);
    }
    let expected = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
    if bexos_migration::codec::checksum(&bytes[..32]) != expected {
        return Err(FsStatus::Corrupt);
    }
    let generation = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let active_slot = bytes[16];
    let candidate_slot = bytes[17];
    let phase = match bytes[18] {
        0 => ResizePhase::Stable,
        1 => ResizePhase::Preparing,
        2 => ResizePhase::Committed,
        _ => return Err(FsStatus::Corrupt),
    };
    let virtual_size_bytes = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    if active_slot > 1 || candidate_slot > 1 || virtual_size_bytes == 0 {
        return Err(FsStatus::Corrupt);
    }
    Ok(UserImageState {
        generation,
        active_slot,
        candidate_slot,
        phase,
        virtual_size_bytes,
    })
}

fn recover_user_image_state(mut state: UserImageState) -> UserImageState {
    match state.phase {
        ResizePhase::Stable => state,
        ResizePhase::Preparing => {
            state.phase = ResizePhase::Stable;
            state.candidate_slot = 1 - state.active_slot;
            state
        }
        ResizePhase::Committed => {
            state.active_slot = state.candidate_slot;
            state.candidate_slot = 1 - state.active_slot;
            state.phase = ResizePhase::Stable;
            state
        }
    }
}

fn diskimage_create(
    manager: Channel,
    file: Channel,
    ukek_vmo: u64,
    image_uuid: [u8; 16],
    virtual_size_bytes: u64,
) -> Result<Channel, FsStatus> {
    let key = Memory::duplicate(ukek_vmo, 1 | 2 | 16 | 32).map_err(|_| FsStatus::Io)?;
    let response = diskimage_call(
        manager,
        2,
        &diskimage::DiskImageManagerCreateEncryptedRequest {
            file_handle: diskimage::HandleRef { raw: file.0 },
            key_vmo: diskimage::HandleRef { raw: key },
            image_uuid,
            virtual_size_bytes,
        },
    )?;
    let handles = diskimage_response_handles(&response.handles);
    let response =
        diskimage::DiskImageManagerCreateEncryptedResponse::decode(&response.bytes, &handles)
            .map_err(|_| FsStatus::Io)?;
    if response.status != diskimage::Status::Ok {
        return Err(diskimage_status(response.status));
    }
    let endpoint = response.block_device.ok_or(FsStatus::Io)?.endpoint;
    if endpoint.raw == 0 {
        return Err(FsStatus::Io);
    }
    Ok(Channel(endpoint.raw))
}

fn diskimage_open(
    manager: Channel,
    file: Channel,
    ukek_vmo: u64,
    read_only: bool,
) -> Result<Channel, FsStatus> {
    let key = Memory::duplicate(ukek_vmo, 1 | 2 | 16 | 32).map_err(|_| FsStatus::Io)?;
    let response = diskimage_call(
        manager,
        3,
        &diskimage::DiskImageManagerOpenEncryptedRequest {
            file_handle: diskimage::HandleRef { raw: file.0 },
            key_vmo: diskimage::HandleRef { raw: key },
            read_only,
        },
    )?;
    let handles = diskimage_response_handles(&response.handles);
    let response =
        diskimage::DiskImageManagerOpenEncryptedResponse::decode(&response.bytes, &handles)
            .map_err(|_| FsStatus::Io)?;
    if response.status != diskimage::Status::Ok {
        return Err(diskimage_status(response.status));
    }
    let endpoint = response.block_device.ok_or(FsStatus::Io)?.endpoint;
    if endpoint.raw == 0 {
        return Err(FsStatus::Io);
    }
    Ok(Channel(endpoint.raw))
}

fn mount_user_volume(
    store: &PackageStore,
    uid: u64,
    block: Channel,
    ukek_vmo: u64,
    format: bool,
    mut state: UserImageState,
) -> Result<UserMount, FsStatus> {
    let key = Memory::duplicate(ukek_vmo, 1 | 2 | 16 | 32).map_err(|_| FsStatus::Io)?;
    let label = user_volume_label(uid)?;
    let response = if format {
        fs_call(
            store.bexfs,
            33,
            &fs_fidl::FilesystemFormatAndMountRawRequest {
                block: fs_fidl::HandleRef { raw: block.0 },
                label: &label,
                key_vmo: fs_fidl::HandleRef { raw: key },
                read_only: false,
            },
        )?
    } else {
        fs_call(
            store.bexfs,
            34,
            &fs_fidl::FilesystemMountRawRequest {
                block: fs_fidl::HandleRef { raw: block.0 },
                label: &label,
                key_vmo: fs_fidl::HandleRef { raw: key },
                read_only: false,
            },
        )?
    };
    if format {
        let handles = fs_response_handles(&response.handles);
        let response =
            fs_fidl::FilesystemFormatAndMountRawResponse::decode(&response.bytes, &handles)
                .map_err(|_| FsStatus::Io)?;
        if response.status != FsStatus::Ok {
            return Err(response.status);
        }
        state.generation = 1;
        write_user_image_state(store.storage_root, uid, state)?;
        Ok(UserMount {
            uid,
            root: Channel(response.root.raw),
            control: Channel(response.control.raw),
            ukek_vmo,
            active_slot: state.active_slot,
            virtual_size_bytes: state.virtual_size_bytes,
        })
    } else {
        let handles = fs_response_handles(&response.handles);
        let response = fs_fidl::FilesystemMountRawResponse::decode(&response.bytes, &handles)
            .map_err(|_| FsStatus::Io)?;
        if response.status != FsStatus::Ok {
            return Err(response.status);
        }
        Ok(UserMount {
            uid,
            root: Channel(response.root.raw),
            control: Channel(response.control.raw),
            ukek_vmo,
            active_slot: state.active_slot,
            virtual_size_bytes: state.virtual_size_bytes,
        })
    }
}

fn mounted_unmount(control: Channel) -> Result<(), FsStatus> {
    let response = fs_call(control, 44, &fs_fidl::MountedFilesystemUnmountRequest {})?;
    let handles = fs_response_handles(&response.handles);
    let response = fs_fidl::MountedFilesystemUnmountResponse::decode(&response.bytes, &handles)
        .map_err(|_| FsStatus::Io)?;
    if response.status == FsStatus::Ok {
        Ok(())
    } else {
        Err(response.status)
    }
}

fn maybe_resize_users(store: &mut Option<PackageStore>) {
    let Some(store) = store.as_mut() else {
        return;
    };
    for index in 0..store.users.len() {
        if let Err(status) = maybe_resize_user(store, index) {
            log(&alloc::format!(
                "vfsd: user resize skipped status={status:?}\n"
            ));
        }
    }
}

fn maybe_resize_user(store: &mut PackageStore, index: usize) -> Result<(), FsStatus> {
    let capacity = mounted_capacity(store.users[index].control)?;
    if capacity.storage_allocated_bytes.saturating_mul(100)
        < capacity
            .virtual_size_bytes
            .saturating_mul(GROW_TRIGGER_PERCENT)
    {
        return Ok(());
    }
    let mut next_size = store.users[index]
        .virtual_size_bytes
        .max(INITIAL_USER_IMAGE_BYTES);
    while capacity.storage_allocated_bytes.saturating_mul(100)
        > next_size.saturating_mul(GROW_TARGET_PERCENT)
    {
        next_size = next_size.checked_mul(2).ok_or(FsStatus::NoSpace)?;
    }
    if next_size <= store.users[index].virtual_size_bytes {
        return Ok(());
    }
    let uid = store.users[index].uid;
    let inactive = 1 - store.users[index].active_slot;
    write_user_image_state(
        store.storage_root,
        uid,
        UserImageState {
            generation: capacity.generation,
            active_slot: store.users[index].active_slot,
            candidate_slot: inactive,
            phase: ResizePhase::Preparing,
            virtual_size_bytes: next_size,
        },
    )?;
    let slot_path = user_slot_path(uid, inactive)?;
    let file = fs::open(store.storage_root, &slot_path, 1 | 2 | 8 | 16)?;
    let block = diskimage_create(
        store.diskimage,
        file,
        store.users[index].ukek_vmo,
        user_image_uuid(uid, inactive),
        next_size,
    )?;
    let label = user_volume_label(uid)?;
    let token = match mounted_prepare_resize(
        store.users[index].control,
        block,
        &label,
        store.users[index].ukek_vmo,
    ) {
        Ok(token) => token,
        Err(status) => return Err(status),
    };
    write_user_image_state(
        store.storage_root,
        uid,
        UserImageState {
            generation: capacity.generation.saturating_add(1),
            active_slot: inactive,
            candidate_slot: 1 - inactive,
            phase: ResizePhase::Committed,
            virtual_size_bytes: next_size,
        },
    )?;
    mounted_commit_resize(store.users[index].control, token)?;
    store.users[index].active_slot = inactive;
    store.users[index].virtual_size_bytes = next_size;
    write_user_image_state(
        store.storage_root,
        uid,
        UserImageState {
            generation: capacity.generation.saturating_add(1),
            active_slot: inactive,
            candidate_slot: 1 - inactive,
            phase: ResizePhase::Stable,
            virtual_size_bytes: next_size,
        },
    )?;
    let old_slot = user_slot_path(uid, 1 - inactive)?;
    let _ = fs::unlink(store.storage_root, &old_slot);
    Ok(())
}

struct MountedCapacity {
    generation: u64,
    virtual_size_bytes: u64,
    storage_allocated_bytes: u64,
}

fn mounted_capacity(control: Channel) -> Result<MountedCapacity, FsStatus> {
    let response = fs_call(
        control,
        40,
        &fs_fidl::MountedFilesystemGetCapacityRequest {},
    )?;
    let handles = fs_response_handles(&response.handles);
    let response = fs_fidl::MountedFilesystemGetCapacityResponse::decode(&response.bytes, &handles)
        .map_err(|_| FsStatus::Io)?;
    if response.status != FsStatus::Ok {
        return Err(response.status);
    }
    Ok(MountedCapacity {
        generation: 0,
        virtual_size_bytes: response.virtual_size_bytes,
        storage_allocated_bytes: response.storage_allocated_bytes,
    })
}

fn mounted_prepare_resize(
    control: Channel,
    block: Channel,
    label: &str,
    ukek_vmo: u64,
) -> Result<u64, FsStatus> {
    let key = Memory::duplicate(ukek_vmo, 1 | 2 | 16 | 32).map_err(|_| FsStatus::Io)?;
    let response = fs_call(
        control,
        41,
        &fs_fidl::MountedFilesystemPrepareResizeRequest {
            block: fs_fidl::HandleRef { raw: block.0 },
            label,
            key_vmo: fs_fidl::HandleRef { raw: key },
        },
    )?;
    let handles = fs_response_handles(&response.handles);
    let response =
        fs_fidl::MountedFilesystemPrepareResizeResponse::decode(&response.bytes, &handles)
            .map_err(|_| FsStatus::Io)?;
    if response.status == FsStatus::Ok {
        Ok(response.token)
    } else {
        Err(response.status)
    }
}

fn mounted_commit_resize(control: Channel, token: u64) -> Result<(), FsStatus> {
    let response = fs_call(
        control,
        42,
        &fs_fidl::MountedFilesystemCommitResizeRequest { token },
    )?;
    let handles = fs_response_handles(&response.handles);
    let response =
        fs_fidl::MountedFilesystemCommitResizeResponse::decode(&response.bytes, &handles)
            .map_err(|_| FsStatus::Io)?;
    if response.status == FsStatus::Ok {
        Ok(())
    } else {
        Err(response.status)
    }
}

fn user_volume_label(uid: u64) -> Result<String, FsStatus> {
    let _ = user_filesystem_path(uid)?;
    Ok(alloc::format!("USER{uid}"))
}

fn user_image_uuid(uid: u64, slot: u8) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&uid.to_le_bytes());
    out[8] = slot;
    out
}

fn diskimage_status(status: diskimage::Status) -> FsStatus {
    match status {
        diskimage::Status::Ok => FsStatus::Ok,
        diskimage::Status::ErrAccessDenied => FsStatus::AccessDenied,
        diskimage::Status::ErrInvalidArgs | diskimage::Status::ErrInvalidHandle => {
            FsStatus::InvalidArgs
        }
        diskimage::Status::ErrNoMemory => FsStatus::NoSpace,
        diskimage::Status::ErrBufferTooSmall => FsStatus::NoSpace,
        diskimage::Status::ErrPeerClosed
        | diskimage::Status::ErrTimedOut
        | diskimage::Status::ErrAlreadyExists => FsStatus::Io,
    }
}

fn diskimage_call<Q: DiskImageEncode>(
    channel: Channel,
    ordinal: u64,
    q: &Q,
) -> Result<bexos_userspace::Message, FsStatus> {
    let mut bytes = vec![0; 65500];
    let mut handles = [diskimage::HandleRef { raw: 0 }; 8];
    let encoded = q
        .encode(&mut bytes, &mut handles)
        .map_err(|_| FsStatus::InvalidArgs)?;
    Rpc(channel)
        .call_raw_with_timeout(
            ordinal,
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
            true,
            300,
        )
        .map_err(|_| FsStatus::Io)
}

fn diskimage_response_handles(handles: &[u64]) -> Vec<diskimage::HandleRef> {
    handles
        .iter()
        .map(|handle| diskimage::HandleRef { raw: *handle })
        .collect()
}

fn fs_call<Q: FsEncode>(
    channel: Channel,
    ordinal: u64,
    q: &Q,
) -> Result<bexos_userspace::Message, FsStatus> {
    let mut bytes = vec![0; 65500];
    let mut handles = [fs_fidl::HandleRef { raw: 0 }; 8];
    let encoded = q
        .encode(&mut bytes, &mut handles)
        .map_err(|_| FsStatus::InvalidArgs)?;
    Rpc(channel)
        .call_raw_with_timeout(
            ordinal,
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
            true,
            300,
        )
        .map_err(|_| FsStatus::Io)
}

fn fs_response_handles(handles: &[u64]) -> Vec<fs_fidl::HandleRef> {
    handles
        .iter()
        .map(|handle| fs_fidl::HandleRef { raw: *handle })
        .collect()
}

fn split_parent_leaf(path: &str) -> Result<(&str, &str), FsStatus> {
    let Some((parent, leaf)) = path.rsplit_once('/') else {
        return Err(FsStatus::InvalidArgs);
    };
    if parent.is_empty() || leaf.is_empty() {
        return Err(FsStatus::InvalidArgs);
    }
    Ok((parent, leaf))
}

fn ensure_directory_path(root: Channel, path: &str) -> Result<Channel, FsStatus> {
    let mut current = Memory::duplicate(root.0, 1 | 2 | 4 | 32)
        .map(Channel)
        .map_err(|_| FsStatus::Io)?;
    for component in path.split('/') {
        let next = match fs::open(current, component, 1 | 2 | 8 | 32) {
            Ok(next) => next,
            Err(status) => {
                let _ = Memory::close(current.0);
                return Err(status);
            }
        };
        if Memory::close(current.0).is_err() {
            let _ = Memory::close(next.0);
            return Err(FsStatus::Io);
        }
        current = next;
    }
    Ok(current)
}

fn response_channel(result: Result<Channel, FsStatus>) -> (FsStatus, Channel) {
    match result {
        Ok(channel) => (FsStatus::Ok, channel),
        Err(status) => {
            let (client, server) = Channel::pair().expect("vfsd placeholder channel");
            let _ = Memory::close(server.0);
            (status, client)
        }
    }
}

fn validate_process_name(process_name: &str) -> Result<(), FsStatus> {
    if process_name.is_empty() || process_name.len() > 64 {
        return Err(FsStatus::InvalidArgs);
    }
    if process_name
        .bytes()
        .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_')
    {
        return Err(FsStatus::InvalidArgs);
    }
    Ok(())
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

fn refs(hs: &[u64]) -> Vec<HandleRef> {
    hs.iter().map(|h| HandleRef { raw: *h }).collect()
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn reply<Q: FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = vec![0; 65500];
    let mut hs = [HandleRef { raw: 0 }; 16];
    let e = q.encode(&mut bytes, &mut hs).expect("vfsd encode");
    channel
        .send(
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .expect("vfsd reply");
}
