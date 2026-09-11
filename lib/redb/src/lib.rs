#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStoreError {
    Io,
    OutOfBounds,
    ReadOnly,
    Closed,
    InvalidLength,
}

#[cfg(feature = "std")]
impl<T: BlockStore> BlockStore for std::sync::Arc<T> {
    fn len(&self) -> Result<u64, BlockStoreError> {
        self.as_ref().len()
    }

    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
        self.as_ref().read_at(offset, out)
    }

    fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), BlockStoreError> {
        self.as_ref().write_at(offset, data)
    }

    fn set_len(&self, len: u64) -> Result<(), BlockStoreError> {
        self.as_ref().set_len(len)
    }

    fn sync(&self) -> Result<(), BlockStoreError> {
        self.as_ref().sync()
    }

    fn close(&self) -> Result<(), BlockStoreError> {
        self.as_ref().close()
    }
}

pub trait BlockStore: core::fmt::Debug + Send + Sync + 'static {
    fn len(&self) -> Result<u64, BlockStoreError>;
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError>;
    fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), BlockStoreError>;
    fn set_len(&self, len: u64) -> Result<(), BlockStoreError>;
    fn sync(&self) -> Result<(), BlockStoreError>;

    fn close(&self) -> Result<(), BlockStoreError> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct RedbStorageBackend<S> {
    store: S,
}

impl<S> RedbStorageBackend<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &S {
        &self.store
    }
}

#[cfg(feature = "redb_backend")]
impl<S: BlockStore> redb::StorageBackend for RedbStorageBackend<S> {
    fn len(&self) -> Result<u64, RedbIoError> {
        self.store.len().map_err(io_error)
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), RedbIoError> {
        self.store.read_at(offset, out).map_err(io_error)
    }

    fn set_len(&self, len: u64) -> Result<(), RedbIoError> {
        self.store.set_len(len).map_err(io_error)
    }

    fn sync_data(&self) -> Result<(), RedbIoError> {
        self.store.sync().map_err(io_error)
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), RedbIoError> {
        self.store.write_at(offset, data).map_err(io_error)
    }

    fn close(&self) -> Result<(), RedbIoError> {
        self.store.close().map_err(io_error)
    }
}

#[cfg(feature = "redb_backend")]
pub fn create_with_store<S: BlockStore>(store: S) -> Result<redb::Database, redb::DatabaseError> {
    let mut builder = redb::Builder::new();
    builder.set_cache_size(BEXOS_CACHE_SIZE_BYTES);
    builder.create_with_backend(RedbStorageBackend::new(store))
}

#[cfg(feature = "redb_backend")]
pub fn open_or_create_with_store<S: BlockStore>(
    store: S,
) -> Result<redb::Database, redb::DatabaseError> {
    let mut builder = redb::Builder::new();
    builder.set_cache_size(BEXOS_CACHE_SIZE_BYTES);
    builder.create_with_backend(RedbStorageBackend::new(store))
}

#[cfg(feature = "redb_backend")]
const BEXOS_CACHE_SIZE_BYTES: usize = 8 * 1024 * 1024;

#[cfg(feature = "redb_backend")]
#[cfg(feature = "redb_no_std")]
type RedbIoError = redb::io::Error;

#[cfg(feature = "redb_backend")]
#[cfg(not(feature = "redb_no_std"))]
type RedbIoError = std::io::Error;

#[cfg(feature = "redb_backend")]
#[cfg(feature = "redb_no_std")]
fn io_error(error: BlockStoreError) -> RedbIoError {
    RedbIoError::other(alloc::format!("{error:?}"))
}

#[cfg(feature = "redb_backend")]
#[cfg(not(feature = "redb_no_std"))]
fn io_error(error: BlockStoreError) -> RedbIoError {
    let kind = match error {
        BlockStoreError::Io => std::io::ErrorKind::Other,
        BlockStoreError::OutOfBounds => std::io::ErrorKind::UnexpectedEof,
        BlockStoreError::ReadOnly => std::io::ErrorKind::PermissionDenied,
        BlockStoreError::Closed => std::io::ErrorKind::BrokenPipe,
        BlockStoreError::InvalidLength => std::io::ErrorKind::InvalidInput,
    };
    RedbIoError::new(kind, alloc::format!("{error:?}"))
}

#[cfg(any(test, feature = "std"))]
pub mod mem {
    use super::{BlockStore, BlockStoreError};
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    pub struct MemBlockStore {
        bytes: Mutex<Vec<u8>>,
        sync_count: AtomicU64,
    }

