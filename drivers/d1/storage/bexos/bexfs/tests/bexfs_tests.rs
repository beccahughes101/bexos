use bexos_bexfs::block::{BEXFS_BLOCK_SIZE, MemoryBlockDevice};
use bexos_bexfs::key::{KeyError, LockedVolumeKey, WrappedVolumeKey, WrappedVolumeKeyProvider};
use bexos_bexfs::sys_state::{ActivePackagePin, Slot, SysStateV1, pinned_apps_path};
use bexos_bexfs::{BexFs, BexFsError, FormatOptions, NodeKind};
use bexos_package_version::{HealthCheckStatus, SemVer};

const BLOCKS: u64 = 8192;
const KEY: [u8; 32] = [0x5a; 32];
const UUID: [u8; 16] = [0x11; 16];

fn format<'a>(device: &'a mut MemoryBlockDevice) -> BexFs {
    BexFs::format(
        device,
        LockedVolumeKey::new(&KEY).unwrap(),
        FormatOptions {
            label: "SYS_STATE",
            volume_uuid: UUID,
            device_uuid: [0x22; 16],
        },
    )
    .unwrap()
}

#[test]
fn encrypted_sys_state_roundtrips_across_remount() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    let mut handle = fs
        .open(fs.root_inode(), "boot_state.bin", 0x1 | 0x2 | 0x8)
        .unwrap();
    let expected = SysStateV1 {
        generation: 2,
        active_slot: Slot::B,
        tries_remaining: [2, 3],
        last_known_good_slot: Slot::A,
        last_known_good_version: 1,
    };
    fs.write(&mut handle, &expected.encode()).unwrap();
    fs.close(&mut device).unwrap();

    let image = device.snapshot();
    assert!(!image.windows(8).any(|window| window == b"BEXSYS01"));

    let mut mounted = BexFs::mount(
        &mut device,
        LockedVolumeKey::new(&KEY).unwrap(),
        "SYS_STATE",
        false,
    )
    .unwrap();
    let mut handle = mounted
        .open(mounted.root_inode(), "boot_state.bin", 0x1)
        .unwrap();
    let bytes = mounted.read(&mut handle, 4096).unwrap();
    assert_eq!(SysStateV1::decode(&bytes).unwrap(), expected);
    assert!(mounted.generation() >= 2);
}

#[test]
fn wrong_key_and_unsafe_paths_are_rejected() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    let root = fs.open(fs.root_inode(), ".", 0x1 | 0x20).unwrap();
    assert_eq!(root.inode(), fs.root_inode());
    assert_eq!(
        fs.open(fs.root_inode(), "/absolute", 0x1),
        Err(BexFsError::InvalidArgs)
    );
    assert_eq!(
        fs.open(fs.root_inode(), "../escape", 0x1),
        Err(BexFsError::InvalidArgs)
    );
    drop(fs);
    assert!(matches!(
        BexFs::mount(
            &mut device,
            LockedVolumeKey::new(&[0x7b; 32]).unwrap(),
            "SYS_STATE",
            false,
        ),
        Err(BexFsError::Locked)
    ));
}

#[test]
fn sparse_files_synthesize_holes_and_account_allocated_extents() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    let mut file = fs
        .open(fs.root_inode(), "sparse.bin", 0x1 | 0x2 | 0x8)
        .unwrap();
    fs.seek(&mut file, 12 * 4096, 0).unwrap();
    fs.write(&mut file, b"tail").unwrap();

    let attr = fs.attributes(file.inode()).unwrap();
    assert_eq!(attr.size_bytes, 12 * 4096 + 4);
    assert_eq!(attr.storage_allocated_bytes, 4096);

    fs.seek(&mut file, 4090, 0).unwrap();
    fs.write(&mut file, b"cross-block").unwrap();
    let attr = fs.attributes(file.inode()).unwrap();
    assert_eq!(attr.storage_allocated_bytes, 3 * 4096);

    fs.seek(&mut file, 0, 0).unwrap();
    let bytes = fs.read(&mut file, 4096).unwrap();
    assert_eq!(&bytes[..4090], &[0; 4090]);
    assert_eq!(&bytes[4090..], b"cross-");

    fs.set_len(&file, 4096).unwrap();
    let attr = fs.attributes(file.inode()).unwrap();
    assert_eq!(attr.size_bytes, 4096);
    assert_eq!(attr.storage_allocated_bytes, 1 * 4096);
}

