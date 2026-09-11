mod shared_secret_tests;

use bexos_trusty_client::protocol::{
    AUTHMGR_BE_PORT, AUTHMGR_BE_UUID, AUTHMGR_FE_UUID, AVB_PORT, GATEKEEPER_PORT, KEYMINT_PORT,
    ORCHESTRATOR_PORT, STORAGE_PROXY_PORT, STORAGE_UUID, service_ports,
};
use bexos_trusty_client::services::{
    AVB_CMD_GET_VERSION, AVB_CMD_LOCK_BOOT_STATE, AVB_CMD_READ_LOCK_STATE,
    AVB_CMD_READ_ROLLBACK_INDEX, AVB_CMD_WRITE_LOCK_STATE, AVB_CMD_WRITE_ROLLBACK_INDEX,
    KeyMintAlgorithm, KeyMintPurpose, ORCHESTRATOR_CMD_GET_KERNEL_SLOT, decode_avb_empty,
    decode_avb_lock_state, decode_avb_u64, decode_avb_version, decode_gatekeeper_enroll,
    decode_gatekeeper_verify, decode_keymint_begin, decode_keymint_generated_key,
    decode_keymint_hmac_sha256, decode_orchestrator_response, encode_authmgr_raw_route,
    encode_authmgr_rpc_route, encode_avb_get_version, encode_avb_lock_boot_state,
    encode_avb_read_lock_state, encode_avb_rollback_index, encode_avb_write_lock_state,
    encode_gatekeeper_delete, encode_gatekeeper_enroll, encode_gatekeeper_reenroll,
    encode_gatekeeper_verify, encode_keymint_begin, encode_keymint_delete_key,
    encode_keymint_finish, encode_keymint_generate_auth_bound_key, encode_keymint_generate_key,
    encode_keymint_set_boot_info, encode_keymint_set_hal_info, encode_keymint_update_aad,
    encode_orchestrator_request,
};
use bexos_trusty_client::users::{HardwareAuthToken, ukek_hmac_message};

#[test]
fn generated_keys_require_secure_deletion_and_asymmetric_certificate_bounds() {
    use kmr_wire::{AsCborValue, PerformOpReq, keymint::KeyParam};
    for algorithm in [
        KeyMintAlgorithm::Ed25519,
        KeyMintAlgorithm::P256,
        KeyMintAlgorithm::AesGcm,
        KeyMintAlgorithm::HmacSha256,
    ] {
        let bytes = encode_keymint_generate_key(7, "alias", algorithm, false, 0, None).unwrap();
        let PerformOpReq::DeviceGenerateKey(request) = PerformOpReq::from_slice(&bytes).unwrap()
        else {
            panic!("expected key generation");
        };
        assert!(request.key_params.contains(&KeyParam::RollbackResistance));
        let asymmetric = matches!(
            algorithm,
            KeyMintAlgorithm::Ed25519 | KeyMintAlgorithm::P256
        );
        assert_eq!(
            request
                .key_params
                .iter()
                .any(|p| matches!(p, KeyParam::CertificateNotBefore(_))),
            asymmetric
        );
        assert_eq!(
            request
                .key_params
                .iter()
                .any(|p| matches!(p, KeyParam::CertificateNotAfter(_))),
            asymmetric
        );
    }
}

#[test]
fn declares_upstream_trusty_service_ports_and_retained_orchestrator() {
    let ports = service_ports();
    for port in [
        KEYMINT_PORT,
        GATEKEEPER_PORT,
        STORAGE_PROXY_PORT,
        AVB_PORT,
        AUTHMGR_BE_PORT,
        ORCHESTRATOR_PORT,
    ] {
        assert!(ports.iter().any(|candidate| candidate == port));
    }
    assert_eq!(
        AUTHMGR_FE_UUID,
        [
            0x9b, 0x3c, 0x1e, 0x9e, 0x18, 0x08, 0x4b, 0x98, 0x8f, 0xa9, 0x85, 0x92, 0xdf, 0xf3,
            0xa3, 0x37,
        ]
    );
    assert_eq!(
        AUTHMGR_BE_UUID,
        [
            0xf4, 0x76, 0x89, 0x56, 0x62, 0xd9, 0x49, 0x04, 0x95, 0x12, 0x86, 0xdf, 0x36, 0x0d,
            0x8d, 0x50,
        ]
    );
    assert_eq!(
        STORAGE_UUID,
        [
            0xce, 0xa8, 0x70, 0x6d, 0x6c, 0xb4, 0x49, 0xf3, 0xb9, 0x94, 0x29, 0xe0, 0xe4, 0x78,
            0xbd, 0x29,
        ]
    );
}

