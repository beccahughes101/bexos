use bexos_crypto::{CryptoError, KEY_LEN_256, blake3_256, verify_ed25519};
use ed25519_dalek::{Signer, SigningKey};

#[test]
fn blake3_and_ed25519_helpers_verify_messages() {
    let digest = blake3_256(b"payload");
    assert_eq!(digest.len(), KEY_LEN_256);

    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let verifying_key = signing_key.verifying_key();
    let signature = signing_key.sign(b"payload").to_bytes();

    assert!(verify_ed25519(verifying_key.as_bytes(), b"payload", &signature).is_ok());
    assert_eq!(
        verify_ed25519(verifying_key.as_bytes(), b"other", &signature).unwrap_err(),
        CryptoError::BadSignature
    );
}
