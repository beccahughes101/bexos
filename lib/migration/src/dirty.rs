//! Bounded, coalescing invalidations for adapters whose canonical state already
//! lives in typed storage. Copying a record clears its invalidation; later writes
//! reinsert it. Deltas encode current values, never replay external side effects.
use crate::Error;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

pub struct DirtySet {
    keys: BTreeSet<u64>,
    capacity: usize,
    failed: bool,
}
impl DirtySet {
    pub fn new(capacity: usize) -> Self {
        Self {
            keys: BTreeSet::new(),
            capacity,
            failed: false,
        }
    }
    pub fn mark(&mut self, key: u64) {
        if self.keys.len() >= self.capacity && !self.keys.contains(&key) {
            self.failed = true;
        }
        if !self.failed {
            self.keys.insert(key);
        }
    }
    pub fn copied(&mut self, key: u64) {
        self.keys.remove(&key);
    }
    pub fn next(&self) -> Result<Option<u64>, Error> {
        if self.failed {
            Err(Error::Capacity)
        } else {
            Ok(self.keys.first().copied())
        }
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    /// A stable ordered pass prevents a repeatedly dirtied low key from
    /// starving the other records while keeping metadata before its children.
    pub fn snapshot_keys(&self) -> Result<Vec<u64>, Error> {
        if self.failed {
            return Err(Error::Capacity);
        }
        Ok(self.keys.iter().copied().collect())
    }
}
