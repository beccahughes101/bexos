use alloc::vec::Vec;

use bexos_crypto::{AES_GCM_NONCE_LEN, ED25519_SIGNATURE_LEN, KEY_LEN_256};
use kmr_wire::keymint::{
    Algorithm, BlockMode, Digest, EcCurve, HardwareAuthToken, HardwareAuthenticatorType, KeyParam,
    KeyPurpose,
};
use kmr_wire::secureclock::Timestamp;
use kmr_wire::{
    AsCborValue, BeginRequest, DeleteKeyRequest, FinishRequest, GenerateKeyRequest, PerformOpReq,
    PerformOpResponse, PerformOpRsp, SetBootInfoRequest, SetHalInfoRequest, UpdateAadRequest,
};

use crate::protocol::{
    MAX_AUTH_TOKEN_LEN, MAX_BLOB_LEN, MAX_PAYLOAD_LEN, TrustyResult, TrustyWireError,
    validate_alias,
};

pub const KEYMINT_CMD_GENERATE_KEY: u32 = 0x13;
pub const KEYMINT_CMD_DELETE_KEY: u32 = 0x17;
pub const KEYMINT_CMD_BEGIN: u32 = 0x1a;
pub const KEYMINT_CMD_UPDATE_AAD: u32 = 0x31;
pub const KEYMINT_CMD_FINISH: u32 = 0x33;
pub const KEYMINT_CMD_SET_HAL_INFO: u32 = 0x81;
pub const KEYMINT_CMD_SET_BOOT_INFO: u32 = 0x82;

