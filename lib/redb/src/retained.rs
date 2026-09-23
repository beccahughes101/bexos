//! Storage whose close operation belongs to an external owner, such as a
//! service retaining file channels across database quiescence and migration.
use crate::{BlockStore, BlockStoreError};

#[derive(Debug)]
pub struct RetainedStore<S>(S);

impl<S> RetainedStore<S> {
    /// The caller must retain the underlying resource until the database has
    /// closed, and close it when its own ownership ends. Database flushes and
    /// transaction durability are unaffected.
    pub fn new(store: S) -> Self {
        Self(store)
    }
}

impl<S: BlockStore> BlockStore for RetainedStore<S> {
    fn len(&self) -> Result<u64, BlockStoreError> {
        self.0.len()
    }

    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
        self.0.read_at(offset, out)
    }

    fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), BlockStoreError> {
        self.0.write_at(offset, data)
    }

    fn set_len(&self, len: u64) -> Result<(), BlockStoreError> {
        self.0.set_len(len)
    }

    fn sync(&self) -> Result<(), BlockStoreError> {
        self.0.sync()
    }

    fn close(&self) -> Result<(), BlockStoreError> {
        Ok(())
    }
}
