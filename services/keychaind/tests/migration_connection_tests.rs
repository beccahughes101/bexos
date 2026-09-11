use bexos_keychaind::migration::Runtime;
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};

#[test]
fn independent_user_connections_survive_handover() {
    let mut source = Runtime::empty();
    source.control = Channel(1);
    source.migration = Some(Channel(2));
    source.users = Channel(3);
    source.user_auth = Channel(4);
    source.validate().unwrap();
    let record = source.encode_record(0).unwrap().unwrap();
    let mut target = Runtime::empty();
    target.adopt_record(0, Some(&record)).unwrap();
    assert!(target.validate().is_err());
    for key in source.keys().into_iter().skip(1) {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.validate().unwrap();
    target.activated(1);
    assert_eq!(target.users.0, 3);
    assert_eq!(target.user_auth.0, 4);
    assert!(
        target
            .resources()
            .iter()
            .any(|r| matches!(r, Resource::Handle(3)))
    );
    assert!(
        target
            .resources()
            .iter()
            .any(|r| matches!(r, Resource::Handle(4)))
    );
    target.user_auth = target.users;
    assert!(target.validate().is_err());
}

#[test]
fn old_shared_connection_checkpoint_is_rejected() {
    let mut target = Runtime::empty();
    let mut encoder = bexos_migration::codec::Encoder::new();
    encoder.word(1);
    assert_eq!(
        target.adopt_record(0, Some(&encoder.finish())),
        Err(bexos_migration::Error::UnsupportedVersion)
    );
}

#[test]
fn volatile_system_and_user_vaults_survive_chunked_migration_without_hardware_downgrade() {
    use keychain_fidl::{KeyAlgorithm, KeyFlags, KeychainScope, KeychainStatus};
    let mut source = Runtime::empty();
    source.control = Channel(1);
    source.migration = Some(Channel(2));
    source.users = Channel(3);
    source.user_auth = Channel(4);
    for index in 0..6 {
        assert_eq!(
            source.service.store_secret(
                KeychainScope::System,
                0,
                &format!("secret{index}"),
                &vec![index; 4096],
                KeyFlags(0),
                true,
                None
            ),
            KeychainStatus::Ok
        );
    }
    assert_eq!(
        source.service.store_secret(
            KeychainScope::User,
            1000,
            "personal",
            b"retained",
            KeyFlags(0),
            true,
            None
        ),
        KeychainStatus::Ok
    );
    let mut target = Runtime::empty();
    for key in source.keys() {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.validate().unwrap();
    target.activated(7);
    for index in 0..6 {
        assert_eq!(
            target.service.get_secret(
                KeychainScope::System,
                0,
                &format!("secret{index}"),
                true,
                None
            ),
            (KeychainStatus::Ok, vec![index; 4096])
        );
    }
    assert_eq!(
        target
            .service
            .get_secret(KeychainScope::User, 1000, "personal", true, None),
        (KeychainStatus::Ok, b"retained".to_vec())
    );
    assert_eq!(
        target
            .service
            .get_secret(KeychainScope::User, 1000, "personal", false, None)
            .0,
        KeychainStatus::ErrLocked
    );
    assert_eq!(
        target
            .service
            .generate_key(
                KeychainScope::System,
                0,
                "hardware",
                KeyAlgorithm::Aes256Gcm,
                KeyFlags(4),
                true,
                None
            )
            .0,
        KeychainStatus::ErrUnsupported
    );
}

#[test]
fn incomplete_hardware_metadata_and_wrong_architecture_are_rejected() {
    let mut source = Runtime::empty();
    source.control = Channel(1);
    source.migration = Some(Channel(2));
    source.users = Channel(3);
    source.user_auth = Channel(4);
    let mut target = Runtime::empty();
    for key in source.keys().into_iter().filter(|key| *key != 1) {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    assert!(target.validate().is_err());
    target
        .adopt_record(1, source.encode_record(1).unwrap().as_deref())
        .unwrap();
    target.validate().unwrap();
    let mut header = source.encode_record(0).unwrap().unwrap();
    let architecture = u64::from_le_bytes(header[8..16].try_into().unwrap());
    header[8..16].copy_from_slice(&(3 - architecture).to_le_bytes());
    assert!(target.adopt_record(0, Some(&header)).is_err());
}

#[test]
fn retained_client_handle_and_method_grants_survive_activation() {
    let mut source = Runtime::empty();
    source.control = Channel(1);
    source.migration = Some(Channel(2));
    source.users = Channel(3);
    source.user_auth = Channel(4);
    let mut header = source.encode_record(0).unwrap().unwrap();
    header[64..72].copy_from_slice(&1u64.to_le_bytes());
    let mut client_record = Vec::new();
    for word in [99u64, 2, 1, 2] {
        client_record.extend_from_slice(&word.to_le_bytes());
    }
    header.splice(72..72, client_record);
    let mut target = Runtime::empty();
    target.adopt_record(0, Some(&header)).unwrap();
    for key in source.keys().into_iter().skip(1) {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.validate().unwrap();
    target.activated(8);
    assert!(
        target
            .resources()
            .iter()
            .any(|resource| matches!(resource, Resource::Handle(99)))
    );
    assert_eq!(target.encode_record(0).unwrap().unwrap(), header);
}
