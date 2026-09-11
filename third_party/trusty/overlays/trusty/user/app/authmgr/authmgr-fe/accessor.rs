/*
 * Copyright (C) 2025 The Android Open Source Project
 * Copyright 2026 The BexOS Authors
 * SPDX-License-Identifier: Apache-2.0
 */

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use android_hardware_security_see_authmgr::aidl::android::hardware::security::see::authmgr::{
    DiceChainEntry::DiceChainEntry as AidlDiceChainEntry,
    DiceLeafArtifacts::DiceLeafArtifacts, DicePolicy::DicePolicy as AidlDicePolicy,
    ExplicitKeyDiceCertChain::ExplicitKeyDiceCertChain,
    IAuthMgrAuthorization::IAuthMgrAuthorization,
    SignedConnectionRequest::SignedConnectionRequest,
};
use authgraph_boringssl::BoringRng;
use authgraph_core::key::{CertChain, DiceChainEntry};
use authgraph_core::traits::Rng;
use authmgr_common::{
    signed_connection_request::{
        ConnectionRequest, CONNECTION_REQUEST_UUID, TEMP_AUTHMGR_BE_TRANSPORT_ID,
        TEMP_AUTHMGR_FE_TRANSPORT_ID,
    },
    CMD_RAW, CMD_RPC, TOKEN_LENGTH,
};
use authmgr_common_util::{get_constraint_spec_for_static_trusty_ta, policy_for_dice_node};
use binder::{
    BinderFeatures, Interface, ParcelFileDescriptor, Status, StatusCode, Strong,
};
use ciborium::Value;
use coset::{AsCborValue, CborOrdering, CborSerializable, CoseKey, CoseSign1};
use diced_open_dice::{
    bcc_handover_parse, retry_bcc_format_config_descriptor, retry_dice_main_flow, Config,
    DiceArtifacts as _, DiceConfigValues, DiceMode, Hash, Hidden, InputValues, HASH_SIZE,
    HIDDEN_SIZE,
};
use hwbcc::{
    get_bcc, get_dice_artifacts, sign_data, HwBccMode, SigningAlgorithm,
    HWBCC_MAX_RESP_PAYLOAD_LENGTH,
};
use rpcbinder::RpcSession;
use std::ffi::CStr;
use binder::fd_compat::{FromRawFd, OwnedFd};
use tipc::{Handle, Uuid};
use trusty_binder_accessor::aidl::trusty::os::ITrustyAccessor::{
    BnTrustyAccessor, ITrustyAccessor, ERROR_CONNECTION_INFO_NOT_FOUND,
    ERROR_FAILED_TO_CONNECT_TO_SOCKET, ERROR_FAILED_TO_CREATE_SOCKET,
};

const AUTHMGR_BE_PORT: &CStr = c"com.android.trusty.rust.authmgr.V1";
const FE_INSTANCE_ID: &[u8] = b"bexos.authmgr.fe.v1";
const EXPLICIT_KEY_DICE_CERT_CHAIN_VERSION: u64 = 1;
const ZERO_HASH: Hash = [0; HASH_SIZE];
const ZERO_HIDDEN: Hidden = [0; HIDDEN_SIZE];

