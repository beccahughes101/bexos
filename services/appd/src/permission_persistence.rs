use alloc::vec::Vec;

use bexos_permission_store::{
    MemoryPermissionStore, PermissionDeclaration, PermissionRequirement, PermissionStoreError,
    UserGrantRecord, UserGrantState, values_are_subset,
};
#[cfg(feature = "persistent")]
use bexos_redb::bexos_fs::FileBlockStore;
use bexos_userspace::Channel;
#[cfg(feature = "persistent")]
use bexos_userspace::{fs, vfs};

use crate::{Manifest, MemoryAppRegistry};

pub const PERMISSIONS_DB: &str = "permissions.redb";
#[cfg(feature = "persistent")]
const APPD_PACKAGE: &str = "bexos.platform.appd";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermissionPersistenceError {
    Locked,
    Storage,
    Store(PermissionStoreError),
}

impl From<PermissionStoreError> for PermissionPersistenceError {
    fn from(error: PermissionStoreError) -> Self {
        match error {
            PermissionStoreError::Storage => Self::Storage,
            other => Self::Store(other),
        }
    }
}

#[cfg(feature = "persistent")]
pub fn load_system(
    vfsd: Channel,
    memory: &mut MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    let db = open_system_db(vfsd)?;
    db.scrub_non_system_user_rows()?;
    let mut durable = db.snapshot_system_memory()?;
    durable.merge_from_memory(memory);
    durable.remove_user_rows_except_system();
    db.replace_system_from_memory(&durable)?;
    *memory = durable;
    Ok(())
}

#[cfg(not(feature = "persistent"))]
pub fn load_system(
    _vfsd: Channel,
    _memory: &mut MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    Ok(())
}

#[cfg(feature = "persistent")]
pub fn sync_system(
    vfsd: Channel,
    memory: &MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    let db = open_system_db(vfsd)?;
    db.replace_system_from_memory(memory)?;
    Ok(())
}

#[cfg(not(feature = "persistent"))]
pub fn sync_system(
    _vfsd: Channel,
    _memory: &MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    Ok(())
}

#[cfg(feature = "persistent")]
pub fn load_user(
    vfsd: Channel,
    uid: u64,
    registry: &MemoryAppRegistry,
    memory: &mut MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    if uid == crate::namespace::SYSTEM_UID {
        return load_system(vfsd, memory);
    }
    bexos_userspace::log(&alloc::format!(
        "appd: loading user permissions uid={uid}\n"
    ));
    let Some(db) = open_user_db(vfsd, uid, false)? else {
        memory.remove_user(uid);
        return Ok(());
    };
    bexos_userspace::log(&alloc::format!("appd: opened user permissions uid={uid}\n"));
    let durable = db.snapshot_user_memory(uid)?;
    let records = durable.user_records(uid);
    let reconciled = reconcile_user_records(registry, records.clone());
    let changed = reconciled != records;
    memory.replace_user_records(uid, reconciled)?;
    if changed {
        db.replace_user_from_memory(uid, memory)?;
    }
    bexos_userspace::log(&alloc::format!("appd: loaded user permissions uid={uid}\n"));
    Ok(())
}

#[cfg(not(feature = "persistent"))]
pub fn load_user(
    _vfsd: Channel,
    _uid: u64,
    _registry: &MemoryAppRegistry,
    _memory: &mut MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    Ok(())
}

#[cfg(feature = "persistent")]
pub fn sync_user(
    vfsd: Channel,
    uid: u64,
    memory: &MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    if uid == crate::namespace::SYSTEM_UID {
        return sync_system(vfsd, memory);
    }
    let db = open_user_db(vfsd, uid, true)?.ok_or(PermissionPersistenceError::Storage)?;
    db.replace_user_from_memory(uid, memory)?;
    Ok(())
}

#[cfg(not(feature = "persistent"))]
pub fn sync_user(
    _vfsd: Channel,
    _uid: u64,
    _memory: &MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    Ok(())
}

pub fn reconcile_user_records(
    registry: &MemoryAppRegistry,
    records: Vec<UserGrantRecord>,
) -> Vec<UserGrantRecord> {
    records
        .into_iter()
        .filter(|record| record.state == UserGrantState::Granted)
        .filter(|record| {
            let Ok(app) = registry.record(&record.package_id) else {
                return false;
            };
            let Ok(manifest) = Manifest::decode(&app.manifest_bytes) else {
                return false;
            };
            manifest.processes.iter().any(|process| {
                let mut declarations = manifest.permissions.clone();
                declarations.extend(process.permissions.iter().cloned());
                declarations.iter().any(|declaration| {
                    declaration.name == record.permission_name
                        && values_are_subset(&record.granted_values, &declaration.values)
                })
            })
        })
        .collect()
}

