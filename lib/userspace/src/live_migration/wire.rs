//! Handle-free requests sent directly between migration peers.
use alloc::{vec, vec::Vec};
use bexos_migration::Error;
use migration_fidl::{
    FidlDecode, FidlEncode, StateReceiverAdoptDeltaResponse, StateReceiverValidateResponse,
};

/// Encodes control requests and full-size delta records with the same bound.
pub fn encode_request<Q: FidlEncode>(ordinal: u64, request: &Q) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0; 65500];
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    let encoded = request
        .encode(&mut bytes[8..], &mut [])
        .map_err(|_| Error::InvalidData)?;
    bytes.truncate(8 + encoded.bytes);
    Ok(bytes)
}

pub fn decode_delta_ack(bytes: &[u8], has_handles: bool, generation: u64) -> Result<(), Error> {
    // Two routing words followed by the fixed eight-byte FIDL response.
    if has_handles
        || bytes.len() != 24
        || u64::from_le_bytes(bytes[..8].try_into().unwrap()) != generation
        || u64::from_le_bytes(bytes[8..16].try_into().unwrap()) != 4
    {
        return Err(Error::InvalidData);
    }
    let response = StateReceiverAdoptDeltaResponse::decode(&bytes[16..], &[])
        .map_err(|_| Error::InvalidData)?;
    if response.status != 0 {
        return Err(Error::BadState);
    }
    Ok(())
}

pub fn decode_validate_ack(bytes: &[u8], has_handles: bool, generation: u64) -> Result<(), Error> {
    // Two routing words followed by the fixed eight-byte FIDL response.
    if has_handles
        || bytes.len() != 24
        || u64::from_le_bytes(bytes[..8].try_into().unwrap()) != generation
        || u64::from_le_bytes(bytes[8..16].try_into().unwrap()) != 5
    {
        return Err(Error::InvalidData);
    }
    let response =
        StateReceiverValidateResponse::decode(&bytes[16..], &[]).map_err(|_| Error::InvalidData)?;
    if response.status != 0 {
        return Err(Error::BadState);
    }
    Ok(())
}
