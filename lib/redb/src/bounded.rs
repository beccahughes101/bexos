//! Enforce a physical backing-store limit before redb can grow or write a file.
use crate::{BlockStore, BlockStoreError};
#[derive(Debug)]
pub struct BoundedStore<S> {
    store: S,
    maximum: u64,
}
impl<S: BlockStore> BoundedStore<S> {
    pub fn new(store: S, maximum: u64) -> Result<Self, BlockStoreError> {
        if store.len()? > maximum {
            return Err(BlockStoreError::OutOfBounds);
        }
        Ok(Self { store, maximum })
    }
    fn range(&self, offset: u64, length: usize) -> Result<(), BlockStoreError> {
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > self.maximum)
        {
            return Err(BlockStoreError::OutOfBounds);
        }
        Ok(())
    }
}
impl<S: BlockStore> BlockStore for BoundedStore<S> {
    fn len(&self) -> Result<u64, BlockStoreError> {
        self.store.len()
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
        self.range(offset, out.len())?;
        self.store.read_at(offset, out)
    }
    fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), BlockStoreError> {
        self.range(offset, data.len())?;
        self.store.write_at(offset, data)
    }
    fn set_len(&self, length: u64) -> Result<(), BlockStoreError> {
        if length > self.maximum {
            return Err(BlockStoreError::OutOfBounds);
        }
        self.store.set_len(length)
    }
    fn sync(&self) -> Result<(), BlockStoreError> {
        self.store.sync()
    }
    fn close(&self) -> Result<(), BlockStoreError> {
        self.store.close()
    }
}
