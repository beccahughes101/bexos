use alloc::vec::Vec;
use bexos_crypto::KEY_LEN_256;

use crate::protocol::HW_AUTH_TOKEN_TIMEOUT_SECS;

pub const UKEK_DERIVATION_LABEL: &[u8] = b"bexos.ukek.v1\0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareAuthToken {
    pub uid: u64,
    pub secure_user_id: u64,
    pub secure_timestamp_ms: u64,
    pub expires_at_ms: u64,
    pub encoded: Vec<u8>,
}

impl HardwareAuthToken {
    pub fn new(uid: u64, secure_user_id: u64, secure_timestamp_ms: u64, encoded: Vec<u8>) -> Self {
        Self {
            uid,
            secure_user_id,
            secure_timestamp_ms,
            expires_at_ms: secure_timestamp_ms
                .saturating_add(HW_AUTH_TOKEN_TIMEOUT_SECS.saturating_mul(1000)),
            encoded,
        }
    }

    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms >= self.secure_timestamp_ms
            && now_ms < self.expires_at_ms
            && !self.encoded.is_empty()
    }

    /// Validate the immutable identity and lifetime fields before a token is
    /// accepted from migrated state. The deadline must be the one established
    /// by Gatekeeper; migration must never manufacture or extend it.
    pub fn is_well_formed(&self) -> bool {
        self.uid != 0
            && self.secure_user_id != 0
            && self.secure_timestamp_ms != 0
            && self.expires_at_ms
                == self
                    .secure_timestamp_ms
                    .saturating_add(HW_AUTH_TOKEN_TIMEOUT_SECS.saturating_mul(1000))
            && !self.encoded.is_empty()
    }
}

pub fn ukek_hmac_message(uid: u64) -> Vec<u8> {
    let mut message = Vec::with_capacity(UKEK_DERIVATION_LABEL.len() + core::mem::size_of::<u64>());
    message.extend_from_slice(UKEK_DERIVATION_LABEL);
    message.extend_from_slice(&uid.to_le_bytes());
    message
}

pub fn zeroize_ukek(ukek: &mut [u8; KEY_LEN_256]) {
    ukek.fill(0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
