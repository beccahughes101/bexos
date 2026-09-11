use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use fs_fidl::FsStatus;
use rosefs_core::block::BlockDevice;

use crate::{BexFs, BexFsError, FileHandle, NodeAttributes};

pub struct BexfsServer<D> {
    filesystem: BexFs,
    device: D,
    open_files: BTreeMap<u64, FileHandle>,
}

impl<D: BlockDevice> BexfsServer<D> {
    pub fn new(filesystem: BexFs, device: D) -> Self {
        Self {
            filesystem,
            device,
            open_files: BTreeMap::new(),
        }
    }

    pub fn bind_file(&mut self, endpoint: u64, base: u64, path: &str, flags: u32) -> FsStatus {
        match self.filesystem.open(base, path, flags) {
            Ok(handle) => {
                self.open_files.insert(endpoint, handle);
                FsStatus::Ok
            }
            Err(error) => status(error),
        }
    }

    pub fn read(&mut self, endpoint: u64, count: u64) -> Result<Vec<u8>, FsStatus> {
        let handle = self
            .open_files
            .get_mut(&endpoint)
            .ok_or(FsStatus::BadState)?;
        self.filesystem.read(handle, count).map_err(status)
    }

    pub fn write(&mut self, endpoint: u64, bytes: &[u8]) -> Result<u64, FsStatus> {
        let handle = self
            .open_files
            .get_mut(&endpoint)
            .ok_or(FsStatus::BadState)?;
        self.filesystem.write(handle, bytes).map_err(status)
    }

    pub fn seek(&mut self, endpoint: u64, offset: i64, whence: u8) -> Result<u64, FsStatus> {
        let handle = self
            .open_files
            .get_mut(&endpoint)
            .ok_or(FsStatus::BadState)?;
        self.filesystem.seek(handle, offset, whence).map_err(status)
    }

    pub fn attributes(&self, endpoint: u64) -> Result<NodeAttributes, FsStatus> {
        let handle = self.open_files.get(&endpoint).ok_or(FsStatus::BadState)?;
        self.filesystem.attributes(handle.inode()).map_err(status)
    }

    pub fn close(&mut self, endpoint: u64) -> FsStatus {
        if self.open_files.remove(&endpoint).is_none() {
            return FsStatus::BadState;
        }
        match self.filesystem.close(&mut self.device) {
            Ok(()) => FsStatus::Ok,
            Err(error) => status(error),
        }
    }

    pub fn unmount(mut self) -> Result<D, FsStatus> {
        self.filesystem.close(&mut self.device).map_err(status)?;
        Ok(self.device)
    }
}

pub fn status(error: BexFsError) -> FsStatus {
    match error {
        BexFsError::Io => FsStatus::Io,
        BexFsError::Corrupt => FsStatus::Corrupt,
        BexFsError::Locked => FsStatus::Locked,
        BexFsError::NoSpace => FsStatus::NoSpace,
        BexFsError::NotFound => FsStatus::NotFound,
        BexFsError::NotDirectory => FsStatus::NotDirectory,
        BexFsError::IsDirectory => FsStatus::IsDirectory,
        BexFsError::NotEmpty => FsStatus::NotEmpty,
        BexFsError::AlreadyExists => FsStatus::AlreadyExists,
        BexFsError::AccessDenied => FsStatus::AccessDenied,
        BexFsError::InvalidArgs => FsStatus::InvalidArgs,
        BexFsError::ReadOnly => FsStatus::ReadOnly,
    }
}
