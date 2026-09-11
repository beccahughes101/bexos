mod migration_connection_tests;

use bexos_keychain_store::KeychainStoreError;
use bexos_keychaind::service::{HardwareKeyProvider, KeychainService};
use bexos_trusty_client::services::GeneratedKey;
use keychain_fidl::{KeyAlgorithm, KeyFlags, KeychainScope, KeychainStatus};

#[test]
fn system_and_user_keychains_have_separate_alias_namespaces() {
    let mut service = KeychainService::new();

    assert_eq!(
        service.store_secret(
            KeychainScope::System,
            0,
            "token",
            b"system",
            KeyFlags(0),
            true,
            None,
        ),
        KeychainStatus::Ok
    );
    assert_eq!(
        service.store_secret(
            KeychainScope::User,
            1000,
            "token",
            b"user",
            KeyFlags(0),
            true,
            None,
        ),
        KeychainStatus::Ok
    );

    assert_eq!(
        service.get_secret(KeychainScope::System, 0, "token", true, None),
        (KeychainStatus::Ok, b"system".to_vec())
    );
    assert_eq!(
        service.get_secret(KeychainScope::User, 1000, "token", true, None),
        (KeychainStatus::Ok, b"user".to_vec())
    );
}

#[test]
fn user_keychain_requires_an_unlocked_user_context() {
    let mut service = KeychainService::new();

    assert_eq!(
        service.store_secret(
            KeychainScope::User,
            1000,
            "token",
            b"user",
            KeyFlags(0),
            false,
            None,
        ),
        KeychainStatus::ErrLocked
    );
    assert_eq!(
        service.store_secret(
            KeychainScope::User,
            0,
            "token",
            b"user",
            KeyFlags(0),
            true,
            None,
        ),
        KeychainStatus::ErrInvalidArgs
    );
}

#[test]
fn hardware_backed_secret_is_envelope_encrypted_and_round_trips() {
    let mut service = KeychainService::with_hardware(FakeHardwareKeyProvider);
    assert_eq!(
        service.store_secret(
            KeychainScope::System,
            0,
            "sealed",
            b"top secret",
            KeyFlags(0x0004),
            true,
            None,
        ),
        KeychainStatus::Ok
    );
    assert_eq!(
        service.get_secret(KeychainScope::System, 0, "sealed", true, None),
        (KeychainStatus::Ok, b"top secret".to_vec())
    );
    assert_eq!(
        service.delete_secret(KeychainScope::System, 0, "sealed", true),
        KeychainStatus::Ok
    );
}

#[test]
fn key_generation_and_signing_are_not_faked_without_tee_or_entropy() {
    let mut service = KeychainService::new();

    assert_eq!(
        service.generate_key(
            KeychainScope::System,
            0,
            "signing",
            KeyAlgorithm::Ed25519,
            KeyFlags(0),
            true,
            None,
        ),
        (KeychainStatus::ErrUnsupported, Vec::new())
    );
    assert_eq!(
        service.sign(KeychainScope::System, 0, "signing", &[1; 32], true, None),
        (KeychainStatus::ErrUnsupported, Vec::new())
    );
}

#[test]
fn hardware_backed_ed25519_routes_through_keymint_provider() {
    let mut service = KeychainService::with_hardware(FakeHardwareKeyProvider);

    assert_eq!(
        service.generate_key(
            KeychainScope::System,
            0,
            "signing",
            KeyAlgorithm::Ed25519,
            KeyFlags(0x0004),
            true,
            None,
        ),
        (KeychainStatus::Ok, vec![0xa5; 32])
    );
    assert_eq!(
        service.sign(KeychainScope::System, 0, "signing", &[7; 32], true, None),
        (KeychainStatus::Ok, vec![0x5a; 64])
    );
}

#[test]
fn require_user_auth_needs_a_current_usersd_token() {
    let mut service = KeychainService::with_hardware(FakeHardwareKeyProvider);

    assert_eq!(
        service.generate_key(
            KeychainScope::User,
            1000,
            "authsign",
            KeyAlgorithm::Ed25519,
            KeyFlags(0x0005),
            true,
            None,
        ),
        (KeychainStatus::ErrLocked, Vec::new())
    );
    assert_eq!(
        service.generate_key(
            KeychainScope::User,
            1000,
            "authsign",
            KeyAlgorithm::Ed25519,
            KeyFlags(0x0005),
            true,
            Some(b"hat"),
        ),
        (KeychainStatus::Ok, vec![0xa5; 32])
    );
    assert_eq!(
        service.sign(KeychainScope::User, 1000, "authsign", &[7; 32], true, None),
        (KeychainStatus::ErrLocked, Vec::new())
    );
    assert_eq!(
        service.sign(
            KeychainScope::User,
            1000,
            "authsign",
            &[7; 32],
            true,
            Some(b"hat"),
        ),
        (KeychainStatus::Ok, vec![0x5a; 64])
    );
}

