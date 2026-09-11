use std::sync::Arc;

use bexos_app_archive::{BuildEntry, Compression, TrustedKey, build_archive};
use bexos_app_registry::persistent::AppRegistryDb;
use bexos_app_registry::{
    ActivePinRecord, AppRecord, HealthCheckStatus, InstallRequest, InstallSource, LifecycleState,
    MemoryAppRegistry, RegistryError, SemVer, VerifiedSignerMetadata,
};
use bexos_redb::mem::MemBlockStore;

const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];

#[test]
fn memory_registry_installs_lists_and_loads_manifest() {
    let archive = signed_archive("com.example:demo", "Demo");
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();

    let record = registry
        .install_bundle(InstallRequest {
            archive_bytes: &archive,
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Debugd,
            protected: false,
            archive_path: "pkg/com.example_demo.bex",
        })
        .unwrap();

    assert_eq!(record.package_id, "com.example:demo");
    assert_eq!(record.display_name, "Demo");
    assert_eq!(registry.list_packages().len(), 1);
    assert_eq!(
        registry.load_manifest("com.example:demo").unwrap(),
        manifest("com.example:demo", "Demo")
    );
}

#[test]
fn system_image_manifest_cache_record_survives_migration_checkpoint() {
    let manifest = manifest("bexos.driver.network.virtio_net", "virtio-net");
    let mut registry = MemoryAppRegistry::new();
    let record = registry
        .import_manifest_cache(
            &manifest,
            InstallSource::SystemImage,
            true,
            "pkg/bexos.driver.network.virtio_net.bex",
        )
        .unwrap();

    let restored = AppRecord::from_checkpoint(&record.checkpoint()).unwrap();

    assert_eq!(restored.package_id, record.package_id);
    assert_eq!(restored.archive_path, record.archive_path);
    assert_eq!(restored.install_source, InstallSource::SystemImage);
}

#[test]
fn accepted_generation_survives_checkpoint_round_trip() {
    let manifest = manifest("bexos.service.demo", "demo");
    let mut registry = MemoryAppRegistry::new();
    let mut record = registry
        .import_manifest_cache(
            &manifest,
            InstallSource::SystemImage,
            true,
            "pkg/bexos.service.demo.generation_7.bex",
        )
        .unwrap();
    record.accepted_generation = 7;

    let restored = AppRecord::from_checkpoint(&record.checkpoint()).unwrap();

    assert_eq!(restored.accepted_generation, 7);
    assert_eq!(restored.archive_id(), "bexos.service.demo.generation_7");
}

#[test]
fn persistent_active_commit_refuses_to_lower_generation() {
    let store = Arc::new(MemBlockStore::new());
    let archive = signed_archive("bexos.service.demo", "demo");
    let keys = trusted_keys();
    let registry = AppRegistryDb::open(store).unwrap();
    let mut record = registry
        .install_bundle(InstallRequest {
            archive_bytes: &archive,
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Debugd,
            protected: false,
            archive_path: "pkg/bexos.service.demo.bex",
        })
        .unwrap();

    record.accepted_generation = 9;
    record.archive_content_root[0] ^= 0x5a;
    let mut memory = registry.snapshot_memory().unwrap();
    memory.replace_checkpoint_record(record.clone()).unwrap();
    assert_eq!(
        memory
            .active_pin(&record.package_id)
            .unwrap()
            .content_blake3,
        record.archive_content_root
    );
    registry.commit_active_record(&record).unwrap();
    assert_eq!(
        registry
            .record(&record.package_id)
            .unwrap()
            .archive_content_root,
        record.archive_content_root
    );
    assert_eq!(
        registry
            .snapshot_memory()
            .unwrap()
            .active_pin(&record.package_id)
            .unwrap()
            .content_blake3,
        record.archive_content_root
    );
    record.accepted_generation = 8;

    assert_eq!(
        registry.commit_active_record(&record),
        Err(RegistryError::CorruptRecord)
    );
    assert_eq!(
        registry
            .record("bexos.service.demo")
            .unwrap()
            .accepted_generation,
        9
    );
}