    impl MemBlockStore {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn from_bytes(bytes: Vec<u8>) -> Self {
            Self {
                bytes: Mutex::new(bytes),
                sync_count: AtomicU64::new(0),
            }
        }

        pub fn bytes(&self) -> Vec<u8> {
            self.bytes.lock().unwrap().clone()
        }

        pub fn sync_count(&self) -> u64 {
            self.sync_count.load(Ordering::SeqCst)
        }
    }

    impl BlockStore for MemBlockStore {
        fn len(&self) -> Result<u64, BlockStoreError> {
            Ok(self.bytes.lock().unwrap().len() as u64)
        }

        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
            let bytes = self.bytes.lock().unwrap();
            let start = usize::try_from(offset).map_err(|_| BlockStoreError::InvalidLength)?;
            let end = start
                .checked_add(out.len())
                .ok_or(BlockStoreError::InvalidLength)?;
            if end > bytes.len() {
                return Err(BlockStoreError::OutOfBounds);
            }
            out.copy_from_slice(&bytes[start..end]);
            Ok(())
        }

        fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), BlockStoreError> {
            let mut bytes = self.bytes.lock().unwrap();
            let start = usize::try_from(offset).map_err(|_| BlockStoreError::InvalidLength)?;
            let end = start
                .checked_add(data.len())
                .ok_or(BlockStoreError::InvalidLength)?;
            if end > bytes.len() {
                return Err(BlockStoreError::OutOfBounds);
            }
            bytes[start..end].copy_from_slice(data);
            Ok(())
        }

        fn set_len(&self, len: u64) -> Result<(), BlockStoreError> {
            let len = usize::try_from(len).map_err(|_| BlockStoreError::InvalidLength)?;
            self.bytes.lock().unwrap().resize(len, 0);
            Ok(())
        }

        fn sync(&self) -> Result<(), BlockStoreError> {
            self.sync_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
}

#[cfg(feature = "std")]
pub mod fs {
    use super::{BlockStore, BlockStoreError};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::unix::fs::FileExt;
    use std::path::Path;
    use std::sync::Mutex;

    #[derive(Debug)]
    pub struct FileBlockStore {
        file: Mutex<File>,
    }

    impl FileBlockStore {
        pub fn open(path: impl AsRef<Path>) -> Result<Self, io::Error> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(path)?;
            Ok(Self {
                file: Mutex::new(file),
            })
        }
    }

    impl BlockStore for FileBlockStore {
        fn len(&self) -> Result<u64, BlockStoreError> {
            self.file
                .lock()
                .map_err(|_| BlockStoreError::Closed)?
                .metadata()
                .map(|m| m.len())
                .map_err(|_| BlockStoreError::Io)
        }

        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
            let file = self.file.lock().map_err(|_| BlockStoreError::Closed)?;
            let mut read = 0usize;
            while read < out.len() {
                let n = file
                    .read_at(
                        &mut out[read..],
                        offset
                            .checked_add(read as u64)
                            .ok_or(BlockStoreError::InvalidLength)?,
                    )
                    .map_err(|error| match error.kind() {
                        io::ErrorKind::InvalidInput => BlockStoreError::InvalidLength,
                        _ => BlockStoreError::Io,
                    })?;
                if n == 0 {
                    return Err(BlockStoreError::OutOfBounds);
                }
                read += n;
            }
            Ok(())
        }

        fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), BlockStoreError> {
            let file = self.file.lock().map_err(|_| BlockStoreError::Closed)?;
            let mut written = 0usize;
            while written < data.len() {
                let n = file
                    .write_at(
                        &data[written..],
                        offset
                            .checked_add(written as u64)
                            .ok_or(BlockStoreError::InvalidLength)?,
                    )
                    .map_err(|error| match error.kind() {
                        io::ErrorKind::PermissionDenied => BlockStoreError::ReadOnly,
                        io::ErrorKind::InvalidInput => BlockStoreError::InvalidLength,
                        _ => BlockStoreError::Io,
                    })?;
                if n == 0 {
                    return Err(BlockStoreError::Io);
                }
                written += n;
            }
            Ok(())
        }

        fn set_len(&self, len: u64) -> Result<(), BlockStoreError> {
            self.file
                .lock()
                .map_err(|_| BlockStoreError::Closed)?
                .set_len(len)
                .map_err(|_| BlockStoreError::Io)
        }

        fn sync(&self) -> Result<(), BlockStoreError> {
            self.file
                .lock()
                .map_err(|_| BlockStoreError::Closed)?
                .sync_data()
                .map_err(|_| BlockStoreError::Io)
        }
    }
}