#[test]
fn sparse_namespace_persists_without_materializing_holes() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    let mut file = fs
        .open(fs.root_inode(), "persist.bin", 0x1 | 0x2 | 0x8)
        .unwrap();
    fs.seek(&mut file, 2 * 1024 * 1024, 0).unwrap();
    fs.write(&mut file, b"marker").unwrap();
    assert_eq!(
        fs.attributes(file.inode()).unwrap().storage_allocated_bytes,
        4096
    );
    fs.close(&mut device).unwrap();

    let mut mounted = BexFs::mount(
        &mut device,
        LockedVolumeKey::new(&KEY).unwrap(),
        "SYS_STATE",
        false,
    )
    .unwrap();
    let mut file = mounted
        .open(mounted.root_inode(), "persist.bin", 0x1)
        .unwrap();
    mounted.seek(&mut file, 2 * 1024 * 1024, 0).unwrap();
    assert_eq!(mounted.read(&mut file, 6).unwrap(), b"marker");
    assert_eq!(
        mounted
            .attributes(file.inode())
            .unwrap()
            .storage_allocated_bytes,
        4096
    );
}

#[test]
fn namespace_persistence_streams_large_external_data() {
    const LARGE_BLOCKS: u64 = 200_000;
    const PAYLOAD_BYTES: usize = 33 * 1024 * 1024;

    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, LARGE_BLOCKS);
    let mut fs = format(&mut device);
    let mut file = fs
        .open(fs.root_inode(), "large.bin", 0x1 | 0x2 | 0x8)
        .unwrap();
    fs.write(&mut file, &vec![0xa5; PAYLOAD_BYTES]).unwrap();
    fs.close(&mut device).unwrap();

    let mut mounted = BexFs::mount(
        &mut device,
        LockedVolumeKey::new(&KEY).unwrap(),
        "SYS_STATE",
        false,
    )
    .unwrap();
    let mut file = mounted
        .open(mounted.root_inode(), "large.bin", 0x1)
        .unwrap();
    mounted
        .seek(&mut file, (PAYLOAD_BYTES - 16) as i64, 0)
        .unwrap();
    assert_eq!(mounted.read(&mut file, 16).unwrap(), vec![0xa5; 16]);
}

#[test]
fn active_package_pin_round_trips_for_slot_scoped_registry() {
    assert_eq!(pinned_apps_path(Slot::A), "slot_a/pinned_apps.redb");
    assert_eq!(pinned_apps_path(Slot::B), "slot_b/pinned_apps.redb");

    let pin = ActivePackagePin {
        package_id: "com.example:demo".to_string(),
        pinned_version: semver(1, 3, 0, 201),
        content_blake3: [0x7a; 32],
        is_critical_boot_app: true,
        health_check_status: HealthCheckStatus::Probation,
        rollback_target_version: Some(semver(1, 2, 0, 104)),
    };

    assert_eq!(ActivePackagePin::decode(&pin.encode()).unwrap(), pin);
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

#[test]
fn directory_entries_and_unlink_enforce_type_and_nonempty_rules() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    let directory = fs
        .open(fs.root_inode(), "logs", 0x1 | 0x2 | 0x8 | 0x20)
        .unwrap();
    let mut file = fs
        .open(directory.inode(), "panic.log", 0x1 | 0x2 | 0x8)
        .unwrap();
    fs.write(&mut file, b"panic").unwrap();
    let entries = fs.read_entries(directory.inode()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "panic.log");
    assert_eq!(entries[0].kind, NodeKind::File);
    assert_eq!(
        fs.unlink(fs.root_inode(), "logs"),
        Err(BexFsError::NotEmpty)
    );
    fs.unlink(directory.inode(), "panic.log").unwrap();
    fs.unlink(fs.root_inode(), "logs").unwrap();
}

#[test]
fn rename_moves_nodes_and_replaces_compatible_targets() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    fs.open(fs.root_inode(), "a", 0x1 | 0x2 | 0x8).unwrap();
    fs.rename(fs.root_inode(), "a", "b").unwrap();
    assert_eq!(
        fs.open(fs.root_inode(), "a", 0x1),
        Err(BexFsError::NotFound)
    );
    assert!(fs.open(fs.root_inode(), "b", 0x1).is_ok());
    fs.open(fs.root_inode(), "c", 0x1 | 0x2 | 0x8).unwrap();
    fs.rename(fs.root_inode(), "b", "c").unwrap();
    assert!(fs.open(fs.root_inode(), "c", 0x1).is_ok());
}

