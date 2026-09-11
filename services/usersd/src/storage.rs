#[cfg(feature = "persistent")]
use bexos_redb::bexos_fs::{FileBlockStore, defer_file_syncs};
#[cfg(feature = "persistent")]
use bexos_user_store::persistent::UserStoreDb;
#[cfg(feature = "persistent")]
use bexos_userspace::{Channel, fs, log, vfs};

#[cfg(feature = "persistent")]
const USERSD_PACKAGE: &str = "bexos.service.usersd";
#[cfg(feature = "persistent")]
const USERS_DB_FILE: &str = "users.redb";

#[cfg(feature = "persistent")]
pub fn open_user_store(vfsd: Channel) -> Result<UserStoreDb, fs_fidl::FsStatus> {
    log("usersd: persistent store opening system data directory\n");
    let dir = vfs::get_system_data_directory(vfsd, USERSD_PACKAGE)?;
    log("usersd: persistent store opening database file\n");
    let result = open_user_store_file(dir);
    let close = fs::close(dir);
    let result = result.and_then(|db| close.map(|()| db));
    if result.is_ok() {
        log("usersd: persistent store database open complete\n");
    }
    result
}

#[cfg(feature = "persistent")]
fn open_user_store_file(dir: Channel) -> Result<UserStoreDb, fs_fidl::FsStatus> {
    let file = fs::open(dir, USERS_DB_FILE, 1 | 2 | 8)?;
    let length = fs::attributes(file)?.size_bytes;
    log("usersd: persistent store redb open begin
");
    if length == 0 {
        log(
            "usersd: persistent store creating empty database with deferred sync
",
        );
        let _defer_syncs = defer_file_syncs();
        return UserStoreDb::open(FileBlockStore::new(file)).map_err(|_| {
            let _ = fs::close(file);
            fs_fidl::FsStatus::Io
        });
    }
    UserStoreDb::open(FileBlockStore::new(file)).map_err(|_| {
        let _ = fs::close(file);
        fs_fidl::FsStatus::Io
    })
}
