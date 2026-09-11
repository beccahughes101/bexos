use bexos_migration::{Error, codec::Encoder};
use bexos_trusty_client::users::HardwareAuthToken;
use bexos_user_store::MemoryUserStore;
use bexos_usersd::token_cache::{AuthStateChange, invalidate_for_change, retain_usable};
use bexos_usersd::{auth::RuntimeUserAuthProvider, migration::Runtime, service::UsersdService};
use bexos_userspace::{Channel, live_migration::State};

#[test]
fn secure_auth_adopts_the_already_bound_startup_endpoint() {
    let auth = bexos_usersd::auth::TeeUserAuthProvider::connect(Channel(321)).unwrap();
    assert_eq!(auth.client().0, 321);
    assert_eq!(auth.gatekeeper_session(), None);
    assert_eq!(auth.keymint_session(), None);
    assert!(bexos_usersd::auth::TeeUserAuthProvider::connect(Channel(0)).is_err());
}

#[test]
fn usersd_service_links_user_store() {
    let store = MemoryUserStore::new();
    assert!(store.list_users().is_empty());
}

#[test]
fn user_store_waits_for_persistent_storage_without_accepting_volatile_users() {
    let service = UsersdService::awaiting_storage();
    assert!(service.storage_pending());
    assert!(matches!(
        service.list_users(),
        Err(bexos_user_store::UserStoreError::Storage)
    ));
    assert!(matches!(
        service.get_user(2001),
        Err(bexos_user_store::UserStoreError::Storage)
    ));
}

#[test]
fn heart_transplant_preserves_token_timestamp_and_expiry() {
    let mut source = Runtime::new(
        Channel(1),
        Some(Channel(2)),
        Channel(3),
        UsersdService::new(),
        RuntimeUserAuthProvider::Unsupported,
        7,
    );
    source.auth_tokens.push(HardwareAuthToken {
        uid: 42,
        secure_user_id: 99,
        secure_timestamp_ms: 1_000,
        expires_at_ms: 301_000,
        encoded: b"gatekeeper-token".to_vec(),
    });

    let record = source.encode_record(4).unwrap().unwrap();
    let mut target = Runtime::empty();
    target.adopt_record(4, Some(&record)).unwrap();

    assert_eq!(target.auth_tokens, source.auth_tokens);
    assert_eq!(target.auth_tokens[0].expires_at_ms, 301_000);
    assert!(!target.auth_tokens[0].is_valid_at(301_000));
}

#[test]
fn heart_transplant_filters_expired_tokens_without_mutating_source() {
    let mut source = Runtime::new(
        Channel(1),
        Some(Channel(2)),
        Channel(3),
        UsersdService::new(),
        RuntimeUserAuthProvider::Unsupported,
        7,
    );
    source.auth_tokens.push(HardwareAuthToken::new(
        42,
        99,
        1_000,
        b"gatekeeper-token".to_vec(),
    ));

    assert_eq!(source.migratable_auth_tokens_at(300_999).len(), 1);
    assert!(source.migratable_auth_tokens_at(301_000).is_empty());
    assert_eq!(
        source.auth_tokens.len(),
        1,
        "an aborted handover retains the source token"
    );
    assert_eq!(source.auth_tokens[0].expires_at_ms, 301_000);
}

#[test]
fn heart_transplant_rejects_token_lifetime_extension() {
    let mut writer = Encoder::new();
    writer.word(1);
    writer.word(1);
    writer.word(42);
    writer.word(99);
    writer.word(1_000);
    writer.word(301_001);
    writer.word(4);
    for byte in b"auth" {
        writer.word(u64::from(*byte));
    }

    let mut target = Runtime::empty();
    assert_eq!(
        target.adopt_record(4, Some(&writer.finish())),
        Err(Error::InvalidData)
    );
    assert!(target.auth_tokens.is_empty());
}

#[test]
fn heart_transplant_rejects_malformed_token_without_destroying_staged_state() {
    let mut target = Runtime::empty();
    target.auth_tokens.push(HardwareAuthToken::new(
        7,
        11,
        1_000,
        b"previously-staged-token".to_vec(),
    ));

    let mut writer = Encoder::new();
    writer.word(1);
    writer.word(1);
    writer.word(42);
    writer.word(99);
    writer.word(1_000);
    writer.word(301_000);
    writer.word(2);
    writer.word(u64::from(b'o'));
    writer.word(256);

    assert_eq!(
        target.adopt_record(4, Some(&writer.finish())),
        Err(Error::InvalidData)
    );
    assert_eq!(target.auth_tokens.len(), 1);
    assert_eq!(target.auth_tokens[0].uid, 7);
}

#[test]
fn heart_transplant_excludes_malformed_source_tokens() {
    let mut source = Runtime::new(
        Channel(1),
        Some(Channel(2)),
        Channel(3),
        UsersdService::new(),
        RuntimeUserAuthProvider::Unsupported,
        7,
    );
    let mut extended = HardwareAuthToken::new(42, 99, 1_000, b"extended".to_vec());
    extended.expires_at_ms += 1;
    source.auth_tokens.push(extended);
    source.auth_tokens.push(HardwareAuthToken {
        uid: 43,
        secure_user_id: 100,
        secure_timestamp_ms: 1_000,
        expires_at_ms: 301_000,
        encoded: Vec::new(),
    });

    assert!(source.migratable_auth_tokens_at(2_000).is_empty());
    assert_eq!(source.auth_tokens.len(), 2);

    let record = source.encode_record(4).unwrap().unwrap();
    let mut target = Runtime::empty();
    target.adopt_record(4, Some(&record)).unwrap();
    assert!(target.auth_tokens.is_empty());
    assert_eq!(source.auth_tokens.len(), 2);
}

#[test]
fn migrated_tokens_are_invalidated_by_security_state_changes() {
    for change in [
        AuthStateChange::Lock,
        AuthStateChange::Disable,
        AuthStateChange::Delete,
        AuthStateChange::PasswordReplacement,
    ] {
        let mut tokens = vec![HardwareAuthToken::new(
            42,
            99,
            1_000,
            b"migrated-token".to_vec(),
        )];
        let mut unlocked = vec![42];
        invalidate_for_change(&mut tokens, &mut unlocked, 42, change);
        assert!(tokens.is_empty(), "{change:?} retained a migrated token");
        if change == AuthStateChange::PasswordReplacement {
            assert_eq!(unlocked, vec![42]);
        } else {
            assert!(unlocked.is_empty(), "{change:?} retained unlocked state");
        }
    }
}

#[test]
fn migrated_token_expiry_is_enforced_at_original_deadline() {
    let mut tokens = vec![HardwareAuthToken::new(
        42,
        99,
        1_000,
        b"migrated-token".to_vec(),
    )];
    retain_usable(&mut tokens, &[42], 300_999);
    assert_eq!(tokens.len(), 1);
    retain_usable(&mut tokens, &[42], 301_000);
    assert!(tokens.is_empty());
}
