use std::sync::Arc;

use bexos_permission_store::{
    MemoryPermissionStore, PermissionDeclaration, PermissionRequirement, PermissionStoreError,
};
use bexos_redb::mem::MemBlockStore;

#[test]
fn auto_grants_required_declarations_with_scoped_values() {
    let mut store = MemoryPermissionStore::new();
    let declarations = vec![
        PermissionDeclaration {
            name: "CAMERA".to_string(),
            values: vec!["front".to_string(), "back".to_string()],
            requirement: PermissionRequirement::Required,
            usage_description: "capture photos".to_string(),
        },
        PermissionDeclaration {
            name: "MICROPHONE".to_string(),
            values: vec!["default".to_string()],
            requirement: PermissionRequirement::Optional,
            usage_description: "record video".to_string(),
        },
    ];

    store
        .register_system_declarations("com.example.camera", &declarations, 7)
        .unwrap();
    store
        .auto_grant_required(1000, "com.example.camera", &declarations)
        .unwrap();

    assert_eq!(
        store.granted_values(1000, "com.example.camera", "CAMERA"),
        vec!["front".to_string(), "back".to_string()]
    );
    assert!(
        store
            .granted_values(1000, "com.example.camera", "MICROPHONE")
            .is_empty()
    );
}

#[test]
fn rejects_values_outside_manifest_declaration() {
    let mut store = MemoryPermissionStore::new();
    let declaration = PermissionDeclaration {
        name: "NETWORK_INTERFACE".to_string(),
        values: vec!["wlan0".to_string()],
        requirement: PermissionRequirement::Optional,
        usage_description: String::new(),
    };

    let err = store
        .grant_declared(1000, "com.example.net", &declaration, &["eth0".to_string()])
        .unwrap_err();

    assert_eq!(err, PermissionStoreError::UndeclaredValue);
}

#[test]
fn redb_store_persists_system_and_user_grants() {
    let store = Arc::new(MemBlockStore::new());
    let db = bexos_permission_store::persistent::PermissionStoreDb::open(store.clone()).unwrap();
    let declaration = PermissionDeclaration {
        name: "NETWORK_INTERFACE".to_string(),
        values: vec!["wlan0".to_string()],
        requirement: PermissionRequirement::Optional,
        usage_description: "Use Wi-Fi".to_string(),
    };

    db.register_system_declarations("com.example.net", &[declaration.clone()], 42)
        .unwrap();
    db.grant_declared(
        1000,
        "com.example.net",
        &declaration,
        &["wlan0".to_string()],
    )
    .unwrap();
    drop(db);

    let reopened = bexos_permission_store::persistent::PermissionStoreDb::open(store).unwrap();
    assert_eq!(reopened.system_grants().unwrap()[0].verified_at, 42);
    assert_eq!(
        reopened.user_grants(1000, "com.example.net").unwrap()[0].granted_values,
        vec!["wlan0".to_string()]
    );
}

#[test]
fn system_snapshot_and_replace_keep_only_uid_zero_grants() {
    let store = Arc::new(MemBlockStore::new());
    let db = bexos_permission_store::persistent::PermissionStoreDb::open(store.clone()).unwrap();
    let declaration = PermissionDeclaration {
        name: "CAMERA".to_string(),
        values: vec!["front".to_string()],
        requirement: PermissionRequirement::Optional,
        usage_description: "Take photos".to_string(),
    };
    db.register_system_declarations("com.example.camera", &[declaration.clone()], 11)
        .unwrap();
    db.grant_declared(0, "com.example.camera", &declaration, &[])
        .unwrap();
    db.grant_declared(1000, "com.example.camera", &declaration, &[])
        .unwrap();

    let snapshot = db.snapshot_system_memory().unwrap();
    assert_eq!(snapshot.user_records(0).len(), 1);
    assert!(snapshot.user_records(1000).is_empty());

    db.replace_system_from_memory(&snapshot).unwrap();
    drop(db);
    let reopened = bexos_permission_store::persistent::PermissionStoreDb::open(store).unwrap();
    assert_eq!(
        reopened.user_grants(0, "com.example.camera").unwrap().len(),
        1
    );
    assert!(
        reopened
            .user_grants(1000, "com.example.camera")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn user_replace_and_snapshot_are_uid_scoped() {
    let store = Arc::new(MemBlockStore::new());
    let db = bexos_permission_store::persistent::PermissionStoreDb::open(store).unwrap();
    let camera = PermissionDeclaration {
        name: "CAMERA".to_string(),
        values: vec!["front".to_string()],
        requirement: PermissionRequirement::Optional,
        usage_description: String::new(),
    };
    let mic = PermissionDeclaration {
        name: "MICROPHONE".to_string(),
        values: vec!["default".to_string()],
        requirement: PermissionRequirement::Optional,
        usage_description: String::new(),
    };
    let mut memory = MemoryPermissionStore::new();
    memory
        .grant_declared(1000, "com.example.camera", &camera, &[])
        .unwrap();
    memory
        .grant_declared(1001, "com.example.mic", &mic, &[])
        .unwrap();
    db.replace_user_from_memory(1000, &memory).unwrap();

    let uid_1000 = db.snapshot_user_memory(1000).unwrap();
    assert_eq!(uid_1000.user_records(1000).len(), 1);
    assert!(uid_1000.user_records(1001).is_empty());
    assert!(db.user_grants(1001, "com.example.mic").unwrap().is_empty());

    db.replace_user_from_memory(1001, &memory).unwrap();
    assert_eq!(db.user_grants(1001, "com.example.mic").unwrap().len(), 1);
    db.remove_user(1001).unwrap();
    assert!(db.user_grants(1001, "com.example.mic").unwrap().is_empty());
}

#[test]
fn scrub_non_system_user_rows_removes_legacy_user_grants() {
    let store = Arc::new(MemBlockStore::new());
    let db = bexos_permission_store::persistent::PermissionStoreDb::open(store).unwrap();
    let declaration = PermissionDeclaration {
        name: "LOCATION".to_string(),
        values: vec!["coarse".to_string()],
        requirement: PermissionRequirement::Optional,
        usage_description: String::new(),
    };
    db.grant_declared(0, "com.example.maps", &declaration, &[])
        .unwrap();
    db.grant_declared(42, "com.example.maps", &declaration, &[])
        .unwrap();
    db.scrub_non_system_user_rows().unwrap();

    assert_eq!(db.user_grants(0, "com.example.maps").unwrap().len(), 1);
    assert!(db.user_grants(42, "com.example.maps").unwrap().is_empty());
}