#[test]
fn protected_packages_cannot_be_uninstalled() {
    let archive = signed_archive("bexos.service.vfsd", "vfsd");
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();
    registry
        .import_boot_bundle(&archive, &keys, "boot/pkg/bexos.service.vfsd.bex")
        .unwrap();

    assert_eq!(
        registry.uninstall_package("bexos.service.vfsd").map(|_| ()),
        Err(RegistryError::ProtectedPackage)
    );
}

#[test]
fn mutable_install_cannot_replace_protected_package() {
    let archive = signed_archive("bexos.service.vfsd", "vfsd");
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();
    registry
        .import_boot_bundle(&archive, &keys, "boot/pkg/bexos.service.vfsd.bex")
        .unwrap();

    assert_eq!(
        registry
            .install_bundle(InstallRequest {
                archive_bytes: &archive,
                trusted_keys: &keys,
                verified_signer: Some(test_signer()),
                source: InstallSource::Debugd,
                protected: false,
                archive_path: "pkg/bexos.service.vfsd.bex",
            })
            .unwrap_err(),
        RegistryError::ProtectedPackage
    );
}

#[test]
fn redb_registry_denies_mutable_replacement_of_protected_package() {
    let store = Arc::new(MemBlockStore::new());
    let archive = signed_archive("bexos.service.vfsd", "vfsd");
    let keys = trusted_keys();
    let registry = AppRegistryDb::open(store).unwrap();
    registry
        .import_boot_bundle(&archive, &keys, "boot/pkg/bexos.service.vfsd.bex")
        .unwrap();

    assert_eq!(
        registry
            .install_bundle(InstallRequest {
                archive_bytes: &archive,
                trusted_keys: &keys,
                verified_signer: Some(test_signer()),
                source: InstallSource::Debugd,
                protected: false,
                archive_path: "pkg/bexos.service.vfsd.bex",
            })
            .unwrap_err(),
        RegistryError::ProtectedPackage
    );
}

#[test]
fn lifecycle_updates_record() {
    let archive = signed_archive("com.example:demo", "Demo");
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();
    registry
        .install_bundle(InstallRequest {
            archive_bytes: &archive,
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Debugd,
            protected: false,
            archive_path: "pkg/com.example_demo.bex",
        })
        .unwrap();

    registry
        .mark_lifecycle("com.example:demo", LifecycleState::Running)
        .unwrap();

    assert_eq!(
        registry.record("com.example:demo").unwrap().lifecycle_state,
        LifecycleState::Running
    );
}

#[test]
fn single_active_registry_keeps_multiple_versions_and_rollback_pin() {
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();
    let v1 = semver(1, 2, 0, 104);
    let v2 = semver(1, 3, 0, 201);

    let first = registry
        .install_bundle(InstallRequest {
            archive_bytes: &signed_archive_with_version("com.example:demo", "Demo", &v1),
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Debugd,
            protected: true,
            archive_path: "pkg/com.example:demo/1.2.0-b104/pkg.bex",
        })
        .unwrap();
    assert_eq!(first.package_key(), "com.example:demo:1.2.0-b104");

    let second = registry
        .install_bundle(InstallRequest {
            archive_bytes: &signed_archive_with_version("com.example:demo", "Demo", &v2),
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Bootfs,
            protected: true,
            archive_path: "boot/pkg/com.example:demo/1.3.0-b201/pkg.bex",
        })
        .unwrap();

    assert_eq!(registry.list_packages().len(), 2);
    assert_eq!(registry.record("com.example:demo").unwrap().version, v2);
    assert_eq!(
        registry
            .active_pin("com.example:demo")
            .unwrap()
            .rollback_target_version,
        Some(v1.clone())
    );
    assert_eq!(
        registry
            .active_pin("com.example:demo")
            .unwrap()
            .health_check_status,
        HealthCheckStatus::Probation
    );
    assert_eq!(second.package_key(), "com.example:demo:1.3.0-b201");

    let restored = registry.rollback_to_previous("com.example:demo").unwrap();
    assert_eq!(restored, v1);
    assert_eq!(registry.record("com.example:demo").unwrap().version, v1);
    assert_eq!(
        registry
            .active_pin("com.example:demo")
            .unwrap()
            .rollback_target_version,
        None
    );
}