#[test]
fn keymint_wire_formats_reject_malformed_responses() {
    let req =
        encode_keymint_generate_key(7, "alias", KeyMintAlgorithm::Ed25519, false, 0, None).unwrap();
    assert!(!req.is_empty());
    assert!(decode_keymint_generated_key(b"\x00\x00\x00\x00").is_err());
    assert!(
        encode_keymint_begin(
            KeyMintAlgorithm::Ed25519,
            KeyMintPurpose::Sign,
            b"blob",
            None,
            None,
        )
        .is_ok()
    );
    assert!(
        encode_keymint_begin(
            KeyMintAlgorithm::AesGcm,
            KeyMintPurpose::Decrypt,
            b"blob",
            Some(&[0; 12]),
            None,
        )
        .is_ok()
    );
    assert!(encode_keymint_update_aad(9, b"aad", None).is_ok());
    assert!(encode_keymint_finish(9, b"payload", None).is_ok());
    assert!(decode_keymint_begin(&[0; 12]).is_err());
    assert!(decode_keymint_hmac_sha256(&[5; 32]).is_err());
    assert!(encode_keymint_delete_key(b"blob").is_ok());
    assert!(encode_keymint_delete_key(b"").is_err());
    assert!(
        encode_keymint_generate_auth_bound_key(
            7,
            "auth-bound",
            KeyMintAlgorithm::HmacSha256,
            99,
            300,
        )
        .is_ok()
    );
    assert!(
        encode_keymint_generate_auth_bound_key(
            7,
            "auth-bound",
            KeyMintAlgorithm::HmacSha256,
            0,
            300,
        )
        .is_err()
    );
    assert!(encode_keymint_set_boot_info(&[0x21; 32], true, 0, &[0x42; 32], 20260904).is_ok());
    assert!(encode_keymint_set_boot_info(&[], true, 0, &[0x42; 32], 20260904).is_err());
    assert!(encode_keymint_set_boot_info(&[0x21; 32], true, 4, &[0x42; 32], 20260904).is_err());
    assert!(encode_keymint_set_boot_info(&[0x21; 32], true, 0, &[0x42; 31], 20260904).is_err());
    assert!(encode_keymint_set_hal_info(1, 202609, 20260904).is_ok());
}