pub enum SecurityConfig {
    Secure { target_port: &'static CStr },
    Insecure { target_port: &'static CStr },
}

pub struct AuthMgrAccessor {
    service_name: &'static str,
    security_config: SecurityConfig,
    uuid: Uuid,
}

impl AuthMgrAccessor {
    pub fn new_binder(
        service_name: &'static str,
        security_config: SecurityConfig,
        uuid: Uuid,
    ) -> Strong<dyn ITrustyAccessor> {
        BnTrustyAccessor::new_binder(
            Self { service_name, security_config, uuid },
            BinderFeatures::default(),
        )
    }
}

impl ITrustyAccessor for AuthMgrAccessor {
    fn addConnection(&self) -> Result<ParcelFileDescriptor, Status> {
        match self.security_config {
            SecurityConfig::Secure { target_port } => {
                add_secure_connection(self.service_name, target_port, &self.uuid)
            }
            SecurityConfig::Insecure { target_port } => add_insecure_connection(target_port),
        }
    }

    fn getInstanceName(&self) -> Result<String, Status> {
        let mut out_name = String::new();
        out_name
            .try_reserve_exact(self.service_name.len())
            .map_err(|_| StatusCode::NO_MEMORY)?;
        out_name.push_str(self.service_name);
        Ok(out_name)
    }
}

impl Interface for AuthMgrAccessor {}

fn add_secure_connection(
    service_name: &str,
    _target_port: &CStr,
    client_uuid: &Uuid,
) -> Result<ParcelFileDescriptor, Status> {
    let rpc_handle = connect_and_select_route(CMD_RPC)?;
    let rpc_fd = rpc_handle.as_raw_fd();
    let session = RpcSession::new();
    let authorization: Strong<dyn IAuthMgrAuthorization> = session
        .setup_preconnected_client(move || Some(rpc_fd))
        .map_err(|_| binder_failure(c"AuthMgr FE failed to establish the BE Binder route"))?;

    let mut bcc_buffer = [0u8; HWBCC_MAX_RESP_PAYLOAD_LENGTH];
    let bcc = get_bcc(HwBccMode::Release, &mut bcc_buffer)
        .map_err(|_| dice_failure(c"AuthMgr FE could not obtain its DICE chain"))?;
    let explicit_chain = to_explicit_chain(bcc)
        .map_err(|_| dice_failure(c"AuthMgr FE received an invalid DICE chain"))?;
    let challenge = authorization.initAuthentication(
        &ExplicitKeyDiceCertChain { diceCertChain: explicit_chain.clone() },
        Some(FE_INSTANCE_ID),
    )?;

    let connection_request = ConnectionRequest::new_for_ffa_transport(
        challenge,
        TEMP_AUTHMGR_FE_TRANSPORT_ID,
        TEMP_AUTHMGR_BE_TRANSPORT_ID,
    );
    let encoded_request = connection_request
        .to_vec()
        .map_err(|_| dice_failure(c"AuthMgr FE could not encode the connection request"))?;
    let aad = connection_request_aad();
    let mut signature_buffer = [0u8; HWBCC_MAX_RESP_PAYLOAD_LENGTH];
    let signature = sign_data(
        HwBccMode::Release,
        SigningAlgorithm::ED25519,
        &encoded_request,
        &aad,
        &mut signature_buffer,
    )
    .map_err(|_| dice_failure(c"AuthMgr FE could not sign the connection request"))?;
    // HWBCC embeds the signed payload. AuthMgr reconstructs the request from
    // its challenge and transport identities and requires detached COSE.
    let mut signature = CoseSign1::from_slice(signature)
        .map_err(|_| dice_failure(c"AuthMgr FE received a malformed HWBCC signature"))?;
    if signature.payload.take().as_deref() != Some(encoded_request.as_slice()) {
        return Err(dice_failure(c"AuthMgr FE HWBCC signature payload mismatch"));
    }
    let signature = signature.to_vec()
        .map_err(|_| dice_failure(c"AuthMgr FE could not encode its detached signature"))?;
    let fe_policy = dice_policy_builder::policy_for_dice_chain(&explicit_chain, Vec::new())
        .map_err(|_| dice_failure(c"AuthMgr FE could not construct its DICE policy"))?
        .to_vec()
        .map_err(|_| dice_failure(c"AuthMgr FE could not encode its DICE policy"))?;
    authorization.completeAuthentication(
        &SignedConnectionRequest { signedConnectionRequest: signature },
        &AidlDicePolicy { dicePolicy: fe_policy },
    )?;

    let (client_id, client_leaf, client_policy) = client_dice_artifacts(client_uuid, &explicit_chain)?;
    let mut token = [0u8; TOKEN_LENGTH];
    BoringRng.fill_bytes(&mut token);
    let raw_handle = connect_and_select_raw_route(&token)?;
    authorization.authorizeAndConnectClientToTrustedService(
        &client_id,
        service_name,
        &token,
        &DiceLeafArtifacts {
            diceLeaf: AidlDiceChainEntry { diceChainEntry: client_leaf },
            diceLeafPolicy: AidlDicePolicy { dicePolicy: client_policy },
        },
    )?;

    drop(authorization);
    drop(session);
    drop(rpc_handle);
    Ok(into_parcel_fd(raw_handle))
}

fn client_dice_artifacts(client_uuid: &Uuid, fe_chain: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), Status> {
    let client_id = uuid_bytes(client_uuid).to_vec();
    let component_name = uuid_component_name(&client_id)?;
    let config = retry_bcc_format_config_descriptor(&DiceConfigValues {
        component_name: Some(&component_name),
        component_version: Some(1),
        resettable: false,
        security_version: Some(1),
        rkp_vm_marker: false,
    })
    .map_err(|_| dice_failure(c"AuthMgr FE could not encode client identity"))?;

    let mut artifacts_buffer = [0u8; HWBCC_MAX_RESP_PAYLOAD_LENGTH];
    let artifacts = get_dice_artifacts(0, &mut artifacts_buffer)
        .map_err(|_| dice_failure(c"AuthMgr FE could not obtain client DICE material"))?;
    let handover = bcc_handover_parse(artifacts.artifacts)
        .map_err(|_| dice_failure(c"AuthMgr FE received malformed client DICE material"))?;
    let inputs = InputValues::new(
        ZERO_HASH,
        Config::Descriptor(&config),
        ZERO_HASH,
        DiceMode::kDiceModeDebug,
        ZERO_HIDDEN,
    );
    let (_, leaf) = retry_dice_main_flow(handover.cdi_attest(), handover.cdi_seal(), &inputs)
        .map_err(|_| dice_failure(c"AuthMgr FE could not derive the client DICE leaf"))?;
    let leaf = certify_client_leaf(fe_chain, &leaf)?;
    let parsed_leaf = DiceChainEntry::from_slice(&leaf)
        .map_err(|_| dice_failure(c"AuthMgr FE derived an invalid client DICE leaf"))?;
    let policy = policy_for_dice_node(&parsed_leaf, get_constraint_spec_for_static_trusty_ta())
        .map_err(|_| dice_failure(c"AuthMgr FE could not construct the client DICE policy"))?
        .to_vec()
        .map_err(|_| dice_failure(c"AuthMgr FE could not encode the client DICE policy"))?;
    Ok((client_id, leaf, policy))
}

fn certify_client_leaf(fe_chain: &[u8], leaf: &[u8]) -> Result<Vec<u8>, Status> {
    let chain = CertChain::from_slice(fe_chain)
        .map_err(|_| dice_failure(c"AuthMgr FE chain could not be decoded"))?;
    let issuer = chain.dice_cert_chain.as_ref().and_then(|entries| entries.last())
        .and_then(|entry| entry.payload.subject.clone())
        .ok_or_else(|| dice_failure(c"AuthMgr FE chain has no issuer identity"))?;
    let certificate = CoseSign1::from_slice(leaf)
        .map_err(|_| dice_failure(c"AuthMgr FE client certificate is malformed"))?;
    let payload = certificate.payload
        .ok_or_else(|| dice_failure(c"AuthMgr FE client certificate has no payload"))?;
    let mut payload = Value::from_slice(&payload)
        .map_err(|_| dice_failure(c"AuthMgr FE client certificate payload is malformed"))?;
    let Value::Map(ref mut fields) = payload else {
        return Err(dice_failure(c"AuthMgr FE client certificate payload is not a map"));
    };
    let (_, parent) = fields.iter_mut().find(|(key, _)| *key == Value::from(1))
        .ok_or_else(|| dice_failure(c"AuthMgr FE client certificate has no issuer"))?;
    *parent = Value::Text(issuer);
    let payload = payload.to_vec()
        .map_err(|_| dice_failure(c"AuthMgr FE could not encode the client certificate"))?;
    // The QEMU provider's handover describes its nonsecure child, whereas the
    // FE BCC uses a per-TA HWBCC key. Certify the transport-authenticated client
    // with that same FE key so its leaf extends the authenticated FE chain.
    let mut output = [0; HWBCC_MAX_RESP_PAYLOAD_LENGTH];
    let signed = sign_data(HwBccMode::Release, SigningAlgorithm::ED25519,
        &payload, &[], &mut output)
        .map_err(|_| dice_failure(c"AuthMgr FE could not certify the client identity"))?;
    let parsed = DiceChainEntry::from_slice(signed)
        .map_err(|_| dice_failure(c"AuthMgr FE received an invalid client certificate"))?;
    chain.extend_with(&parsed, &authgraph_boringssl::ec::BoringEcDsa)
        .and_then(|chain| chain.validate(&authgraph_boringssl::ec::BoringEcDsa))
        .map_err(|_| dice_failure(c"AuthMgr FE client certificate does not extend its chain"))?;
    try_copy(signed)
}

fn connect_and_select_route(route: u8) -> Result<Handle, Status> {
    let handle = Handle::connect(AUTHMGR_BE_PORT)
        .map_err(|_| socket_failure(c"AuthMgr FE could not connect to AuthMgr BE"))?;
    handle
        .send(&(&[route][..]))
        .map_err(|_| socket_failure(c"AuthMgr FE could not select the AuthMgr BE route"))?;
    Ok(handle)
}

fn connect_and_select_raw_route(token: &[u8; TOKEN_LENGTH]) -> Result<Handle, Status> {
    let handle = Handle::connect(AUTHMGR_BE_PORT)
        .map_err(|_| socket_failure(c"AuthMgr FE could not create the raw AuthMgr BE route"))?;
    let mut route = Vec::new();
    route.try_reserve_exact(1 + token.len()).map_err(|_| StatusCode::NO_MEMORY)?;
    route.push(CMD_RAW);
    route.extend_from_slice(token);
    handle
        .send(&route.as_slice())
        .map_err(|_| socket_failure(c"AuthMgr FE could not submit the raw-route token"))?;
    Ok(handle)
}

fn add_insecure_connection(port: &CStr) -> Result<ParcelFileDescriptor, Status> {
    let handle = Handle::connect(port).map_err(|_| {
        Status::new_service_specific_error(
            ERROR_FAILED_TO_CREATE_SOCKET,
            Some(c"AuthMgrAccessor failed to connect to port"),
        )
    })?;
    Ok(into_parcel_fd(handle))
}

fn into_parcel_fd(handle: Handle) -> ParcelFileDescriptor {
    let fd = handle.as_raw_fd();
    core::mem::forget(handle);
    // SAFETY: ownership of the live descriptor was removed from Handle above.
    ParcelFileDescriptor::new(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn try_copy(bytes: &[u8]) -> Result<Vec<u8>, Status> {
    let mut out = Vec::new();
    out.try_reserve_exact(bytes.len()).map_err(|_| StatusCode::NO_MEMORY)?;
    out.extend_from_slice(bytes);
    Ok(out)
}

fn uuid_bytes(uuid: &Uuid) -> [u8; Uuid::UUID_BYTE_LEN] {
    let mut bytes = [0; Uuid::UUID_BYTE_LEN];
    // SAFETY: Uuid::as_ptr points to a live UUID whose size is UUID_BYTE_LEN.
    unsafe {
        core::ptr::copy_nonoverlapping(uuid.as_ptr().cast::<u8>(), bytes.as_mut_ptr(), bytes.len());
    }
    bytes
}

fn uuid_component_name(uuid: &[u8]) -> Result<CString, Status> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(uuid.len() * 2 + 1).map_err(|_| StatusCode::NO_MEMORY)?;
    for byte in uuid {
        bytes.push(HEX[(byte >> 4) as usize]);
        bytes.push(HEX[(byte & 0xf) as usize]);
    }
    CString::new(bytes).map_err(|_| dice_failure(c"AuthMgr FE client UUID is malformed"))
}

fn connection_request_aad() -> [u8; 17] {
    // Match ExternalAADForDICESigned::to_vec in the pinned AuthMgr protocol.
    // That method encodes the byte string without the optional CBOR tag.
    let mut aad = [0u8; 17];
    aad[0] = 0x50;
    aad[1..].copy_from_slice(&CONNECTION_REQUEST_UUID);
    aad
}

fn to_explicit_chain(bytes: &[u8]) -> Result<Vec<u8>, ()> {
    let value = Value::from_slice(bytes).map_err(|_| ())?;
    let mut chain = value.into_array().map_err(|_| ())?;
    if matches!(&chain[..], [Value::Integer(_), Value::Bytes(_), ..]) {
        return try_copy(bytes).map_err(|_| ());
    }
    if chain.is_empty() {
        return Err(());
    }
    let root = chain.remove(0);
    let mut root = CoseKey::from_cbor_value(root).map_err(|_| ())?;
    root.canonicalize(CborOrdering::Lexicographic);
    let root = root.to_vec().map_err(|_| ())?;
    let mut explicit = Vec::new();
    explicit.try_reserve_exact(chain.len() + 2).map_err(|_| ())?;
    explicit.push(Value::from(EXPLICIT_KEY_DICE_CERT_CHAIN_VERSION));
    explicit.push(Value::Bytes(root));
    explicit.extend(chain);
    Value::Array(explicit).to_vec().map_err(|_| ())
}

fn dice_failure(message: &'static CStr) -> Status {
    Status::new_service_specific_error(ERROR_CONNECTION_INFO_NOT_FOUND, Some(message))
}

fn binder_failure(message: &'static CStr) -> Status {
    Status::new_exception(binder::ExceptionCode::TRANSACTION_FAILED, Some(message))
}

fn socket_failure(message: &'static CStr) -> Status {
    Status::new_service_specific_error(ERROR_FAILED_TO_CONNECT_TO_SOCKET, Some(message))
}
