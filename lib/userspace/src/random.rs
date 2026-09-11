//! Kernel-backed randomness fails closed when no boot entropy is available.
use crate::KernelTransport;
use alloc::vec::Vec;
use kernel_fidl::{
    FidlDecode, FidlEncode, FidlTransport, HandleRef, RandomGetBytesRequest,
    RandomGetBytesResponse, Status,
};
pub fn bytes(length: usize) -> Result<Vec<u8>, Status> {
    if length > 32768 {
        return Err(Status::ErrInvalidArgs);
    }
    let mut request = [0; 32];
    let encoded = RandomGetBytesRequest {
        length: length as u32,
    }
    .encode(&mut request, &mut [])
    .map_err(|_| Status::ErrInvalidArgs)?;
    let mut response = alloc::vec![0;length+128];
    let mut handles: [HandleRef; 0] = [];
    let encoded = KernelTransport(13)
        .call(
            1,
            &request[..encoded.bytes],
            &[],
            &mut response,
            &mut handles,
        )
        .map_err(|_| Status::ErrAccessDenied)?;
    if encoded.handles != 0 {
        return Err(Status::ErrInvalidArgs);
    }
    let response = RandomGetBytesResponse::decode(&response[..encoded.bytes], &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    crate::ipc::check(response.status)?;
    if response.data.len() != length {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(response.data.to_vec())
}