#[test]
fn gatekeeper_wire_formats_preserve_secure_timestamps() {
    let enroll_req = encode_gatekeeper_enroll(7, "password").unwrap();
    assert_eq!(&enroll_req[..12], &[0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0]);
    assert_eq!(
        encode_gatekeeper_verify(7, 99, b"hndl", "password").unwrap()[..12],
        [2, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0]
    );
    let reenroll = encode_gatekeeper_reenroll(7, b"old-handle", "old", "new").unwrap();
    assert_eq!(&reenroll[..12], &[0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0]);
    assert_eq!(&reenroll[12..19], &[3, 0, 0, 0, b'n', b'e', b'w']);
    assert!(encode_gatekeeper_reenroll(7, b"", "old", "new").is_err());
    assert_eq!(
        encode_gatekeeper_delete(7, 99).unwrap(),
        vec![4, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0]
    );

    let mut password_handle = vec![2];
    password_handle.extend_from_slice(&99u64.to_le_bytes());
    password_handle.extend_from_slice(&0u64.to_le_bytes());
    password_handle.extend_from_slice(&123u64.to_le_bytes());
    password_handle.extend_from_slice(&[0x55; 32]);
    password_handle.push(1);

    let mut upstream_enroll = Vec::new();
    upstream_enroll.extend_from_slice(&1u32.to_le_bytes());
    upstream_enroll.extend_from_slice(&0u32.to_le_bytes());
    upstream_enroll.extend_from_slice(&7u32.to_le_bytes());
    upstream_enroll.extend_from_slice(&(password_handle.len() as u32).to_le_bytes());
    upstream_enroll.extend_from_slice(&password_handle);
    let parsed_enroll = decode_gatekeeper_enroll(&upstream_enroll).unwrap();
    assert_eq!(parsed_enroll.secure_user_id, 99);
    assert_eq!(parsed_enroll.password_handle, password_handle);

    let mut hat = vec![0; 69];
    hat[9..17].copy_from_slice(&99u64.to_le_bytes());
    hat[29..37].copy_from_slice(&1234u64.to_be_bytes());
    let mut upstream_verify = Vec::new();
    upstream_verify.extend_from_slice(&3u32.to_le_bytes());
    upstream_verify.extend_from_slice(&0u32.to_le_bytes());
    upstream_verify.extend_from_slice(&7u32.to_le_bytes());
    upstream_verify.extend_from_slice(&(hat.len() as u32).to_le_bytes());
    upstream_verify.extend_from_slice(&hat);
    upstream_verify.push(0);
    let token = decode_gatekeeper_verify(&upstream_verify).unwrap();
    assert_eq!(token.secure_user_id, 99);
    assert_eq!(token.secure_timestamp_ms, 1234);
    assert_eq!(token.auth_token, hat);

    *upstream_verify.last_mut().unwrap() = 1;
    assert!(decode_gatekeeper_verify(&upstream_verify).is_ok());
    *upstream_verify.last_mut().unwrap() = 2;
    assert!(decode_gatekeeper_verify(&upstream_verify).is_err());
    upstream_verify.pop();
    assert!(decode_gatekeeper_verify(&upstream_verify).is_err());
    upstream_verify.extend_from_slice(&0u32.to_le_bytes());
    assert!(decode_gatekeeper_verify(&upstream_verify).is_err());

    let mut enroll = Vec::new();
    enroll.extend_from_slice(&99u64.to_le_bytes());
    enroll.extend_from_slice(&4u32.to_le_bytes());
    enroll.extend_from_slice(b"hndl");
    assert_eq!(
        decode_gatekeeper_enroll(&enroll).unwrap().secure_user_id,
        99
    );

    let mut verify = Vec::new();
    verify.extend_from_slice(&99u64.to_le_bytes());
    verify.extend_from_slice(&1234u64.to_le_bytes());
    verify.extend_from_slice(&3u32.to_le_bytes());
    verify.extend_from_slice(b"hat");
    let token = decode_gatekeeper_verify(&verify).unwrap();
    assert_eq!(token.secure_timestamp_ms, 1234);
    assert_eq!(token.auth_token, b"hat");
}

