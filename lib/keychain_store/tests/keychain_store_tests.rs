use std::sync::Arc;

use bexos_keychain_store::persistent::KeychainStoreDb;
use bexos_keychain_store::{KeyFlags, KeychainStoreError, MemoryKeychainStore, SecretRecord};
use bexos_redb::mem::MemBlockStore;

#[test]
fn memory_store_keys_secrets_by_alias_inside_one_vault() {
    let mut store = MemoryKeychainStore::new();

    store
        .store_secret("api:token", b"secret", KeyFlags::empty())
        .unwrap();
    assert_eq!(store.get_secret("api:token").unwrap(), b"secret");

    store
        .store_secret("api:token", b"rotated", KeyFlags::EXPORTABLE)
        .unwrap();
    assert_eq!(store.get_secret("api:token").unwrap(), b"rotated");

    store.delete_secret("api:token").unwrap();
    assert_eq!(
        store.get_secret("api:token").unwrap_err(),
        KeychainStoreError::NotFound
    );
}

#[test]
fn memory_store_rejects_bad_aliases_and_empty_secrets() {
    let mut store = MemoryKeychainStore::new();

    assert_eq!(
        store
            .store_secret("../bad", b"secret", KeyFlags::empty())
            .unwrap_err(),
        KeychainStoreError::InvalidAlias
    );
    assert_eq!(
        store
            .store_secret("empty", b"", KeyFlags::empty())
            .unwrap_err(),
        KeychainStoreError::InvalidSecret
    );
}

#[test]
fn redb_store_persists_alias_scoped_secrets() {
    let backing = Arc::new(MemBlockStore::new());
    {
        let store = KeychainStoreDb::open(backing.clone()).unwrap();
        store
            .put_secret(&SecretRecord {
                alias: "wifi:office".into(),
                secret: b"password".to_vec(),
                flags: KeyFlags::empty(),
                generation: 1,
                envelope_key_blob: Vec::new(),
                nonce: Vec::new(),
                characteristics: Vec::new(),
            })
            .unwrap();
    }

    let store = KeychainStoreDb::open(backing).unwrap();
    let record = store.get_secret("wifi:office").unwrap();
    assert_eq!(record.secret, b"password");
    store.delete_secret("wifi:office").unwrap();
    assert_eq!(
        store.get_secret("wifi:office").unwrap_err(),
        KeychainStoreError::NotFound
    );
}

#[test]
fn memory_checkpoint_preserves_generation_and_rejects_truncation() {
    use bexos_keychain_store::{KeyFlags, MemoryKeychainStore};
    let mut store = MemoryKeychainStore::new();
    store
        .store_secret("retained", b"first", KeyFlags::empty())
        .unwrap();
    store
        .store_secret("retained", b"second", KeyFlags::empty())
        .unwrap();
    let bytes = store.checkpoint();
    let restored = MemoryKeychainStore::from_checkpoint(&bytes).unwrap();
    assert_eq!(restored, store);
    assert!(MemoryKeychainStore::from_checkpoint(&bytes[..bytes.len() - 1]).is_err());
}