#[test]
fn active_pins_survive_checkpoint_reopen_exactly() {
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();
    let v1 = semver(1, 0, 0, 0);
    let v2 = semver(2, 0, 0, 0);
    registry
        .install_bundle(InstallRequest {
            archive_bytes: &signed_archive_with_version("com.example:demo", "Demo", &v1),
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Debugd,
            protected: true,
            archive_path: "pkg/com.example/demo/1/pkg.bex",
        })
        .unwrap();
    registry
        .install_bundle(InstallRequest {
            archive_bytes: &signed_archive_with_version("com.example:demo", "Demo", &v2),
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Debugd,
            protected: true,
            archive_path: "pkg/com.example/demo/2/pkg.bex",
        })
        .unwrap();
    registry
        .mark_pin_health("com.example:demo", HealthCheckStatus::CrashLoop)
        .unwrap();

    let records = registry.list_packages().to_vec();
    let pins = registry.active_pins().to_vec();
    let restored = MemoryAppRegistry::from_checkpoint_records_and_pins(records, pins.clone())
        .expect("restore exact pins");

    assert_eq!(restored.active_pins(), pins.as_slice());
}

#[test]
fn active_pin_digest_validation_fails_closed() {
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();
    let version = semver(1, 0, 0, 0);
    let record = registry
        .install_bundle(InstallRequest {
            archive_bytes: &signed_archive_with_version("com.example:demo", "Demo", &version),
            trusted_keys: &keys,
            verified_signer: Some(test_signer()),
            source: InstallSource::Debugd,
            protected: true,
            archive_path: "pkg/com.example/demo/1/pkg.bex",
        })
        .unwrap();
    let mut bad_digest = record.archive_content_root;
    bad_digest[0] ^= 1;

    assert_eq!(
        registry.replace_active_pins(vec![ActivePinRecord {
            package_id: "com.example:demo".into(),
            pinned_version: version,
            content_blake3: bad_digest,
            is_critical_boot_app: true,
            health_check_status: HealthCheckStatus::Probation,
            rollback_target_version: None,
        }]),
        Err(RegistryError::CorruptRecord)
    );
}

#[test]
fn prune_candidates_keep_active_newest_and_probation_fallback() {
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();
    for version in [
        semver(1, 0, 0, 0),
        semver(1, 1, 0, 0),
        semver(1, 2, 0, 0),
        semver(1, 3, 0, 0),
    ] {
        registry
            .install_bundle(InstallRequest {
                archive_bytes: &signed_archive_with_version("com.example:demo", "Demo", &version),
                trusted_keys: &keys,
                verified_signer: Some(test_signer()),
                source: InstallSource::Debugd,
                protected: true,
                archive_path: "pkg/com.example/demo/pkg.bex",
            })
            .unwrap();
    }

    let candidates = registry
        .prune_inactive_candidates("com.example:demo", 1)
        .unwrap();

    assert_eq!(
        candidates
            .iter()
            .map(|record| record.version.clone())
            .collect::<Vec<_>>(),
        vec![semver(1, 0, 0, 0)]
    );
}

