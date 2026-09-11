#![no_std]
extern crate alloc;
use alloc::vec::Vec;
use bexos_userspace::{Channel, Rpc};
use rpmb_fidl::{
    FidlDecode, FidlEncode, RpmbTransportExchangeRequest, RpmbTransportExchangeResponse,
};

pub const FRAME_SIZE: usize = 512;
pub const MAX_FRAMES: usize = 8;
const HEADER: usize = 24;

pub fn exchange(channel: Channel, frames: &[u8], read_frames: usize) -> Result<Vec<u8>, i32> {
    validate_frames(frames, read_frames)?;
    let request = RpmbTransportExchangeRequest {
        read_frames: read_frames as u32,
        frames,
    };
    let mut encoded = alloc::vec![0; 8192];
    let result = request.encode(&mut encoded, &mut []).map_err(|_| -8)?;
    let reply = Rpc(channel)
        .call_raw(1, &encoded[..result.bytes], &[], true)
        .map_err(|_| -5)?;
    if !reply.handles.is_empty() {
        for handle in reply.handles {
            let _ = bexos_userspace::Memory::close(handle);
        }
        return Err(-8);
    }
    let reply = RpmbTransportExchangeResponse::decode(&reply.bytes, &[]).map_err(|_| -8)?;
    if reply.status != 0 {
        return Err(reply.status);
    }
    let bytes = reply.frames.to_vec();
    if bytes.len() != read_frames * FRAME_SIZE {
        return Err(-8);
    }
    Ok(bytes)
}

pub fn validate_frames(frames: &[u8], count: usize) -> Result<(), i32> {
    if frames.is_empty()
        || frames.len() % FRAME_SIZE != 0
        || frames.len() > FRAME_SIZE * MAX_FRAMES
        || !(1..=MAX_FRAMES).contains(&count)
    {
        return Err(-8);
    }
    Ok(())
}

/// Translate the pinned upstream storage proxy protocol, without interpreting
/// authenticated RPMB frames. Optional nonsecure filesystems are unavailable.
pub fn dispatch(
    request: &[u8],
    mut exchange: impl FnMut(&[u8], usize) -> Result<Vec<u8>, i32>,
) -> Result<Vec<u8>, i32> {
    if request.len() < HEADER {
        return Err(-8);
    }
    let word = |offset| u32::from_le_bytes(request[offset..offset + 4].try_into().unwrap());
    let command = word(0);
    if command & 1 != 0 || word(12) as usize != request.len() || word(16) != 0 || word(20) != 0 {
        return Err(-8);
    }
    let mut response = request[..HEADER].to_vec();
    response[..4].copy_from_slice(&(command | 1).to_le_bytes());
    response[8..12].fill(0);
    let mut result: u32 = 0;
    let payload = &request[HEADER..];
    if word(8) & !0x1e != 0 {
        result = 2;
    } else if command == 16 {
        if payload.len() < 16 {
            result = 2;
        } else {
            let reliable = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
            let written = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
            let read = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
            let frames = &payload[16..];
            if reliable.checked_add(written) != Some(frames.len())
                || reliable % 512 != 0
                || written % 512 != 0
                || read % 512 != 0
                || payload[12..16] != [0; 4]
                || validate_frames(frames, read / 512).is_err()
            {
                result = 2;
            } else {
                match exchange(frames, read / 512) {
                    Ok(bytes) if bytes.len() == read => response.extend_from_slice(&bytes),
                    _ => result = 1,
                }
            }
        }
    } else if command == 4 {
        result = 5;
    }
    // optional NS filesystem not present
    else {
        result = 3;
    }
    response[16..20].copy_from_slice(&result.to_le_bytes());
    let size = response.len() as u32;
    response[12..16].copy_from_slice(&size.to_le_bytes());
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Vec<u8> {
        let mut request = alloc::vec![0; 40 + 512];
        request[..4].copy_from_slice(&16u32.to_le_bytes());
        request[4..8].copy_from_slice(&42u32.to_le_bytes());
        let size = request.len() as u32;
        request[12..16].copy_from_slice(&size.to_le_bytes());
        request[28..32].copy_from_slice(&512u32.to_le_bytes());
        request[32..36].copy_from_slice(&512u32.to_le_bytes());
        request
    }
    #[test]
    fn authenticated_frames_are_opaque_and_operation_id_is_preserved() {
        let reply = dispatch(&request(), |frames, n| {
            assert_eq!(n, 1);
            assert_eq!(frames.len(), 512);
            Ok(alloc::vec![0xa5;512])
        })
        .unwrap();
        assert_eq!(&reply[..8], &[17, 0, 0, 0, 42, 0, 0, 0]);
        assert_eq!(&reply[24..], &[0xa5; 512]);
    }
    #[test]
    fn malformed_requests_do_not_reach_device() {
        let mut input = request();
        input[32..36].copy_from_slice(&513u32.to_le_bytes());
        let output = dispatch(&input, |_, _| panic!("invalid request forwarded")).unwrap();
        assert_eq!(output[16], 2);
        input.truncate(23);
        assert!(dispatch(&input, |_, _| panic!()).is_err());
    }
    #[test]
    fn short_response_and_disconnect_are_errors() {
        for result in [Ok(alloc::vec![0;511]), Err(-5)] {
            let output = dispatch(&request(), |_, _| result.clone()).unwrap();
            assert_eq!(output[16], 1);
            assert_eq!(output.len(), HEADER);
        }
    }
}