#[test]
fn avb_and_authmgr_framing_is_bounded_and_typed() {
    assert_eq!(
        encode_avb_get_version()[..4],
        AVB_CMD_GET_VERSION.to_le_bytes()
    );
    let mut version_response = Vec::new();
    version_response.extend_from_slice(&(AVB_CMD_GET_VERSION | 1).to_le_bytes());
    version_response.extend_from_slice(&0u32.to_le_bytes());
    version_response.extend_from_slice(&3u32.to_le_bytes());
    assert_eq!(decode_avb_version(&version_response).unwrap(), 3);

    let read = encode_avb_rollback_index(AVB_CMD_READ_ROLLBACK_INDEX, 31, 0).unwrap();
    assert_eq!(&read[..8], &[0; 8]);
    assert_eq!(&read[8..16], &[0; 8]);
    assert_eq!(&read[16..], &31u32.to_le_bytes());
    let write = encode_avb_rollback_index(AVB_CMD_WRITE_ROLLBACK_INDEX, 3, 9).unwrap();
    assert_eq!(&write[..4], &AVB_CMD_WRITE_ROLLBACK_INDEX.to_le_bytes());
    assert_eq!(&write[8..16], &9u64.to_le_bytes());
    assert!(encode_avb_rollback_index(AVB_CMD_READ_ROLLBACK_INDEX, 32, 9).is_err());
    assert!(encode_avb_rollback_index(99, 0, 0).is_err());

    assert_eq!(
        encode_avb_read_lock_state()[..4],
        AVB_CMD_READ_LOCK_STATE.to_le_bytes()
    );
    assert_eq!(
        encode_avb_write_lock_state(true)[..4],
        AVB_CMD_WRITE_LOCK_STATE.to_le_bytes()
    );
    assert_eq!(encode_avb_write_lock_state(true)[8], 1);
    assert_eq!(
        encode_avb_lock_boot_state()[..4],
        AVB_CMD_LOCK_BOOT_STATE.to_le_bytes()
    );

    let mut rollback_response = Vec::new();
    rollback_response.extend_from_slice(&(AVB_CMD_READ_ROLLBACK_INDEX | 1).to_le_bytes());
    rollback_response.extend_from_slice(&0u32.to_le_bytes());
    rollback_response.extend_from_slice(&9u64.to_le_bytes());
    assert_eq!(
        decode_avb_u64(&rollback_response, AVB_CMD_READ_ROLLBACK_INDEX).unwrap(),
        9
    );
    // Upstream uses RollbackIndexResponse for writes too, returning the
    // persisted index rather than an empty acknowledgement.
    rollback_response[..4].copy_from_slice(&(AVB_CMD_WRITE_ROLLBACK_INDEX | 1).to_le_bytes());
    assert_eq!(
        decode_avb_u64(&rollback_response, AVB_CMD_WRITE_ROLLBACK_INDEX).unwrap(),
        9
    );
    assert!(decode_avb_u64(&rollback_response[..8], AVB_CMD_WRITE_ROLLBACK_INDEX).is_err());
    rollback_response[4] = 1;
    assert!(decode_avb_u64(&rollback_response, AVB_CMD_WRITE_ROLLBACK_INDEX).is_err());
    let mut lock_response = Vec::new();
    lock_response.extend_from_slice(&(AVB_CMD_READ_LOCK_STATE | 1).to_le_bytes());
    lock_response.extend_from_slice(&0u32.to_le_bytes());
    lock_response.push(1);
    assert!(decode_avb_lock_state(&lock_response).unwrap());
    let mut boot_locked = Vec::new();
    boot_locked.extend_from_slice(&(AVB_CMD_LOCK_BOOT_STATE | 1).to_le_bytes());
    boot_locked.extend_from_slice(&0u32.to_le_bytes());
    assert!(decode_avb_empty(&boot_locked, AVB_CMD_LOCK_BOOT_STATE).is_ok());
    boot_locked[4] = 1;
    assert!(decode_avb_empty(&boot_locked, AVB_CMD_LOCK_BOOT_STATE).is_err());
    assert_eq!(encode_authmgr_rpc_route(), [0]);
    let raw = encode_authmgr_raw_route(&[0xa5; 32]).unwrap();
    assert_eq!(raw[0], 1);
    assert_eq!(&raw[1..], &[0xa5; 32]);
    assert!(encode_authmgr_raw_route(&[0; 31]).is_err());
}

#[test]
fn ukek_message_and_token_expiry_match_usersd_contract() {
    let mut expected = b"bexos.ukek.v1\0".to_vec();
    expected.extend_from_slice(&7u64.to_le_bytes());
    assert_eq!(ukek_hmac_message(7), expected);
    let token = HardwareAuthToken::new(7, 99, 1000, b"hat".to_vec());
    assert!(token.is_valid_at(1000));
    assert!(token.is_valid_at(300_999));
    assert!(!token.is_valid_at(301_000));
}

#[test]
fn orchestrator_protocol_is_typed_and_rejects_malformed_responses() {
    let request = encode_orchestrator_request(ORCHESTRATOR_CMD_GET_KERNEL_SLOT, 1).unwrap();
    assert_eq!(&request[..4], &1u32.to_le_bytes());
    assert_eq!(
        &request[4..8],
        &ORCHESTRATOR_CMD_GET_KERNEL_SLOT.to_le_bytes()
    );
    let mut response = [0u8; 16];
    response[..4].copy_from_slice(&1u32.to_le_bytes());
    response[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(decode_orchestrator_response(&response).unwrap(), 1);
    response[4] = 1;
    assert!(decode_orchestrator_response(&response).is_err());
    assert!(decode_orchestrator_response(&response[..15]).is_err());
    assert!(encode_orchestrator_request(0xffff, 1).is_err());
}