#[test]
fn redb_registry_persists_across_reopen() {
    let store = Arc::new(MemBlockStore::new());
    let archive = signed_archive("com.example:demo", "Demo");
    let keys = trusted_keys();
    {
        let registry = AppRegistryDb::open(store.clone()).unwrap();
        registry
            .install_bundle(InstallRequest {
                archive_bytes: &archive,
                trusted_keys: &keys,
                verified_signer: Some(test_signer()),
                source: InstallSource::Debugd,
                protected: false,
                archive_path: "pkg/com.example_demo.bex",
            })
            .unwrap();
        registry
            .mark_lifecycle("com.example:demo", LifecycleState::Stopped)
            .unwrap();
    }

    let registry = AppRegistryDb::open(store).unwrap();
    let record = registry.record("com.example:demo").unwrap();

    assert_eq!(record.package_id, "com.example:demo");
    assert_eq!(record.lifecycle_state, LifecycleState::Stopped);
}

#[test]
fn invalid_manifest_is_rejected() {
    let archive = build_archive(
        &[BuildEntry {
            path: "package.bexmanifest",
            bytes: b"\x08\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01",
            mode: 0o444,
        }],
        Compression::None,
        KEY_ID,
        SEED,
    )
    .unwrap();
    let keys = trusted_keys();
    let mut registry = MemoryAppRegistry::new();

    assert_eq!(
        registry
            .install_bundle(InstallRequest {
                archive_bytes: &archive.bytes,
                trusted_keys: &keys,
                verified_signer: Some(test_signer()),
                source: InstallSource::Debugd,
                protected: false,
                archive_path: "pkg/bad.bex",
            })
            .unwrap_err(),
        RegistryError::ManifestDecode
    );
}

fn signed_archive(package_id: &str, name: &str) -> Vec<u8> {
    signed_archive_with_manifest(&manifest(package_id, name))
}

fn signed_archive_with_version(package_id: &str, name: &str, version: &SemVer) -> Vec<u8> {
    signed_archive_with_manifest(&manifest_with_version(package_id, name, version))
}

fn signed_archive_with_manifest(manifest: &[u8]) -> Vec<u8> {
    build_archive(
        &[BuildEntry {
            path: "package.bexmanifest",
            bytes: manifest,
            mode: 0o444,
        }],
        Compression::None,
        KEY_ID,
        SEED,
    )
    .unwrap()
    .bytes
}

fn trusted_keys() -> [TrustedKey<'static>; 1] {
    const PUBLIC: [u8; 32] = [
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ];
    [TrustedKey {
        key_id: KEY_ID,
        public_key: &PUBLIC,
    }]
}

fn test_signer() -> VerifiedSignerMetadata {
    VerifiedSignerMetadata {
        root_anchor_id: "bexos-dev-app-root".into(),
        leaf_certificate_fingerprint: [9; 32],
        signature_algorithm: 1,
        granted_trust_tier: 1,
    }
}

fn manifest(package_id: &str, name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, package_id);
    put_string(&mut out, 2, name);
    out
}

fn manifest_with_version(package_id: &str, name: &str, version: &SemVer) -> Vec<u8> {
    let mut out = manifest(package_id, name);
    let mut semver = Vec::new();
    put_varint_field(&mut semver, 1, version.major as u64);
    put_varint_field(&mut semver, 2, version.minor as u64);
    put_varint_field(&mut semver, 3, version.patch as u64);
    put_varint_field(&mut semver, 4, version.build as u64);
    put_message(&mut out, 13, &semver);
    out
}

