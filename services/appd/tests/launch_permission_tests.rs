use bexos_appd::permission_persistence::auto_grant_and_sync;
use bexos_appd::{MemoryPermissionStore, PermissionDeclaration, PermissionRequirement};
use bexos_userspace::Channel;

fn required_permission() -> PermissionDeclaration {
    PermissionDeclaration {
        name: "CAMERA".into(),
        requirement: PermissionRequirement::Required,
        ..Default::default()
    }
}

#[test]
fn existing_launch_grants_do_not_require_storage_io() {
    let declarations = [required_permission()];
    let mut memory = MemoryPermissionStore::new();
    memory
        .register_system_declarations("com.example.camera", &declarations, 0)
        .unwrap();
    memory
        .auto_grant_required(1000, "com.example.camera", &declarations)
        .unwrap();
    let before = memory.clone();
    // No usable VFS endpoint: an unchanged launch must not try to persist.
    auto_grant_and_sync(
        Channel(0),
        1000,
        "com.example.camera",
        &declarations,
        &mut memory,
    )
    .unwrap();
    assert_eq!(memory, before);
}

#[test]
fn failed_persistence_does_not_publish_new_launch_grants() {
    let mut memory = MemoryPermissionStore::new();
    let before = memory.clone();
    assert!(
        auto_grant_and_sync(
            Channel(0),
            1000,
            "com.example.camera",
            &[required_permission()],
            &mut memory
        )
        .is_err()
    );
    assert_eq!(memory, before);
}