#[cfg(all(feature = "std", feature = "bexos_libc_runtime"))]
pub mod bexos_fs {
    use super::{BlockStore, BlockStoreError};
    use bexos_userspace::{Channel, fs};
    use core::sync::atomic::{AtomicBool, Ordering};

    static FILE_SYNCS_DEFERRED: AtomicBool = AtomicBool::new(false);

    pub struct FileSyncDeferral {
        _private: (),
    }

    pub fn defer_file_syncs() -> FileSyncDeferral {
        assert!(
            !FILE_SYNCS_DEFERRED.swap(true, Ordering::AcqRel),
            "file sync deferral must not be nested"
        );
        FileSyncDeferral { _private: () }
    }

    impl Drop for FileSyncDeferral {
        fn drop(&mut self) {
            FILE_SYNCS_DEFERRED.store(false, Ordering::Release);
        }
    }

    #[derive(Clone, Copy)]
    pub struct FileBlockStore {
        file: Channel,
    }

    impl FileBlockStore {
        pub fn new(file: Channel) -> Self {
            Self { file }
        }

        pub fn file(&self) -> Channel {
            self.file
        }
    }

    impl core::fmt::Debug for FileBlockStore {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("FileBlockStore").finish_non_exhaustive()
        }
    }

    impl BlockStore for FileBlockStore {
        fn len(&self) -> Result<u64, BlockStoreError> {
            fs::attributes(self.file)
                .map(|attr| attr.size_bytes)
                .map_err(|status| {
                    bexos_userspace::log(&alloc::format!(
                        "redb-fs: attributes failed status={status:?}\n"
                    ));
                    BlockStoreError::Io
                })
        }

        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
            fs::seek(
                self.file,
                i64::try_from(offset).map_err(|_| BlockStoreError::InvalidLength)?,
            )
            .map_err(|status| {
                bexos_userspace::log(&alloc::format!(
                    "redb-fs: read seek failed offset={offset} status={status:?}\n"
                ));
                BlockStoreError::Io
            })?;
            let bytes = fs::read(self.file, out.len() as u64).map_err(|status| {
                bexos_userspace::log(&alloc::format!(
                    "redb-fs: read failed offset={offset} bytes={} status={status:?}\n",
                    out.len()
                ));
                BlockStoreError::Io
            })?;
            if bytes.len() != out.len() {
                bexos_userspace::log(&alloc::format!(
                    "redb-fs: short read offset={offset} expected={} actual={}\n",
                    out.len(),
                    bytes.len()
                ));
                return Err(BlockStoreError::OutOfBounds);
            }
            out.copy_from_slice(&bytes);
            Ok(())
        }

        fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), BlockStoreError> {
            fs::seek(
                self.file,
                i64::try_from(offset).map_err(|_| BlockStoreError::InvalidLength)?,
            )
            .map_err(|status| {
                bexos_userspace::log(&alloc::format!(
                    "redb-fs: write seek failed offset={offset} status={status:?}\n"
                ));
                BlockStoreError::Io
            })?;
            fs::write(self.file, data).map_err(|status| {
                bexos_userspace::log(&alloc::format!(
                    "redb-fs: write failed offset={offset} bytes={} status={status:?}\n",
                    data.len()
                ));
                BlockStoreError::Io
            })
        }

        fn set_len(&self, len: u64) -> Result<(), BlockStoreError> {
            fs::set_len(self.file, len).map_err(|status| {
                bexos_userspace::log(&alloc::format!(
                    "redb-fs: set-len failed bytes={len} status={status:?}\n"
                ));
                BlockStoreError::Io
            })
        }

        fn sync(&self) -> Result<(), BlockStoreError> {
            if FILE_SYNCS_DEFERRED.load(Ordering::Acquire) {
                Ok(())
            } else {
                fs::sync_file(self.file).map_err(|status| {
                    bexos_userspace::log(&alloc::format!(
                        "redb-fs: sync failed status={status:?}\n"
                    ));
                    BlockStoreError::Io
                })
            }
        }

        fn close(&self) -> Result<(), BlockStoreError> {
            fs::close(self.file).map_err(|_| BlockStoreError::Io)
        }
    }
}
