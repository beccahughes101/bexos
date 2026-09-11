use alloc::vec::Vec;

pub use crate::keymint::*;

use crate::protocol::{
    Cursor, MAX_AUTH_TOKEN_LEN, MAX_BLOB_LEN, TrustyResult, TrustyWireError, put_bytes, put_u32,
    put_u64,
};

pub const GATEKEEPER_CMD_ENROLL: u32 = 0 << 1;
pub const GATEKEEPER_CMD_VERIFY: u32 = 1 << 1;
pub const GATEKEEPER_CMD_DELETE_USER: u32 = 2 << 1;
pub const GATEKEEPER_CMD_DELETE_ALL_USERS: u32 = 3 << 1;
const GATEKEEPER_RESP_BIT: u32 = 1;
const GATEKEEPER_ERROR_NONE: u32 = 0;
const GATEKEEPER_ERROR_RETRY: u32 = 2;
const PASSWORD_HANDLE_SECURE_USER_ID_OFFSET: usize = 1;
const HW_AUTH_TOKEN_TIMESTAMP_OFFSET: usize = 29;

pub const AVB_CMD_READ_ROLLBACK_INDEX: u32 = 0 << 1;
pub const AVB_CMD_WRITE_ROLLBACK_INDEX: u32 = 1 << 1;
pub const AVB_CMD_GET_VERSION: u32 = 2 << 1;
pub const AVB_CMD_READ_PERMANENT_ATTRIBUTES: u32 = 3 << 1;
pub const AVB_CMD_WRITE_PERMANENT_ATTRIBUTES: u32 = 4 << 1;
pub const AVB_CMD_READ_LOCK_STATE: u32 = 5 << 1;
pub const AVB_CMD_WRITE_LOCK_STATE: u32 = 6 << 1;
pub const AVB_CMD_LOCK_BOOT_STATE: u32 = 7 << 1;
const AVB_RESP_BIT: u32 = 1;
const AVB_ERROR_NONE: u32 = 0;