// These aliases keep command telemetry meaningful for callers whose public API
// describes a complete operation rather than KeyMint's begin/finish phases.
pub const KEYMINT_CMD_SIGN: u32 = KEYMINT_CMD_BEGIN;
pub const KEYMINT_CMD_AES_GCM_ENCRYPT: u32 = KEYMINT_CMD_BEGIN;
pub const KEYMINT_CMD_AES_GCM_DECRYPT: u32 = KEYMINT_CMD_BEGIN;
pub const KEYMINT_CMD_HMAC_SHA256: u32 = KEYMINT_CMD_BEGIN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyMintAlgorithm {
    Ed25519,
    P256,
    AesGcm,
    HmacSha256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyMintPurpose {
    Sign,
    Encrypt,
    Decrypt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedKey {
    pub public_material: Vec<u8>,
    pub characteristics: Vec<u8>,
    pub opaque_blob: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BegunOperation {
    pub handle: i64,
    pub nonce: Option<[u8; AES_GCM_NONCE_LEN]>,
}

pub fn encode_keymint_set_boot_info(
    verified_boot_key: &[u8],
    device_boot_locked: bool,
    verified_boot_state: i32,
    verified_boot_hash: &[u8],
    boot_patchlevel: u32,
) -> TrustyResult<Vec<u8>> {
    if verified_boot_key.is_empty()
        || verified_boot_key.len() > MAX_BLOB_LEN
        || verified_boot_hash.len() != 32
        || !(0..=3).contains(&verified_boot_state)
    {
        return Err(TrustyWireError::InvalidArgs);
    }
    PerformOpReq::SetBootInfo(SetBootInfoRequest {
        verified_boot_key: verified_boot_key.to_vec(),
        device_boot_locked,
        verified_boot_state,
        verified_boot_hash: verified_boot_hash.to_vec(),
        boot_patchlevel,
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_keymint_set_boot_info(bytes: &[u8]) -> TrustyResult<()> {
    match decode_response(bytes)? {
        PerformOpRsp::SetBootInfo(_) => Ok(()),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

pub fn encode_keymint_set_hal_info(
    os_version: u32,
    os_patchlevel: u32,
    vendor_patchlevel: u32,
) -> TrustyResult<Vec<u8>> {
    PerformOpReq::SetHalInfo(SetHalInfoRequest {
        os_version,
        os_patchlevel,
        vendor_patchlevel,
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_keymint_set_hal_info(bytes: &[u8]) -> TrustyResult<()> {
    match decode_response(bytes)? {
        PerformOpRsp::SetHalInfo(_) => Ok(()),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

pub fn encode_keymint_generate_key(
    uid: u64,
    alias: &str,
    algorithm: KeyMintAlgorithm,
    auth_bound: bool,
    auth_timeout_secs: u64,
    hardware_auth_token: Option<&[u8]>,
) -> TrustyResult<Vec<u8>> {
    let secure_user_id = if auth_bound {
        let token =
            decode_hardware_auth_token(hardware_auth_token.ok_or(TrustyWireError::InvalidArgs)?)?;
        let secure_user_id =
            u64::try_from(token.user_id).map_err(|_| TrustyWireError::InvalidArgs)?;
        Some(secure_user_id)
    } else {
        None
    };
    encode_keymint_generate_key_with_sid(uid, alias, algorithm, secure_user_id, auth_timeout_secs)
}

/// Create a key bound to Gatekeeper's secure user ID. KeyMint key creation
/// binds the authorization policy but does not itself consume a hardware-auth
/// token; a current token is required when the key is used.
pub fn encode_keymint_generate_auth_bound_key(
    uid: u64,
    alias: &str,
    algorithm: KeyMintAlgorithm,
    secure_user_id: u64,
    auth_timeout_secs: u64,
) -> TrustyResult<Vec<u8>> {
    encode_keymint_generate_key_with_sid(
        uid,
        alias,
        algorithm,
        Some(secure_user_id),
        auth_timeout_secs,
    )
}

fn encode_keymint_generate_key_with_sid(
    uid: u64,
    alias: &str,
    algorithm: KeyMintAlgorithm,
    secure_user_id: Option<u64>,
    auth_timeout_secs: u64,
) -> TrustyResult<Vec<u8>> {
    validate_uid(uid)?;
    validate_alias(alias)?;
    let mut key_params = generation_params(algorithm);
    // Individual deletion must invalidate retained opaque blobs, which requires
    // a per-key secret in authenticated secure storage.
    key_params.push(KeyParam::RollbackResistance);
    if matches!(
        algorithm,
        KeyMintAlgorithm::Ed25519 | KeyMintAlgorithm::P256
    ) {
        // KeyMint requires explicit validity bounds for asymmetric certificates.
        key_params.push(KeyParam::CertificateNotBefore(
            kmr_wire::keymint::DateTime { ms_since_epoch: 0 },
        ));
        key_params.push(KeyParam::CertificateNotAfter(kmr_wire::keymint::DateTime {
            ms_since_epoch: 253_402_300_799_000,
        }));
    }
    if let Some(secure_user_id) = secure_user_id {
        validate_uid(secure_user_id)?;
        key_params.push(KeyParam::UserSecureId(secure_user_id));
        key_params.push(KeyParam::UserAuthType(
            HardwareAuthenticatorType::Password as u32,
        ));
        key_params.push(KeyParam::AuthTimeout(
            u32::try_from(auth_timeout_secs).map_err(|_| TrustyWireError::InvalidArgs)?,
        ));
    } else {
        key_params.push(KeyParam::NoAuthRequired);
    }
    PerformOpReq::DeviceGenerateKey(GenerateKeyRequest {
        key_params,
        attestation_key: None,
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_keymint_generated_key(bytes: &[u8]) -> TrustyResult<GeneratedKey> {
    let rsp = decode_response(bytes)?;
    let PerformOpRsp::DeviceGenerateKey(response) = rsp else {
        return Err(TrustyWireError::InvalidResponse);
    };
    if response.ret.key_blob.is_empty() || response.ret.key_blob.len() > MAX_BLOB_LEN {
        return Err(TrustyWireError::InvalidResponse);
    }
    let public_material = response
        .ret
        .certificate_chain
        .first()
        .map(|certificate| certificate.encoded_certificate.clone())
        .unwrap_or_default();
    let characteristics = response
        .ret
        .key_characteristics
        .into_vec()
        .map_err(|_| TrustyWireError::InvalidResponse)?;
    Ok(GeneratedKey {
        public_material,
        characteristics,
        opaque_blob: response.ret.key_blob,
    })
}

pub fn encode_keymint_begin(
    algorithm: KeyMintAlgorithm,
    purpose: KeyMintPurpose,
    opaque_blob: &[u8],
    nonce: Option<&[u8]>,
    hardware_auth_token: Option<&[u8]>,
) -> TrustyResult<Vec<u8>> {
    validate_blob(opaque_blob)?;
    let mut params = operation_params(algorithm, purpose)?;
    if let Some(nonce) = nonce {
        if algorithm != KeyMintAlgorithm::AesGcm || nonce.len() != AES_GCM_NONCE_LEN {
            return Err(TrustyWireError::InvalidArgs);
        }
        params.push(KeyParam::Nonce(nonce.to_vec()));
    }
    let auth_token = hardware_auth_token
        .map(decode_hardware_auth_token)
        .transpose()?;
    PerformOpReq::DeviceBegin(BeginRequest {
        purpose: keymint_purpose(purpose),
        key_blob: opaque_blob.to_vec(),
        params,
        auth_token,
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_keymint_begin(bytes: &[u8]) -> TrustyResult<BegunOperation> {
    let rsp = decode_response(bytes)?;
    let PerformOpRsp::DeviceBegin(response) = rsp else {
        return Err(TrustyWireError::InvalidResponse);
    };
    if response.ret.op_handle == 0 {
        return Err(TrustyWireError::InvalidResponse);
    }
    let nonce = response
        .ret
        .params
        .iter()
        .find_map(|param| match param {
            KeyParam::Nonce(bytes) => Some(bytes.as_slice()),
            _ => None,
        })
        .map(|bytes| {
            bytes
                .try_into()
                .map_err(|_| TrustyWireError::InvalidResponse)
        })
        .transpose()?;
    Ok(BegunOperation {
        handle: response.ret.op_handle,
        nonce,
    })
}

pub fn encode_keymint_update_aad(
    handle: i64,
    aad: &[u8],
    hardware_auth_token: Option<&[u8]>,
) -> TrustyResult<Vec<u8>> {
    if handle == 0 || aad.len() > 64 * 1024 {
        return Err(TrustyWireError::InvalidArgs);
    }
    let auth_token = hardware_auth_token
        .map(decode_hardware_auth_token)
        .transpose()?;
    PerformOpReq::OperationUpdateAad(UpdateAadRequest {
        op_handle: handle,
        input: aad.to_vec(),
        auth_token,
        timestamp_token: None,
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_keymint_update_aad(bytes: &[u8]) -> TrustyResult<()> {
    match decode_response(bytes)? {
        PerformOpRsp::OperationUpdateAad(_) => Ok(()),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

pub fn encode_keymint_finish(
    handle: i64,
    input: &[u8],
    hardware_auth_token: Option<&[u8]>,
) -> TrustyResult<Vec<u8>> {
    if handle == 0 || input.len() > MAX_PAYLOAD_LEN {
        return Err(TrustyWireError::InvalidArgs);
    }
    let auth_token = hardware_auth_token
        .map(decode_hardware_auth_token)
        .transpose()?;
    PerformOpReq::OperationFinish(FinishRequest {
        op_handle: handle,
        input: Some(input.to_vec()),
        signature: None,
        auth_token,
        timestamp_token: None,
        confirmation_token: None,
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_keymint_finish(bytes: &[u8]) -> TrustyResult<Vec<u8>> {
    match decode_response(bytes)? {
        PerformOpRsp::OperationFinish(response) => Ok(response.ret),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

pub fn decode_keymint_signature(
    bytes: &[u8],
    algorithm: KeyMintAlgorithm,
) -> TrustyResult<Vec<u8>> {
    let signature = decode_keymint_finish(bytes)?;
    match algorithm {
        KeyMintAlgorithm::Ed25519 if signature.len() == ED25519_SIGNATURE_LEN => Ok(signature),
        KeyMintAlgorithm::P256 if (64..=72).contains(&signature.len()) => Ok(signature),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

pub fn decode_keymint_hmac_sha256(bytes: &[u8]) -> TrustyResult<[u8; KEY_LEN_256]> {
    decode_keymint_finish(bytes)?
        .try_into()
        .map_err(|_| TrustyWireError::InvalidResponse)
}

pub fn encode_keymint_delete_key(opaque_blob: &[u8]) -> TrustyResult<Vec<u8>> {
    validate_blob(opaque_blob)?;
    PerformOpReq::DeviceDeleteKey(DeleteKeyRequest {
        key_blob: opaque_blob.to_vec(),
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_keymint_delete_key(bytes: &[u8]) -> TrustyResult<()> {
    match decode_response(bytes)? {
        PerformOpRsp::DeviceDeleteKey(_) => Ok(()),
        _ => Err(TrustyWireError::InvalidResponse),
    }
}

fn generation_params(algorithm: KeyMintAlgorithm) -> Vec<KeyParam> {
    match algorithm {
        KeyMintAlgorithm::Ed25519 => alloc::vec![
            KeyParam::Purpose(KeyPurpose::Sign),
            KeyParam::Algorithm(Algorithm::Ec),
            KeyParam::EcCurve(EcCurve::Curve25519),
            KeyParam::Digest(Digest::None),
        ],
        KeyMintAlgorithm::P256 => alloc::vec![
            KeyParam::Purpose(KeyPurpose::Sign),
            KeyParam::Algorithm(Algorithm::Ec),
            KeyParam::EcCurve(EcCurve::P256),
            KeyParam::Digest(Digest::None),
        ],
        KeyMintAlgorithm::AesGcm => alloc::vec![
            KeyParam::Purpose(KeyPurpose::Encrypt),
            KeyParam::Purpose(KeyPurpose::Decrypt),
            KeyParam::Algorithm(Algorithm::Aes),
            KeyParam::KeySize(kmr_wire::KeySizeInBits(256)),
            KeyParam::BlockMode(BlockMode::Gcm),
            KeyParam::Padding(kmr_wire::keymint::PaddingMode::None),
            KeyParam::MinMacLength(128),
        ],
        KeyMintAlgorithm::HmacSha256 => alloc::vec![
            KeyParam::Purpose(KeyPurpose::Sign),
            KeyParam::Algorithm(Algorithm::Hmac),
            KeyParam::KeySize(kmr_wire::KeySizeInBits(256)),
            KeyParam::Digest(Digest::Sha256),
            KeyParam::MinMacLength(256),
        ],
    }
}

fn operation_params(
    algorithm: KeyMintAlgorithm,
    purpose: KeyMintPurpose,
) -> TrustyResult<Vec<KeyParam>> {
    match (algorithm, purpose) {
        (KeyMintAlgorithm::Ed25519 | KeyMintAlgorithm::P256, KeyMintPurpose::Sign) => {
            Ok(alloc::vec![KeyParam::Digest(Digest::None)])
        }
        (KeyMintAlgorithm::AesGcm, KeyMintPurpose::Encrypt | KeyMintPurpose::Decrypt) => {
            Ok(alloc::vec![
                KeyParam::BlockMode(BlockMode::Gcm),
                KeyParam::Padding(kmr_wire::keymint::PaddingMode::None),
                KeyParam::MacLength(128)
            ])
        }
        (KeyMintAlgorithm::HmacSha256, KeyMintPurpose::Sign) => Ok(alloc::vec![
            KeyParam::Digest(Digest::Sha256),
            KeyParam::MacLength(256),
        ]),
        _ => Err(TrustyWireError::InvalidArgs),
    }
}

fn keymint_purpose(purpose: KeyMintPurpose) -> KeyPurpose {
    match purpose {
        KeyMintPurpose::Sign => KeyPurpose::Sign,
        KeyMintPurpose::Encrypt => KeyPurpose::Encrypt,
        KeyMintPurpose::Decrypt => KeyPurpose::Decrypt,
    }
}

pub(crate) fn decode_response(bytes: &[u8]) -> TrustyResult<PerformOpRsp> {
    let response =
        PerformOpResponse::from_slice(bytes).map_err(|_| TrustyWireError::InvalidResponse)?;
    if response.error_code != 0 {
        return Err(TrustyWireError::SecureService(response.error_code));
    }
    response.rsp.ok_or(TrustyWireError::InvalidResponse)
}

fn decode_hardware_auth_token(bytes: &[u8]) -> TrustyResult<HardwareAuthToken> {
    if bytes.len() != 69 || bytes.len() > MAX_AUTH_TOKEN_LEN || bytes[0] != 0 {
        return Err(TrustyWireError::InvalidArgs);
    }
    let authenticator_type = u32::from_be_bytes(bytes[25..29].try_into().unwrap());
    let authenticator_type = match authenticator_type {
        1 => HardwareAuthenticatorType::Password,
        2 => HardwareAuthenticatorType::Fingerprint,
        u32::MAX => HardwareAuthenticatorType::Any,
        _ => return Err(TrustyWireError::InvalidArgs),
    };
    Ok(HardwareAuthToken {
        challenge: i64::from_le_bytes(bytes[1..9].try_into().unwrap()),
        user_id: i64::from_le_bytes(bytes[9..17].try_into().unwrap()),
        authenticator_id: i64::from_le_bytes(bytes[17..25].try_into().unwrap()),
        authenticator_type,
        timestamp: Timestamp {
            milliseconds: i64::from_be_bytes(bytes[29..37].try_into().unwrap()),
        },
        mac: bytes[37..69].to_vec(),
    })
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
