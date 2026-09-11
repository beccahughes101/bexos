//! Per-boot authentication-token key negotiation for the single Trusty instance.
use crate::{
    keymint::decode_response,
    protocol::{TrustyResult, TrustyWireError},
};
use alloc::{vec, vec::Vec};
use kmr_wire::{
    AsCborValue, ComputeSharedSecretRequest, GetSharedSecretParametersRequest, PerformOpReq,
    PerformOpRsp,
};

pub const GET_PARAMETERS: u32 = 0x51;
pub const COMPUTE_SHARED_SECRET: u32 = 0x52;

pub fn get_parameters() -> TrustyResult<Vec<u8>> {
    PerformOpReq::SharedSecretGetSharedSecretParameters(GetSharedSecretParametersRequest {})
        .into_vec()
        .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn compute_for_single_instance(response: &[u8]) -> TrustyResult<Vec<u8>> {
    let PerformOpRsp::SharedSecretGetSharedSecretParameters(response) = decode_response(response)?
    else {
        return Err(TrustyWireError::InvalidResponse);
    };
    if !matches!(response.ret.seed.len(), 0 | 32) || response.ret.nonce.len() != 32 {
        return Err(TrustyWireError::InvalidResponse);
    }
    PerformOpReq::SharedSecretComputeSharedSecret(ComputeSharedSecretRequest {
        params: vec![response.ret],
    })
    .into_vec()
    .map_err(|_| TrustyWireError::InvalidArgs)
}

pub fn decode_check(response: &[u8]) -> TrustyResult<[u8; 32]> {
    let PerformOpRsp::SharedSecretComputeSharedSecret(response) = decode_response(response)? else {
        return Err(TrustyWireError::InvalidResponse);
    };
    response
        .ret
        .try_into()
        .map_err(|_| TrustyWireError::InvalidResponse)
}
