use alloc::vec;
use alloc::vec::Vec;
use bexos_userspace::{Channel, Rpc};
use keychain_fidl::{FidlEncode, HandleRef, KeychainScope};
use user_manager_fidl as users;
use user_manager_fidl::{FidlDecode as UserFidlDecode, FidlEncode as UserFidlEncode};

pub(crate) fn user_unlocked(users: Channel, scope: KeychainScope, uid: u64) -> bool {
    if scope == KeychainScope::System {
        return true;
    }
    let request = users::UserManagerGetUserRequest { uid };
    let mut request_bytes = [0; 128];
    let mut request_handles = [users::HandleRef { raw: 0 }; 1];
    let Ok(encoded) = request.encode(&mut request_bytes, &mut request_handles) else {
        return false;
    };
    let message = match Rpc(users).call_raw(2, &request_bytes[..encoded.bytes], &[], true) {
        Ok(message) => message,
        Err(_) => return false,
    };
    let handles: Vec<_> = message
        .handles
        .iter()
        .map(|handle| users::HandleRef { raw: *handle })
        .collect();
    let Ok(response) = users::UserManagerGetUserResponse::decode(&message.bytes, &handles) else {
        return false;
    };
    response.status == users::UserStatus::Ok && response.user.unlocked && !response.user.disabled
}

pub(crate) fn user_auth_token(users: Channel, scope: KeychainScope, uid: u64) -> Option<Vec<u8>> {
    if scope == KeychainScope::System {
        return None;
    }
    let request = users::UserManagerGetHardwareAuthTokenRequest { uid };
    let mut request_bytes = [0; 128];
    let mut request_handles = [users::HandleRef { raw: 0 }; 1];
    let encoded = request
        .encode(&mut request_bytes, &mut request_handles)
        .ok()?;
    let message = Rpc(users)
        .call_raw(9, &request_bytes[..encoded.bytes], &[], true)
        .ok()?;
    let handles: Vec<_> = message
        .handles
        .iter()
        .map(|handle| users::HandleRef { raw: *handle })
        .collect();
    let response =
        users::UserManagerGetHardwareAuthTokenResponse::decode(&message.bytes, &handles).ok()?;
    if response.status == users::UserStatus::Ok {
        Some(response.encoded_token.to_vec())
    } else {
        None
    }
}

pub(crate) fn refs(handles: &[u64]) -> Vec<HandleRef> {
    handles
        .iter()
        .map(|handle| HandleRef { raw: *handle })
        .collect()
}

pub(crate) fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

pub(crate) fn reply<Q: FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = vec![0; 65500];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let encoded = q
        .encode(&mut bytes, &mut handles)
        .expect("keychaind encode");
    channel
        .send(
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
        )
        .expect("keychaind reply");
}
