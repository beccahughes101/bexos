use alloc::vec::Vec;
use bexos_trusty_client::users::HardwareAuthToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthStateChange {
    Lock,
    Disable,
    Delete,
    PasswordReplacement,
}

pub fn invalidate_for_change(
    tokens: &mut Vec<HardwareAuthToken>,
    unlocked: &mut Vec<u64>,
    uid: u64,
    change: AuthStateChange,
) {
    tokens.retain(|candidate| candidate.uid != uid);
    if change != AuthStateChange::PasswordReplacement {
        unlocked.retain(|candidate| *candidate != uid);
    }
}

pub fn retain_usable(tokens: &mut Vec<HardwareAuthToken>, unlocked: &[u64], now_ms: u64) {
    tokens.retain(|token| {
        token.is_well_formed()
            && token.is_valid_at(now_ms)
            && unlocked.iter().any(|uid| *uid == token.uid)
    });
}
