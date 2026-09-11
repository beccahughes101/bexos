//! Small canonical descriptors shared by service adapters.
use bexos_migration::Error;
use fs_fidl::FsStatus;
pub fn fs_status(raw: u64) -> Result<FsStatus, Error> {
    let all = [
        FsStatus::Ok,
        FsStatus::NotFound,
        FsStatus::NotDirectory,
        FsStatus::IsDirectory,
        FsStatus::NotEmpty,
        FsStatus::NoSpace,
        FsStatus::Io,
        FsStatus::Corrupt,
        FsStatus::Locked,
        FsStatus::ReadOnly,
        FsStatus::AccessDenied,
        FsStatus::InvalidArgs,
        FsStatus::AlreadyExists,
        FsStatus::BadState,
    ];
    all.into_iter()
        .find(|s| *s as i32 as i64 as u64 == raw)
        .ok_or(Error::InvalidData)
}
