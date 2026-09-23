use bexos_redb::{BlockStore, BlockStoreError, mem::MemBlockStore, retained::RetainedStore};
use redb::{ReadableDatabase, TableDefinition};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Debug)]
struct TrackedStore {
    bytes: Arc<MemBlockStore>,
    closes: Arc<AtomicUsize>,
}

impl BlockStore for TrackedStore {
    fn len(&self) -> Result<u64, BlockStoreError> {
        self.bytes.len()
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), BlockStoreError> {
        self.bytes.read_at(offset, out)
    }
    fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<(), BlockStoreError> {
        self.bytes.write_at(offset, bytes)
    }
    fn set_len(&self, len: u64) -> Result<(), BlockStoreError> {
        self.bytes.set_len(len)
    }
    fn sync(&self) -> Result<(), BlockStoreError> {
        self.bytes.sync()
    }
    fn close(&self) -> Result<(), BlockStoreError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn quiescing_and_reopening_a_database_retains_its_external_store() {
    let store = TrackedStore {
        bytes: Arc::new(MemBlockStore::new()),
        closes: Arc::new(AtomicUsize::new(0)),
    };
    const TABLE: TableDefinition<&str, &str> = TableDefinition::new("migration");
    {
        let db = bexos_redb::open_or_create_with_store(RetainedStore::new(store.clone())).unwrap();
        let tx = db.begin_write().unwrap();
        tx.open_table(TABLE)
            .unwrap()
            .insert("state", "committed")
            .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(store.closes.load(Ordering::SeqCst), 0);
    {
        let db = bexos_redb::open_or_create_with_store(RetainedStore::new(store.clone())).unwrap();
        let tx = db.begin_read().unwrap();
        let table = tx.open_table(TABLE).unwrap();
        assert_eq!(table.get("state").unwrap().unwrap().value(), "committed");
    }
    assert_eq!(store.closes.load(Ordering::SeqCst), 0);
    store.close().unwrap();
    assert_eq!(store.closes.load(Ordering::SeqCst), 1);
}
