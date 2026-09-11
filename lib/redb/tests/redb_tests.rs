use std::sync::Arc;

use bexos_redb::fs::FileBlockStore;
use bexos_redb::mem::MemBlockStore;
use bexos_redb::{BlockStore, BlockStoreError, open_or_create_with_store};
use redb::{ReadableDatabase, TableDefinition};

const TABLE: TableDefinition<&str, u64> = TableDefinition::new("numbers");

#[test]
fn custom_backend_commits_and_reopens() {
    let store = Arc::new(MemBlockStore::new());
    {
        let db = open_or_create_with_store(store.clone()).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut table = tx.open_table(TABLE).unwrap();
            table.insert("one", 1).unwrap();
        }
        tx.commit().unwrap();
    }

    let db = open_or_create_with_store(store).unwrap();
    let tx = db.begin_read().unwrap();
    let table = tx.open_table(TABLE).unwrap();

    assert_eq!(table.get("one").unwrap().unwrap().value(), 1);
}

#[test]
fn custom_backend_grows_storage() {
    let store = Arc::new(MemBlockStore::new());
    let before = store.len().unwrap();

    let db = open_or_create_with_store(store.clone()).unwrap();
    let tx = db.begin_write().unwrap();
    {
        let mut table = tx.open_table(TABLE).unwrap();
        for i in 0..512 {
            table.insert(format!("key-{i:04}").as_str(), i).unwrap();
        }
    }
    tx.commit().unwrap();

    assert!(store.len().unwrap() > before);
}

#[test]
fn bounds_errors_are_reported() {
    let store = MemBlockStore::from_bytes(vec![1, 2, 3]);
    let mut out = [0; 4];

    assert_eq!(
        store.read_at(0, &mut out),
        Err(BlockStoreError::OutOfBounds)
    );
    assert_eq!(
        store.write_at(2, &[9, 9]),
        Err(BlockStoreError::OutOfBounds)
    );
}

#[test]
fn commit_syncs_store() {
    let store = Arc::new(MemBlockStore::new());
    let db = open_or_create_with_store(store.clone()).unwrap();
    let tx = db.begin_write().unwrap();
    {
        let mut table = tx.open_table(TABLE).unwrap();
        table.insert("synced", 7).unwrap();
    }
    tx.commit().unwrap();

    assert!(store.sync_count() > 0);
}

#[test]
fn file_block_store_persists_database() {
    let path = std::env::temp_dir().join(format!("bexos-redb-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&path);

    {
        let db = open_or_create_with_store(FileBlockStore::open(&path).unwrap()).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut table = tx.open_table(TABLE).unwrap();
            table.insert("file", 11).unwrap();
        }
        tx.commit().unwrap();
    }

    let db = open_or_create_with_store(FileBlockStore::open(&path).unwrap()).unwrap();
    let tx = db.begin_read().unwrap();
    let table = tx.open_table(TABLE).unwrap();
    assert_eq!(table.get("file").unwrap().unwrap().value(), 11);

    let _ = std::fs::remove_file(path);
}