pub fn auto_grant_and_sync(
    vfsd: Channel,
    uid: u64,
    package_id: &str,
    declarations: &[PermissionDeclaration],
    memory: &mut MemoryPermissionStore,
) -> Result<(), PermissionPersistenceError> {
    let mut updated = memory.clone();
    updated.register_system_declarations(package_id, declarations, 0)?;
    updated.auto_grant_required(uid, package_id, declarations)?;
    let system_changed = updated.system_grants() != memory.system_grants();
    let user_changed = updated.user_records(uid) != memory.user_records(uid);
    // A normal launch usually reuses verified declarations and existing grants.
    // Persist a changed system snapshot only once, including system-user grants.
    if system_changed || (uid == crate::namespace::SYSTEM_UID && user_changed) {
        sync_system(vfsd, &updated)?;
    }
    if uid != crate::namespace::SYSTEM_UID && user_changed {
        sync_user(vfsd, uid, &updated)?;
    }
    *memory = updated;
    Ok(())
}

pub fn grant_optional_and_sync(
    vfsd: Channel,
    uid: u64,
    package_id: &str,
    declaration: &PermissionDeclaration,
    requested_values: &[alloc::string::String],
    memory: &mut MemoryPermissionStore,
) -> Result<UserGrantRecord, PermissionPersistenceError> {
    if declaration.requirement != PermissionRequirement::Optional {
        return Err(PermissionStoreError::UndeclaredPermission.into());
    }
    let record = memory.grant_declared(uid, package_id, declaration, requested_values)?;
    sync_user(vfsd, uid, memory)?;
    Ok(record)
}

#[cfg(feature = "persistent")]
fn open_system_db(
    vfsd: Channel,
) -> Result<bexos_permission_store::persistent::PermissionStoreDb, PermissionPersistenceError> {
    let system_dir = vfs::get_system_data_directory(vfsd, APPD_PACKAGE)
        .map_err(|_| PermissionPersistenceError::Storage)?;
    open_db(system_dir)
}

#[cfg(feature = "persistent")]
fn open_user_db(
    vfsd: Channel,
    uid: u64,
    create: bool,
) -> Result<Option<bexos_permission_store::persistent::PermissionStoreDb>, PermissionPersistenceError>
{
    let user_dir = vfs::get_user_home_directory(vfsd, uid).map_err(|status| match status {
        fs_fidl::FsStatus::Locked => PermissionPersistenceError::Locked,
        _ => PermissionPersistenceError::Storage,
    })?;
    // Loading grants must not create an empty database on every first unlock.
    // Only an actual grant mutation needs a new durable permission store.
    let file = fs::open(user_dir, PERMISSIONS_DB, 1 | 2 | if create { 8 } else { 0 });
    let _ = bexos_userspace::Memory::close(user_dir.0);
    match file {
        Ok(file) => open_file_db(file).map(Some),
        Err(fs_fidl::FsStatus::NotFound) if !create => Ok(None),
        Err(_) => Err(PermissionPersistenceError::Storage),
    }
}

#[cfg(feature = "persistent")]
fn open_db(
    root: Channel,
) -> Result<bexos_permission_store::persistent::PermissionStoreDb, PermissionPersistenceError> {
    let file = fs::open(root, PERMISSIONS_DB, 1 | 2 | 8);
    let _ = bexos_userspace::Memory::close(root.0);
    open_file_db(file.map_err(|_| PermissionPersistenceError::Storage)?)
}

#[cfg(feature = "persistent")]
fn open_file_db(
    file: Channel,
) -> Result<bexos_permission_store::persistent::PermissionStoreDb, PermissionPersistenceError> {
    let block = FileBlockStore::new(file);
    bexos_permission_store::persistent::PermissionStoreDb::open_detailed(block).map_err(|error| {
        bexos_userspace::log(&alloc::format!(
            "appd: permission store database open failed error={error:?}\n"
        ));
        PermissionPersistenceError::Storage
    })
}

#[cfg(feature = "persistent")]
trait SystemOnly {
    fn remove_user_rows_except_system(&mut self);
}

#[cfg(feature = "persistent")]
impl SystemOnly for MemoryPermissionStore {
    fn remove_user_rows_except_system(&mut self) {
        self.retain_user_records(|record| record.uid == crate::namespace::SYSTEM_UID);
    }
}
