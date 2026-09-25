#[cfg(feature = "persistent")]
use alloc::vec::Vec;
#[cfg(feature = "persistent")]
use bexos_job_store::{JobRecord, persistent::JobStoreDb};
#[cfg(feature = "persistent")]
use bexos_redb::bexos_fs::{FileBlockStore, defer_file_syncs};
#[cfg(feature = "persistent")]
use bexos_userspace::{Channel, fs, vfs};

#[cfg(feature = "persistent")]
const JOBD_PACKAGE: &str = "bexos.service.jobd";
#[cfg(feature = "persistent")]
pub const JOBS_DB_FILE: &str = "jobs.redb";

#[cfg(feature = "persistent")]
pub fn open_system_jobs(vfsd: Channel) -> Result<JobStoreDb, fs_fidl::FsStatus> {
    let dir = vfs::get_system_data_directory(vfsd, JOBD_PACKAGE)?;
    let result = open_jobs_file(dir);
    let close = fs::close(dir);
    result.and_then(|db| close.map(|()| db))
}

#[cfg(feature = "persistent")]
pub fn open_user_jobs(vfsd: Channel, uid: u64) -> Result<JobStoreDb, fs_fidl::FsStatus> {
    let dir = vfs::get_user_data_directory(vfsd, uid, JOBD_PACKAGE)?;
    let result = open_jobs_file(dir);
    let close = fs::close(dir);
    result.and_then(|db| close.map(|()| db))
}

#[cfg(feature = "persistent")]
pub fn load_system_jobs(vfsd: Channel) -> Result<Vec<JobRecord>, fs_fidl::FsStatus> {
    let dir = vfs::get_system_data_directory(vfsd, JOBD_PACKAGE)?;
    let result = load_jobs_file(dir);
    let close = fs::close(dir);
    result.and_then(|records| close.map(|()| records))
}

#[cfg(feature = "persistent")]
pub fn load_user_jobs(vfsd: Channel, uid: u64) -> Result<Vec<JobRecord>, fs_fidl::FsStatus> {
    let dir = vfs::get_user_data_directory(vfsd, uid, JOBD_PACKAGE)?;
    let result = load_jobs_file(dir);
    let close = fs::close(dir);
    result.and_then(|records| close.map(|()| records))
}

#[cfg(feature = "persistent")]
fn load_jobs_file(dir: Channel) -> Result<Vec<JobRecord>, fs_fidl::FsStatus> {
    let file = fs::open(dir, JOBS_DB_FILE, 1 | 2 | 8)?;
    // Opening or dropping a Redb database can update allocator bookkeeping.
    // A boot-time load is logically read-only, so keep filesystem syncs
    // deferred through `Database` destruction rather than only through open.
    // Actual scheduler mutations continue to use `open_jobs_file` below and
    // retain their normal durable commit behavior.
    let _defer_syncs = defer_file_syncs();
    let store = JobStoreDb::open(FileBlockStore::new(file)).map_err(|_| {
        let _ = fs::close(file);
        fs_fidl::FsStatus::Io
    })?;
    let records = store.list().map_err(|_| fs_fidl::FsStatus::Io);
    drop(store);
    records
}

#[cfg(feature = "persistent")]
pub fn open_jobs_file(dir: Channel) -> Result<JobStoreDb, fs_fidl::FsStatus> {
    let file = fs::open(dir, JOBS_DB_FILE, 1 | 2 | 8)?;
    let empty = fs::attributes(file)
        .map(|attributes| attributes.size_bytes == 0)
        .unwrap_or(false);
    if empty {
        let _defer_syncs = defer_file_syncs();
        return JobStoreDb::open(FileBlockStore::new(file)).map_err(|_| {
            let _ = fs::close(file);
            fs_fidl::FsStatus::Io
        });
    }
    JobStoreDb::open(FileBlockStore::new(file)).map_err(|_| {
        let _ = fs::close(file);
        fs_fidl::FsStatus::Io
    })
}