#[test]
fn hardware_backed_p256_routes_through_keymint_provider() {
    let mut service = KeychainService::with_hardware(FakeHardwareKeyProvider);

    assert_eq!(
        service.generate_key(
            KeychainScope::System,
            0,
            "p256",
            KeyAlgorithm::EcdsaP256,
            KeyFlags(0x0004),
            true,
            None,
        ),
        (KeychainStatus::Ok, vec![0x04; 65])
    );
    assert_eq!(
        service.sign(KeychainScope::System, 0, "p256", &[3; 32], true, None),
        (KeychainStatus::Ok, vec![0x30; 70])
    );
}

#[test]
fn hardware_backed_aes_gcm_uses_opaque_blob_and_round_trips() {
    let mut service = KeychainService::with_hardware(FakeHardwareKeyProvider);

    assert_eq!(
        service.generate_key(
            KeychainScope::System,
            0,
            "sealed",
            KeyAlgorithm::Aes256Gcm,
            KeyFlags(0x0004),
            true,
            None,
        ),
        (KeychainStatus::Ok, Vec::new())
    );

    let (status, nonce, ciphertext) = service.aead_encrypt(
        KeychainScope::System,
        0,
        "sealed",
        b"plaintext",
        b"aad",
        true,
        None,
    );
    assert_eq!(status, KeychainStatus::Ok);
    assert_eq!(nonce, [9; 12]);
    assert_eq!(ciphertext, b"aadplaintext0123456789abcdef".to_vec());

    assert_eq!(
        service.aead_decrypt(
            KeychainScope::System,
            0,
            "sealed",
            &nonce,
            &ciphertext,
            b"aad",
            true,
            None,
        ),
        (KeychainStatus::Ok, b"plaintext".to_vec())
    );
}

#[test]
fn guest_entry_is_tokio_future() {
    let _future = bexos_keychaind::main(0);
}

struct FakeHardwareKeyProvider;

impl HardwareKeyProvider for FakeHardwareKeyProvider {
    fn generate_key(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        alias: &str,
        algorithm: KeyAlgorithm,
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<GeneratedKey, KeychainStoreError> {
        if alias == "authsign" && hardware_auth_token != Some(b"hat".as_slice()) {
            return Err(KeychainStoreError::AccessDenied);
        }
        match (alias, algorithm) {
            ("signing" | "authsign", KeyAlgorithm::Ed25519) => Ok(GeneratedKey {
                public_material: vec![0xa5; 32],
                characteristics: vec![0x81, 0x80],
                opaque_blob: b"ed25519-blob".to_vec(),
            }),
            ("p256", KeyAlgorithm::EcdsaP256) => Ok(GeneratedKey {
                public_material: vec![0x04; 65],
                characteristics: vec![0x81, 0x80],
                opaque_blob: b"p256-blob".to_vec(),
            }),
            ("sealed", KeyAlgorithm::Aes256Gcm) => Ok(GeneratedKey {
                public_material: Vec::new(),
                characteristics: vec![0x81, 0x80],
                opaque_blob: b"aes-blob".to_vec(),
            }),
            _ => Err(KeychainStoreError::NotFound),
        }
    }

    fn sign(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        alias: &str,
        algorithm: KeyAlgorithm,
        opaque_key_blob: &[u8],
        digest: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        if alias == "authsign" && hardware_auth_token != Some(b"hat".as_slice()) {
            return Err(KeychainStoreError::AccessDenied);
        }
        if (alias == "signing" || alias == "authsign")
            && algorithm == KeyAlgorithm::Ed25519
            && opaque_key_blob == b"ed25519-blob"
            && digest == [7; 32]
        {
            Ok(vec![0x5a; 64])
        } else if alias == "p256"
            && algorithm == KeyAlgorithm::EcdsaP256
            && opaque_key_blob == b"p256-blob"
            && digest == [3; 32]
        {
            Ok(vec![0x30; 70])
        } else {
            Err(KeychainStoreError::AccessDenied)
        }
    }

    fn aead_encrypt(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        alias: &str,
        opaque_key_blob: &[u8],
        plaintext: &[u8],
        aad: &[u8],
        _hardware_auth_token: Option<&[u8]>,
    ) -> Result<([u8; 12], Vec<u8>), KeychainStoreError> {
        if alias != "sealed" || opaque_key_blob != b"aes-blob" {
            return Err(KeychainStoreError::NotFound);
        }
        let mut ciphertext = Vec::from(aad);
        ciphertext.extend_from_slice(plaintext);
        ciphertext.extend_from_slice(b"0123456789abcdef");
        Ok(([9; 12], ciphertext))
    }

    fn aead_decrypt(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        alias: &str,
        opaque_key_blob: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
        _hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        if alias != "sealed"
            || opaque_key_blob != b"aes-blob"
            || nonce != [9; 12]
            || !ciphertext.starts_with(aad)
        {
            return Err(KeychainStoreError::AccessDenied);
        }
        let body = &ciphertext[aad.len()..ciphertext.len().saturating_sub(16)];
        Ok(body.to_vec())
    }

    fn delete_key(&mut self, opaque_key_blob: &[u8]) -> Result<(), KeychainStoreError> {
        if opaque_key_blob == b"aes-blob"
            || opaque_key_blob == b"ed25519-blob"
            || opaque_key_blob == b"p256-blob"
        {
            Ok(())
        } else {
            Err(KeychainStoreError::NotFound)
        }
    }
}
