use bexos_pkgd::cas::Cas;
use bexos_redb::{BlockStore, BlockStoreError, mem::MemBlockStore, open_or_create_with_store};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug)]
struct InterruptedStore {
    bytes: MemBlockStore,
    operation: AtomicUsize,
    interrupt_at: AtomicUsize,
}
impl InterruptedStore {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: MemBlockStore::from_bytes(bytes),
            operation: AtomicUsize::new(0),
            interrupt_at: AtomicUsize::new(usize::MAX),
        }
    }
    fn interrupted(&self) -> bool {
        self.operation.fetch_add(1, Ordering::SeqCst) >= self.interrupt_at.load(Ordering::SeqCst)
    }
}
impl BlockStore for InterruptedStore {
    fn len(&self) -> Result<u64, BlockStoreError> {
        self.bytes.len()
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
        self.bytes.read_at(offset, out)
    }
    fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<(), BlockStoreError> {
        if self.interrupted() {
            // Model a torn write, not merely an error before the operation.
            self.bytes.write_at(offset, &bytes[..bytes.len() / 2])?;
            Err(BlockStoreError::Io)
        } else {
            self.bytes.write_at(offset, bytes)
        }
    }
    fn set_len(&self, length: u64) -> Result<(), BlockStoreError> {
        if self.interrupted() {
            Err(BlockStoreError::Io)
        } else {
            self.bytes.set_len(length)
        }
    }
    fn sync(&self) -> Result<(), BlockStoreError> {
        if self.interrupted() {
            Err(BlockStoreError::Io)
        } else {
            self.bytes.sync()
        }
    }
}

#[test]
fn interrupted_cas_replacement_recovers_only_complete_transactions() {
    let baseline = Arc::new(MemBlockStore::new());
    let old_digest: [u8; 32] = Sha256::digest(b"baseline").into();
    let bytes = vec![0x5a; 64 * 1024];
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let capacity = bytes.len() as u64;
    let mut cas = Cas::open(
        open_or_create_with_store(baseline.clone()).unwrap(),
        capacity,
    )
    .unwrap();
    cas.put(&old_digest, b"baseline", &BTreeSet::new()).unwrap();
    drop(cas);
    let initial = baseline.bytes();
    let complete = Arc::new(InterruptedStore::new(initial.clone()));
    let mut cas = Cas::open(
        open_or_create_with_store(complete.clone()).unwrap(),
        capacity,
    )
    .unwrap();
    complete.operation.store(0, Ordering::SeqCst);
    cas.put(&digest, &bytes, &BTreeSet::new()).unwrap();
    let operations = complete.operation.load(Ordering::SeqCst);
    assert!(operations > 0);
    drop(cas);
    for cut in 0..operations {
        let store = Arc::new(InterruptedStore::new(initial.clone()));
        let mut cas =
            Cas::open(open_or_create_with_store(store.clone()).unwrap(), capacity).unwrap();
        store.operation.store(0, Ordering::SeqCst);
        store.interrupt_at.store(cut, Ordering::SeqCst);
        assert!(
            cas.put(&digest, &bytes, &BTreeSet::new()).is_err(),
            "cut {cut}"
        );
        // Capture the crash image before any destructor can flush the database.
        let crashed = store.bytes.bytes();
        drop(cas);
        let database = open_or_create_with_store(MemBlockStore::from_bytes(crashed))
            .unwrap_or_else(|error| panic!("reopen at cut {cut}: {error}"));
        let mut recovered = Cas::open(database, capacity).unwrap();
        let old = recovered.get(&old_digest).unwrap();
        let new = recovered.get(&digest).unwrap();
        assert!(
            old.is_some() ^ new.is_some(),
            "partial replacement at cut {cut}"
        );
        if let Some(old) = old {
            assert_eq!(old, b"baseline");
        }
        if let Some(new) = new {
            assert_eq!(new, bytes);
        }
    }
}
