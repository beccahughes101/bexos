use std::sync::Arc;

use bexos_redb::mem::MemBlockStore;
use bexos_user_store::persistent::UserStoreDb;
use bexos_user_store::{
    CreateUserRequest, MemoryUserStore, UpdateUserRequest, UserStoreError, decode_user, encode_user,
};

#[test]
fn memory_store_round_trips_user_proto_record() {
    let mut store = MemoryUserStore::new();
    let user = store
        .create_user(CreateUserRequest {
            uid: 1000,
            name: "alice".into(),
            display_name: "Alice".into(),
            secure_user_id: 5000,
            gatekeeper_password_handle: b"handle".to_vec(),
            auth_bound_hmac_key_blob: b"blob".to_vec(),
            generation: 7,
        })
        .unwrap();

    let decoded = decode_user(&encode_user(&user)).unwrap();
    assert_eq!(decoded.record.uid, 1000);
    assert_eq!(decoded.record.name, "alice");
    assert_eq!(decoded.filesystem.root_path, "data/users/1000");
}

#[test]
fn invalid_user_records_are_rejected() {
    let mut store = MemoryUserStore::new();
    assert_eq!(
        store
            .create_user(CreateUserRequest {
                uid: 0,
                name: "rootish".into(),
                display_name: "Rootish".into(),
                secure_user_id: 5001,
                gatekeeper_password_handle: b"handle".to_vec(),
                auth_bound_hmac_key_blob: b"blob".to_vec(),
                generation: 1,
            })
            .unwrap_err(),
        UserStoreError::InvalidUid
    );
    assert_eq!(
        store
            .create_user(CreateUserRequest {
                uid: 1001,
                name: "../bad".into(),
                display_name: "Bad".into(),
                secure_user_id: 5002,
                gatekeeper_password_handle: b"handle".to_vec(),
                auth_bound_hmac_key_blob: b"blob".to_vec(),
                generation: 1,
            })
            .unwrap_err(),
        UserStoreError::InvalidName
    );
}

#[test]
fn redb_store_persists_create_update_delete() {
    let backing = Arc::new(MemBlockStore::new());
    {
        let store = UserStoreDb::open(backing.clone()).unwrap();
        store
            .create_user(CreateUserRequest {
                uid: 1000,
                name: "alice".into(),
                display_name: "Alice".into(),
                secure_user_id: 5003,
                gatekeeper_password_handle: b"handle".to_vec(),
                auth_bound_hmac_key_blob: b"blob".to_vec(),
                generation: 1,
            })
            .unwrap();
        store
            .update_user(UpdateUserRequest {
                uid: 1000,
                name: "alice".into(),
                display_name: "Alice Smith".into(),
                disabled: true,
                gatekeeper_password_handle: None,
                generation: 2,
            })
            .unwrap();
    }

    let store = UserStoreDb::open(backing).unwrap();
    let user = store.get_user(1000).unwrap();
    assert_eq!(user.record.display_name, "Alice Smith");
    assert_eq!(store.delete_user(1000).unwrap().record.uid, 1000);
    assert_eq!(store.get_user(1000).unwrap_err(), UserStoreError::NotFound);
}