#[test]
fn hard_links_share_storage_across_remount_and_live_migration() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    let mut original = fs.open(1, "original", 0x1 | 0x2 | 0x8).unwrap();
    fs.write(&mut original, b"durable shared data").unwrap();
    fs.link(1, "original", "alias").unwrap();
    let alias = fs.open(1, "alias", 0x1).unwrap();
    assert_eq!(original.inode(), alias.inode());

    let metadata = fs.checkpoint_record(0).unwrap().unwrap();
    let mut migrated = BexFs::adopt_metadata(&metadata).unwrap();
    for key in fs.checkpoint_keys().into_iter().filter(|key| *key != 0) {
        migrated
            .adopt_record(key, fs.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    migrated.validate_checkpoint().unwrap();
    migrated.unlink(1, "original").unwrap();
    let mut alias = migrated.open(1, "alias", 0x1).unwrap();
    assert_eq!(
        migrated.read(&mut alias, 64).unwrap(),
        b"durable shared data"
    );

    fs.close(&mut device).unwrap();
    let mut mounted = BexFs::mount(
        &mut device,
        LockedVolumeKey::new(&KEY).unwrap(),
        "SYS_STATE",
        false,
    )
    .unwrap();
    let original = mounted.open(1, "original", 0x1).unwrap();
    let mut alias = mounted.open(1, "alias", 0x1).unwrap();
    assert_eq!(original.inode(), alias.inode());
    assert_eq!(
        mounted.read(&mut alias, 64).unwrap(),
        b"durable shared data"
    );
}

#[test]
fn symlinks_and_xattrs_persist_and_migrate() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut fs = format(&mut device);
    let mut original = fs.open(1, "original", 0x1 | 0x2 | 0x8).unwrap();
    fs.write(&mut original, b"payload").unwrap();
    let directory = fs.open(1, "links", 0x1 | 0x8 | 0x20).unwrap();
    fs.symlink(directory.inode(), "../original", "relative")
        .unwrap();
    fs.link(directory.inode(), "relative", "relative_alias")
        .unwrap();
    fs.symlink(1, "/original", "absolute").unwrap();
    fs.set_xattr(original.inode(), "user.oci", b"preserved", 1)
        .unwrap();

    let metadata = fs.checkpoint_record(0).unwrap().unwrap();
    let mut migrated = BexFs::adopt_metadata(&metadata).unwrap();
    for key in fs.checkpoint_keys().into_iter().filter(|key| *key != 0) {
        migrated
            .adopt_record(key, fs.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    migrated.validate_checkpoint().unwrap();
    assert_eq!(
        migrated
            .readlink(directory.inode(), "relative_alias")
            .unwrap(),
        "../original"
    );
    let mut relative = migrated.open(directory.inode(), "relative", 0x1).unwrap();
    assert_eq!(migrated.read(&mut relative, 16).unwrap(), b"payload");
    assert_eq!(
        migrated.get_xattr(relative.inode(), "user.oci").unwrap(),
        b"preserved"
    );

    fs.close(&mut device).unwrap();
    let mut mounted = BexFs::mount(
        &mut device,
        LockedVolumeKey::new(&KEY).unwrap(),
        "SYS_STATE",
        false,
    )
    .unwrap();
    assert_eq!(mounted.readlink(1, "absolute").unwrap(), "/original");
    let mut absolute = mounted.open(1, "absolute", 0x1).unwrap();
    assert_eq!(mounted.read(&mut absolute, 16).unwrap(), b"payload");
    assert_eq!(mounted.list_xattrs(absolute.inode()).unwrap(), ["user.oci"]);
}

#[test]
fn sys_state_codec_detects_corruption() {
    let state = SysStateV1::initial();
    let mut bytes = state.encode();
    assert_eq!(SysStateV1::decode(&bytes).unwrap(), state);
    bytes[20] ^= 1;
    assert!(SysStateV1::decode(&bytes).is_err());
}

#[test]
fn checkpoint_preserves_patterned_record_boundaries_and_sparse_holes() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut old = format(&mut device);
    let mut file = old.open(1, "pattern", 1 | 2 | 8).unwrap();
    let pattern: Vec<u8> = (0..100_123)
        .map(|i| ((i * 37 + i / 4096) % 251) as u8)
        .collect();
    old.write(&mut file, &pattern).unwrap();
    let mut sparse = old.open(1, "sparse", 1 | 2 | 8).unwrap();
    old.write(&mut sparse, b"head").unwrap();
    old.seek(&mut sparse, 10 * 1024 * 1024, 0).unwrap();
    old.write(&mut sparse, b"tail").unwrap();
    let keys = old.checkpoint_keys();
    assert!(keys.len() < 20, "sparse holes must not become bulk records");
    let metadata = old.checkpoint_record(0).unwrap().unwrap();
    let mut new = BexFs::adopt_metadata(&metadata).unwrap();
    for key in keys.into_iter().filter(|key| *key != 0) {
        new.adopt_record(key, old.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.validate_checkpoint().unwrap();
    assert_eq!(new.checkpoint_keys(), old.checkpoint_keys());
    let mut restored = new.open(1, "pattern", 1).unwrap();
    assert_eq!(
        new.read(&mut restored, pattern.len() as u64).unwrap(),
        pattern
    );
    let mut restored = new.open(1, "sparse", 1).unwrap();
    assert_eq!(new.read(&mut restored, 4).unwrap(), b"head");
    new.seek(&mut restored, 5 * 1024 * 1024, 0).unwrap();
    assert_eq!(new.read(&mut restored, 4096).unwrap(), vec![0; 4096]);
    new.seek(&mut restored, 10 * 1024 * 1024, 0).unwrap();
    assert_eq!(new.read(&mut restored, 4).unwrap(), b"tail");
    old.take_changes();
    old.seek(&mut file, 32704, 0).unwrap();
    old.write(&mut file, &vec![0; 32704]).unwrap();
    for key in old.take_changes() {
        new.adopt_record(key, old.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    assert_eq!(new.checkpoint_keys(), old.checkpoint_keys());
    let mut restored = new.open(1, "pattern", 1).unwrap();
    let mut expected = pattern;
    expected[32704..65408].fill(0);
    assert_eq!(
        new.read(&mut restored, expected.len() as u64).unwrap(),
        expected
    );
}

#[test]
fn incremental_checkpoint_carries_concurrent_writes_truncation_and_cursors() {
    use bexos_migration::codec::{Decoder, Encoder};
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, BLOCKS);
    let mut old = format(&mut device);
    let mut file = old.open(1, "live", 1 | 2 | 8).unwrap();
    old.write(&mut file, &vec![7; 48 * 1024]).unwrap();
    old.take_changes();
    let keys = old.checkpoint_keys();
    let metadata = old.checkpoint_record(0).unwrap().unwrap();
    assert!(!metadata.windows(32).any(|b| b == KEY));
    let mut new = BexFs::adopt_metadata(&metadata).unwrap();
    for key in keys.into_iter().filter(|k| *k != 0) {
        new.adopt_record(key, old.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
        if key == file.inode() << 16 | 1 {
            old.seek(&mut file, 12, 0).unwrap();
            old.write(&mut file, b"delta").unwrap();
        }
    }
    let mut new_file = old.open(1, "created-during-bulk", 1 | 2 | 8).unwrap();
    old.write(&mut new_file, b"new").unwrap();
    for key in old.take_changes() {
        new.adopt_record(key, old.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.validate_checkpoint().unwrap();
    let mut w = Encoder::new();
    file.checkpoint(&mut w);
    let bytes = w.finish();
    let mut saved = bexos_bexfs::FileHandle::adopt(&mut Decoder::new(&bytes)).unwrap();
    assert_eq!(
        old.read(&mut file, 100).unwrap(),
        new.read(&mut saved, 100).unwrap()
    );
    let mut old_file = old.open(1, "live", 1 | 2 | 16).unwrap();
    old.write(&mut old_file, b"short").unwrap();
    for key in old.take_changes() {
        new.adopt_record(key, old.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    let mut new_file = new.open(1, "live", 1).unwrap();
    assert_eq!(new.read(&mut new_file, 100).unwrap(), b"short");
    new.activate_key(LockedVolumeKey::new(&KEY).unwrap());
    new.sync(&mut device).unwrap();
    let mut remounted = BexFs::mount(
        &mut device,
        LockedVolumeKey::new(&KEY).unwrap(),
        "SYS_STATE",
        false,
    )
    .unwrap();
    let mut f = remounted.open(1, "created-during-bulk", 1).unwrap();
    assert_eq!(remounted.read(&mut f, 10).unwrap(), b"new");
}

#[test]
fn wrapped_volume_key_provider_rejects_wrong_volume_uuid() {
    struct Provider;

    impl WrappedVolumeKeyProvider for Provider {
        fn unwrap(&self, wrapped: &WrappedVolumeKey) -> Result<LockedVolumeKey, KeyError> {
            if wrapped.volume_uuid != [1; 16] || wrapped.wrapped_key != b"wrapped" {
                return Err(KeyError::InvalidWrappedKey);
            }
            LockedVolumeKey::new(&KEY)
        }
    }

    let provider = Provider;
    assert!(
        provider
            .unwrap(&WrappedVolumeKey {
                volume_uuid: [1; 16],
                wrapped_key: b"wrapped".to_vec(),
            })
            .is_ok()
    );
    assert!(matches!(
        provider.unwrap(&WrappedVolumeKey {
            volume_uuid: [2; 16],
            wrapped_key: b"wrapped".to_vec(),
        }),
        Err(KeyError::InvalidWrappedKey)
    ));
}