pub const AUTHMGR_ROUTE_RPC: u8 = 0;
pub const AUTHMGR_ROUTE_RAW: u8 = 1;
pub const AUTHMGR_TOKEN_LEN: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatekeeperEnrollment {
    pub secure_user_id: u64,
    pub password_handle: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatekeeperVerification {
    pub secure_user_id: u64,
    pub secure_timestamp_ms: u64,
    pub auth_token: Vec<u8>,
}

pub fn encode_gatekeeper_enroll(uid: u64, password: &str) -> TrustyResult<Vec<u8>> {
    validate_uid(uid)?;
    let user_id = gatekeeper_user_id(uid)?;
    if password.is_empty() || password.len() > 256 {
        return Err(TrustyWireError::InvalidArgs);
    }
    let mut out = Vec::new();
    put_u32(&mut out, GATEKEEPER_CMD_ENROLL);
    put_u32(&mut out, GATEKEEPER_ERROR_NONE);
    put_u32(&mut out, user_id);
    put_bytes(&mut out, password.as_bytes())?;
    put_bytes(&mut out, &[])?;
    put_bytes(&mut out, &[])?;
    Ok(out)
}

pub fn encode_gatekeeper_reenroll(
    uid: u64,
    current_password_handle: &[u8],
    current_password: &str,
    new_password: &str,
) -> TrustyResult<Vec<u8>> {
    validate_uid(uid)?;
    let user_id = gatekeeper_user_id(uid)?;
    validate_blob(current_password_handle)?;
    if current_password.is_empty()
        || current_password.len() > 256
        || new_password.is_empty()
        || new_password.len() > 256
    {
        return Err(TrustyWireError::InvalidArgs);
    }
    let mut out = Vec::new();
    put_u32(&mut out, GATEKEEPER_CMD_ENROLL);
    put_u32(&mut out, GATEKEEPER_ERROR_NONE);
    put_u32(&mut out, user_id);
    // Upstream EnrollRequest serializes desired/provided password first,
    // followed by the currently enrolled password and its handle.
    put_bytes(&mut out, new_password.as_bytes())?;
    put_bytes(&mut out, current_password.as_bytes())?;
    put_bytes(&mut out, current_password_handle)?;
    Ok(out)
}

pub fn decode_gatekeeper_enroll(bytes: &[u8]) -> TrustyResult<GatekeeperEnrollment> {
    if let Ok(payload) = gatekeeper_response_payload(bytes, GATEKEEPER_CMD_ENROLL) {
        let mut cursor = Cursor::new(payload);
        let password_handle = cursor.bytes()?.to_vec();
        cursor.finish()?;
        let secure_user_id = password_handle_secure_user_id(&password_handle)?;
        if password_handle.is_empty() || password_handle.len() > MAX_BLOB_LEN {
            return Err(TrustyWireError::InvalidResponse);
        }
        return Ok(GatekeeperEnrollment {
            secure_user_id,
            password_handle,
        });
    }
    let mut cursor = Cursor::new(bytes);
    let secure_user_id = cursor.u64()?;
    let password_handle = cursor.bytes()?.to_vec();
    cursor.finish()?;
    if secure_user_id == 0 || password_handle.is_empty() || password_handle.len() > MAX_BLOB_LEN {
        return Err(TrustyWireError::InvalidResponse);
    }
    Ok(GatekeeperEnrollment {
        secure_user_id,
        password_handle,
    })
}

pub fn encode_gatekeeper_verify(
    uid: u64,
    secure_user_id: u64,
    password_handle: &[u8],
    password: &str,
) -> TrustyResult<Vec<u8>> {
    validate_uid(uid)?;
    validate_uid(secure_user_id)?;
    let user_id = gatekeeper_user_id(uid)?;
    validate_blob(password_handle)?;
    if password.is_empty() || password.len() > 256 {
        return Err(TrustyWireError::InvalidArgs);
    }
    let mut out = Vec::new();
    put_u32(&mut out, GATEKEEPER_CMD_VERIFY);
    put_u32(&mut out, GATEKEEPER_ERROR_NONE);
    put_u32(&mut out, user_id);
    put_u64(&mut out, 0);
    put_bytes(&mut out, password_handle)?;
    put_bytes(&mut out, password.as_bytes())?;
    Ok(out)
}

pub fn decode_gatekeeper_verify(bytes: &[u8]) -> TrustyResult<GatekeeperVerification> {
    if let Ok(payload) = gatekeeper_response_payload(bytes, GATEKEEPER_CMD_VERIFY) {
        let mut cursor = Cursor::new(payload);
        let auth_token = cursor.bytes()?.to_vec();
        // Upstream VerifyResponse serializes its C++ bool as one byte.
        let reenroll = cursor.u8()?;
        cursor.finish()?;
        if reenroll > 1 || auth_token.is_empty() || auth_token.len() > MAX_AUTH_TOKEN_LEN {
            return Err(TrustyWireError::InvalidResponse);
        }
        return Ok(GatekeeperVerification {
            secure_user_id: hw_auth_token_secure_user_id(&auth_token)?,
            secure_timestamp_ms: hw_auth_token_timestamp_ms(&auth_token)?,
            auth_token,
        });
    }
    let mut cursor = Cursor::new(bytes);
    let secure_user_id = cursor.u64()?;
    let secure_timestamp_ms = cursor.u64()?;
    let auth_token = cursor.bytes()?.to_vec();
    cursor.finish()?;
    if secure_user_id == 0
        || secure_timestamp_ms == 0
        || auth_token.is_empty()
        || auth_token.len() > MAX_AUTH_TOKEN_LEN
    {
        return Err(TrustyWireError::InvalidResponse);
    }
    Ok(GatekeeperVerification {
        secure_user_id,
        secure_timestamp_ms,
        auth_token,
    })
}

pub fn encode_gatekeeper_delete(uid: u64, secure_user_id: u64) -> TrustyResult<Vec<u8>> {
    validate_uid(uid)?;
    validate_uid(secure_user_id)?;
    let user_id = gatekeeper_user_id(uid)?;
    let mut out = Vec::new();
    put_u32(&mut out, GATEKEEPER_CMD_DELETE_USER);
    put_u32(&mut out, GATEKEEPER_ERROR_NONE);
    put_u32(&mut out, user_id);
    Ok(out)
}

fn gatekeeper_response_payload(bytes: &[u8], request_cmd: u32) -> TrustyResult<&[u8]> {
    if bytes.len() < 12 {
        return Err(TrustyWireError::InvalidResponse);
    }
    let cmd = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if cmd != (request_cmd | GATEKEEPER_RESP_BIT) {
        return Err(TrustyWireError::InvalidResponse);
    }
    let error = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    match error {
        GATEKEEPER_ERROR_NONE => Ok(&bytes[12..]),
        GATEKEEPER_ERROR_RETRY => Err(TrustyWireError::InvalidArgs),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

fn gatekeeper_user_id(uid: u64) -> TrustyResult<u32> {
    u32::try_from(uid).map_err(|_| TrustyWireError::InvalidArgs)
}

fn password_handle_secure_user_id(password_handle: &[u8]) -> TrustyResult<u64> {
    let end = PASSWORD_HANDLE_SECURE_USER_ID_OFFSET
        .checked_add(8)
        .ok_or(TrustyWireError::InvalidResponse)?;
    let bytes = password_handle
        .get(PASSWORD_HANDLE_SECURE_USER_ID_OFFSET..end)
        .ok_or(TrustyWireError::InvalidResponse)?;
    let sid = u64::from_le_bytes(bytes.try_into().unwrap());
    if sid == 0 {
        Err(TrustyWireError::InvalidResponse)
    } else {
        Ok(sid)
    }
}

fn hw_auth_token_secure_user_id(token: &[u8]) -> TrustyResult<u64> {
    let bytes = token.get(9..17).ok_or(TrustyWireError::InvalidResponse)?;
    let sid = u64::from_le_bytes(bytes.try_into().unwrap());
    if sid == 0 {
        Err(TrustyWireError::InvalidResponse)
    } else {
        Ok(sid)
    }
}

fn hw_auth_token_timestamp_ms(token: &[u8]) -> TrustyResult<u64> {
    let end = HW_AUTH_TOKEN_TIMESTAMP_OFFSET
        .checked_add(8)
        .ok_or(TrustyWireError::InvalidResponse)?;
    let bytes = token
        .get(HW_AUTH_TOKEN_TIMESTAMP_OFFSET..end)
        .ok_or(TrustyWireError::InvalidResponse)?;
    let timestamp = u64::from_be_bytes(bytes.try_into().unwrap());
    if timestamp == 0 {
        Err(TrustyWireError::InvalidResponse)
    } else {
        Ok(timestamp)
    }
}

pub fn encode_avb_rollback_index(command: u32, slot: u32, value: u64) -> TrustyResult<Vec<u8>> {
    if command != AVB_CMD_READ_ROLLBACK_INDEX && command != AVB_CMD_WRITE_ROLLBACK_INDEX {
        return Err(TrustyWireError::InvalidArgs);
    }
    if slot > 31 {
        return Err(TrustyWireError::InvalidArgs);
    }
    let mut out = Vec::new();
    put_u32(&mut out, command);
    put_u32(&mut out, AVB_ERROR_NONE);
    put_u64(&mut out, value);
    put_u32(&mut out, slot);
    Ok(out)
}

pub fn encode_avb_read_lock_state() -> Vec<u8> {
    encode_avb_empty(AVB_CMD_READ_LOCK_STATE)
}

pub fn encode_avb_get_version() -> Vec<u8> {
    encode_avb_empty(AVB_CMD_GET_VERSION)
}

pub fn encode_avb_write_lock_state(locked: bool) -> Vec<u8> {
    let mut out = encode_avb_empty(AVB_CMD_WRITE_LOCK_STATE);
    out.push(u8::from(locked));
    out
}

pub fn encode_avb_lock_boot_state() -> Vec<u8> {
    encode_avb_empty(AVB_CMD_LOCK_BOOT_STATE)
}

pub fn decode_avb_u64(bytes: &[u8], request_command: u32) -> TrustyResult<u64> {
    let payload = avb_response_payload(bytes, request_command)?;
    let mut cursor = Cursor::new(payload);
    let value = cursor.u64()?;
    cursor.finish()?;
    Ok(value)
}

pub fn decode_avb_lock_state(bytes: &[u8]) -> TrustyResult<bool> {
    let payload = avb_response_payload(bytes, AVB_CMD_READ_LOCK_STATE)?;
    match payload {
        [0] => Ok(false),
        [1] => Ok(true),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

pub fn decode_avb_version(bytes: &[u8]) -> TrustyResult<u32> {
    let payload = avb_response_payload(bytes, AVB_CMD_GET_VERSION)?;
    let mut cursor = Cursor::new(payload);
    let version = cursor.u32()?;
    cursor.finish()?;
    Ok(version)
}

pub fn decode_avb_empty(bytes: &[u8], request_command: u32) -> TrustyResult<()> {
    if avb_response_payload(bytes, request_command)?.is_empty() {
        Ok(())
    } else {
        Err(TrustyWireError::InvalidResponse)
    }
}

fn encode_avb_empty(command: u32) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, command);
    put_u32(&mut out, AVB_ERROR_NONE);
    out
}

fn avb_response_payload(bytes: &[u8], request_command: u32) -> TrustyResult<&[u8]> {
    if bytes.len() < 8 {
        return Err(TrustyWireError::InvalidResponse);
    }
    let command = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let result = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if command != (request_command | AVB_RESP_BIT) || result != AVB_ERROR_NONE {
        return Err(TrustyWireError::InvalidResponse);
    }
    Ok(&bytes[8..])
}

/// Select the AuthMgr BE Binder-RPC authorization endpoint on a fresh TIPC
/// connection. The subsequent frames on that connection are Binder RPC, not a
/// private BexOS authorization protocol.
pub fn encode_authmgr_rpc_route() -> [u8; 1] {
    [AUTHMGR_ROUTE_RPC]
}

/// Select the AuthMgr BE raw service connection authorized by a token issued
/// through its FE/BE DICE authorization exchange.
pub fn encode_authmgr_raw_route(token: &[u8]) -> TrustyResult<Vec<u8>> {
    let token: &[u8; AUTHMGR_TOKEN_LEN] =
        token.try_into().map_err(|_| TrustyWireError::InvalidArgs)?;
    let mut out = Vec::with_capacity(1 + AUTHMGR_TOKEN_LEN);
    out.push(AUTHMGR_ROUTE_RAW);
    out.extend_from_slice(token);
    Ok(out)
}

pub const ORCHESTRATOR_CMD_MARK_COMPONENT_FAILED: u32 = 0x200;
pub const ORCHESTRATOR_CMD_GET_ACTIVE_COMPONENT: u32 = 0x201;
pub const ORCHESTRATOR_CMD_GET_KERNEL_SLOT: u32 = 0x202;

pub fn encode_orchestrator_request(command: u32, component: u32) -> TrustyResult<[u8; 16]> {
    if !matches!(
        command,
        ORCHESTRATOR_CMD_MARK_COMPONENT_FAILED
            | ORCHESTRATOR_CMD_GET_ACTIVE_COMPONENT
            | ORCHESTRATOR_CMD_GET_KERNEL_SLOT
    ) {
        return Err(TrustyWireError::InvalidArgs);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&1u32.to_le_bytes());
    out[4..8].copy_from_slice(&command.to_le_bytes());
    out[8..12].copy_from_slice(&component.to_le_bytes());
    Ok(out)
}

pub fn decode_orchestrator_response(bytes: &[u8]) -> TrustyResult<u32> {
    if bytes.len() != 16
        || u32::from_le_bytes(bytes[..4].try_into().unwrap()) != 1
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != 0
        || bytes[12..16] != [0; 4]
    {
        return Err(TrustyWireError::InvalidResponse);
    }
    let slot = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if matches!(slot, 1 | 2) {
        Ok(slot)
    } else {
        Err(TrustyWireError::InvalidResponse)
    }
}

fn validate_uid(uid: u64) -> TrustyResult<()> {
    if uid == 0 {
        Err(TrustyWireError::InvalidArgs)
    } else {
        Ok(())
    }
}

fn validate_blob(blob: &[u8]) -> TrustyResult<()> {
    if blob.is_empty() || blob.len() > MAX_BLOB_LEN {
        Err(TrustyWireError::InvalidArgs)
    } else {
        Ok(())
    }
}
