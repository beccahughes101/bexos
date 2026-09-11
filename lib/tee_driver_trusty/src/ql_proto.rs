#![allow(dead_code)]

pub const PAGE_SIZE: usize = 4096;
// Eight RPMB frames plus storage and QL headers exceed a single page.
pub const QL_BUFFER_SIZE: usize = 2 * PAGE_SIZE;
pub const QL_HEADER_SIZE: usize = 16;
pub const QL_RESP_BIT: u16 = 0x8000;
pub const QL_FLAG_HAS_EVENT: u16 = 0x0100;
pub const QL_OP_CONNECT: u16 = 1;
pub const QL_OP_GET_EVENT: u16 = 2;
pub const QL_OP_SEND: u16 = 3;
pub const QL_OP_RECV: u16 = 4;
pub const QL_OP_DISCONNECT: u16 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub opcode: u16,
    pub flags: u16,
    pub status: u32,
    pub handle: u32,
    pub payload_len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    BufferTooSmall,
    PayloadTooLarge,
    InvalidResponse,
}

impl Header {
    pub const fn new(opcode: u16, handle: u32, payload_len: u32) -> Self {
        Self {
            opcode,
            flags: 0,
            status: 0,
            handle,
            payload_len,
        }
    }
}

pub fn max_payload_len(buffer_len: usize) -> usize {
    buffer_len.saturating_sub(QL_HEADER_SIZE)
}

pub fn encode_header(out: &mut [u8], header: Header) -> Result<(), Error> {
    if out.len() < QL_HEADER_SIZE {
        return Err(Error::BufferTooSmall);
    }
    out[0..2].copy_from_slice(&header.opcode.to_le_bytes());
    out[2..4].copy_from_slice(&header.flags.to_le_bytes());
    out[4..8].copy_from_slice(&header.status.to_le_bytes());
    out[8..12].copy_from_slice(&header.handle.to_le_bytes());
    out[12..16].copy_from_slice(&header.payload_len.to_le_bytes());
    Ok(())
}

pub fn decode_header(input: &[u8]) -> Result<Header, Error> {
    if input.len() < QL_HEADER_SIZE {
        return Err(Error::BufferTooSmall);
    }
    Ok(Header {
        opcode: u16::from_le_bytes(input[0..2].try_into().unwrap()),
        flags: u16::from_le_bytes(input[2..4].try_into().unwrap()),
        status: u32::from_le_bytes(input[4..8].try_into().unwrap()),
        handle: u32::from_le_bytes(input[8..12].try_into().unwrap()),
        payload_len: u32::from_le_bytes(input[12..16].try_into().unwrap()),
    })
}

pub fn encode_command(
    buffer: &mut [u8],
    opcode: u16,
    handle: u32,
    payload: &[u8],
) -> Result<(), Error> {
    if payload.len() > max_payload_len(buffer.len()) {
        return Err(Error::PayloadTooLarge);
    }
    let payload_len = u32::try_from(payload.len()).map_err(|_| Error::PayloadTooLarge)?;
    encode_header(buffer, Header::new(opcode, handle, payload_len))?;
    buffer[QL_HEADER_SIZE..QL_HEADER_SIZE + payload.len()].copy_from_slice(payload);
    Ok(())
}

pub fn response_payload(buffer: &[u8], expected_opcode: u16) -> Result<(Header, &[u8]), Error> {
    let header = decode_header(buffer)?;
    if header.opcode != (expected_opcode | QL_RESP_BIT) {
        return Err(Error::InvalidResponse);
    }
    let payload_len = usize::try_from(header.payload_len).map_err(|_| Error::InvalidResponse)?;
    let end = QL_HEADER_SIZE
        .checked_add(payload_len)
        .ok_or(Error::InvalidResponse)?;
    let payload = buffer
        .get(QL_HEADER_SIZE..end)
        .ok_or(Error::InvalidResponse)?;
    Ok((header, payload))
}

/// Decode the one-byte continuation marker used by KeyMint's non-secure TIPC
/// port. The marker is transport framing and is not part of the CBOR response.
pub fn keymint_response_chunk(payload: &[u8]) -> Result<(bool, &[u8]), Error> {
    let Some((&marker, content)) = payload.split_first() else {
        return Err(Error::InvalidResponse);
    };
    match marker {
        0 => Ok((false, content)),
        1 => Ok((true, content)),
        _ => Err(Error::InvalidResponse),
    }
}
