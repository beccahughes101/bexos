#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(all(target_arch = "aarch64", any(target_os = "linux", target_os = "none")))]
core::arch::global_asm!(
    r#"
    .section .note.gnu.property, "a", %note
    .balign 8
    .long 4
    .long 16
    .long 5
    .asciz "GNU"
    .balign 8
    .long 0xc0000000
    .long 4
    .long 3
    .long 0
    .balign 8
    .previous
"#
);

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

pub const KEY_LEN_256: usize = 32;
pub const AES_GCM_NONCE_LEN: usize = 12;
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;
pub const ED25519_SIGNATURE_LEN: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoError {
    InvalidWrappedKey,
    AccessDenied,
    Storage,
    InvalidKey,
    BadSignature,
}

pub fn blake3_256(bytes: &[u8]) -> [u8; KEY_LEN_256] {
    blake3::hash(bytes).into()
}

pub fn verify_ed25519(
    public_key: &[u8; ED25519_PUBLIC_KEY_LEN],
    message: &[u8],
    signature: &[u8; ED25519_SIGNATURE_LEN],
) -> Result<(), CryptoError> {
    let verifying_key =
        VerifyingKey::from_bytes(public_key).map_err(|_| CryptoError::InvalidKey)?;
    let signature = Signature::from_bytes(signature);
    verifying_key
        .verify(message, &signature)
        .map_err(|_| CryptoError::BadSignature)
}