fn put_string(out: &mut Vec<u8>, field: u32, value: &str) {
    put_varint(out, u64::from(field << 3 | 2));
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

fn put_message(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_varint(out, u64::from(field << 3 | 2));
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    put_varint(out, u64::from(field << 3));
    put_varint(out, value);
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn semver(major: u32, minor: u32, patch: u32, build: u32) -> SemVer {
    SemVer {
        major,
        minor,
        patch,
        build,
        prerelease: String::new(),
    }
}

fn native_manifest(architecture: Option<u64>) -> Vec<u8> {
    let mut bytes = manifest("com.example:native", "Native");
    bytes.extend_from_slice(&[0x1a, 5, 0x12, 3, b'e', b'l', b'f']);
    if let Some(architecture) = architecture {
        put_varint_field(&mut bytes, 20, architecture);
    }
    bytes
}

#[test]
fn native_installs_require_a_matching_explicit_architecture() {
    let incompatible = 3 - bexos_app_manifest::Architecture::current_guest() as u64;
    for architecture in [None, Some(0), Some(incompatible)] {
        let archive = signed_archive_with_manifest(&native_manifest(architecture));
        let mut registry = MemoryAppRegistry::new();
        let keys = trusted_keys();
        assert!(matches!(
            registry.install_bundle(InstallRequest {
                archive_bytes: &archive,
                trusted_keys: &keys,
                verified_signer: Some(test_signer()),
                source: InstallSource::Debugd,
                protected: false,
                archive_path: "pkg/native.bex",
            }),
            Err(RegistryError::Architecture(_))
        ));
        assert!(registry.list_packages().is_empty());
    }
}

#[test]
fn unlabelled_durable_native_records_are_unavailable_without_breaking_other_packages() {
    let mut memory = MemoryAppRegistry::new();
    let mut legacy = memory
        .import_manifest_cache(
            &manifest("com.example:native", "Native"),
            InstallSource::SystemImage,
            false,
            "pkg/native.bex",
        )
        .unwrap();
    legacy.manifest_bytes = native_manifest(None);
    memory.install_checkpoint_record(legacy.clone());
    memory
        .import_manifest_cache(
            &manifest("com.example:portable", "Portable"),
            InstallSource::SystemImage,
            false,
            "pkg/portable.bex",
        )
        .unwrap();
    let store = Arc::new(MemBlockStore::new());
    let db = AppRegistryDb::open(store.clone()).unwrap();
    db.replace_from_memory(&memory).unwrap();
    drop(db);
    let db = AppRegistryDb::open(store).unwrap();
    let mut restored = db.snapshot_memory().unwrap();
    assert!(restored.record("com.example:portable").is_ok());
    assert!(matches!(
        restored.record("com.example:native"),
        Err(RegistryError::Architecture(_))
    ));
    let unavailable = restored
        .list_packages()
        .iter()
        .find(|r| r.package_id == "com.example:native")
        .unwrap();
    assert_eq!(unavailable.manifest_bytes, legacy.manifest_bytes);
    assert!(
        unavailable
            .architecture_diagnostic()
            .unwrap()
            .contains("rebuild and reinstall")
    );
    let encoded = unavailable.checkpoint();
    assert_eq!(
        AppRecord::from_checkpoint(&encoded).unwrap().manifest_bytes,
        legacy.manifest_bytes
    );
    let removed = db.uninstall_package("com.example:native").unwrap();
    assert_eq!(removed.manifest_bytes, legacy.manifest_bytes);
    assert!(matches!(
        db.record("com.example:native"),
        Err(RegistryError::NotFound)
    ));
    assert!(db.record("com.example:portable").is_ok());
    restored
        .import_manifest_cache(
            &native_manifest(Some(
                bexos_app_manifest::Architecture::current_guest() as u64
            )),
            InstallSource::SystemImage,
            false,
            "pkg/native-rebuilt.bex",
        )
        .unwrap();
    assert!(restored.record("com.example:native").is_ok());
    assert_eq!(restored.list_packages().len(), 2);
}

#[test]
fn replacement_cannot_change_native_architecture_or_active_pin() {
    let mut registry = MemoryAppRegistry::new();
    let mut candidate = registry
        .import_manifest_cache(
            &native_manifest(Some(
                bexos_app_manifest::Architecture::current_guest() as u64
            )),
            InstallSource::SystemImage,
            false,
            "pkg/native.bex",
        )
        .unwrap();
    let pin = registry.active_pin("com.example:native").unwrap().clone();
    candidate.manifest_bytes = native_manifest(Some(
        3 - bexos_app_manifest::Architecture::current_guest() as u64,
    ));
    assert!(registry.replace_checkpoint_record(candidate).is_err());
    assert_eq!(registry.active_pin("com.example:native"), Some(&pin));
    assert!(registry.record("com.example:native").is_ok());
}
