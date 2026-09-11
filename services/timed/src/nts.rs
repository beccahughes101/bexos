use aes_siv::{KeyInit, aead::generic_array::GenericArray, siv::Aes128Siv};
use alloc::vec::Vec;
use time_fidl::{ClockSource, Status, TimeQuality};

use crate::sntp::{self, SntpSample};

pub const NTS_KE_PORT: u16 = 4460;
pub const NTS_ALPN: &[u8] = b"ntske/1";
pub const AEAD_AES_SIV_CMAC_256: u16 = 15;
pub const NTPV4_NEXT_PROTOCOL: u16 = 0;
const RECORD_END_OF_MESSAGE: u16 = 0;
const RECORD_NEXT_PROTOCOL: u16 = 1;
const RECORD_AEAD_ALGORITHM: u16 = 4;
const RECORD_NEW_COOKIE: u16 = 5;
const RECORD_SERVER: u16 = 6;
const RECORD_PORT: u16 = 7;
const CRITICAL: u16 = 0x8000;
const UNIQUE_IDENTIFIER: u16 = 0x0104;
const NTS_COOKIE: u16 = 0x0204;
const NTS_COOKIE_PLACEHOLDER: u16 = 0x0304;
const NTS_AUTHENTICATOR: u16 = 0x0404;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NtsCookieState {
    pub c2s_key: Vec<u8>,
    pub s2c_key: Vec<u8>,
    pub cookies: Vec<Vec<u8>>,
    pub server: Vec<u8>,
    pub port: u16,
    pub replay_window: Vec<Vec<u8>>,
}

impl NtsCookieState {
    pub fn configured(&self) -> bool {
        self.c2s_key.len() == 32 && self.s2c_key.len() == 32 && !self.cookies.is_empty()
    }
}

pub fn encode_ke_request(out: &mut Vec<u8>) {
    write_record(
        out,
        CRITICAL | RECORD_NEXT_PROTOCOL,
        &NTPV4_NEXT_PROTOCOL.to_be_bytes(),
    );
    write_record(
        out,
        CRITICAL | RECORD_AEAD_ALGORITHM,
        &AEAD_AES_SIV_CMAC_256.to_be_bytes(),
    );
    write_record(out, CRITICAL | RECORD_END_OF_MESSAGE, &[]);
}

pub fn adopt_ke_response(
    response: &[u8],
    default_server: &str,
    c2s_key: Vec<u8>,
    s2c_key: Vec<u8>,
) -> Result<NtsCookieState, Status> {
    let mut offset = 0usize;
    let mut saw_protocol = false;
    let mut saw_algorithm = false;
    let mut cookies = Vec::new();
    let mut server = default_server.as_bytes().to_vec();
    let mut port = 123u16;
    while offset + 4 <= response.len() {
        let raw_type = u16::from_be_bytes([response[offset], response[offset + 1]]);
        let record_type = raw_type & !CRITICAL;
        let critical = raw_type & CRITICAL != 0;
        let len = u16::from_be_bytes([response[offset + 2], response[offset + 3]]) as usize;
        offset += 4;
        let value = response
            .get(offset..offset + len)
            .ok_or(Status::ErrInvalidArgs)?;
        offset += len;
        match record_type {
            RECORD_END_OF_MESSAGE => break,
            RECORD_NEXT_PROTOCOL if value == NTPV4_NEXT_PROTOCOL.to_be_bytes() => {
                saw_protocol = true;
            }
            RECORD_AEAD_ALGORITHM if value == AEAD_AES_SIV_CMAC_256.to_be_bytes() => {
                saw_algorithm = true;
            }
            RECORD_NEW_COOKIE if cookies.len() < 8 && !value.is_empty() && value.len() <= 1024 => {
                cookies.push(value.to_vec());
            }
            RECORD_SERVER if !value.is_empty() && value.len() <= 255 => server = value.to_vec(),
            RECORD_PORT if value.len() == 2 => port = u16::from_be_bytes([value[0], value[1]]),
            _ if critical => return Err(Status::ErrInvalidArgs),
            _ => {}
        }
    }
    if !saw_protocol
        || !saw_algorithm
        || cookies.is_empty()
        || c2s_key.len() != 32
        || s2c_key.len() != 32
    {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(NtsCookieState {
        c2s_key,
        s2c_key,
        cookies,
        server,
        port: if port == 0 { 123 } else { port },
        replay_window: Vec::new(),
    })
}

pub fn encode_request(
    state: &mut NtsCookieState,
    out: &mut [u8],
) -> Result<(usize, Vec<u8>), Status> {
    let cookie = if state.cookies.is_empty() {
        return Err(Status::ErrTimedOut);
    } else {
        state.cookies.remove(0)
    };
    let base = sntp::encode_request(out)?;
    let unique = unique_identifier();
    let mut offset = base;
    offset = write_ext(out, offset, UNIQUE_IDENTIFIER, &unique)?;
    offset = write_ext(out, offset, NTS_COOKIE, &cookie)?;
    offset = write_ext(out, offset, NTS_COOKIE_PLACEHOLDER, &[0; 100])?;
    let tag = authenticator(&state.c2s_key, &out[..offset], &unique)?;
    offset = write_ext(out, offset, NTS_AUTHENTICATOR, &tag)?;
    Ok((offset, unique))
}

pub fn decode_response(
    packet: &[u8],
    state: &mut NtsCookieState,
    unique: &[u8],
) -> Result<SntpSample, Status> {
    if state
        .replay_window
        .iter()
        .any(|seen| seen.as_slice() == unique)
    {
        return Err(Status::ErrInvalidArgs);
    }
    let sample = sntp::decode_response(packet)?;
    let mut offset = 48usize;
    let mut saw_unique = false;
    let mut saw_auth = false;
    let mut returned = Vec::new();
    while offset + 4 <= packet.len() {
        let field_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let len_words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        let len = len_words.checked_mul(4).ok_or(Status::ErrInvalidArgs)?;
        if len < 4 || offset + len > packet.len() {
            return Err(Status::ErrInvalidArgs);
        }
        let value = &packet[offset + 4..offset + len];
        match field_type {
            UNIQUE_IDENTIFIER => saw_unique = value == unique,
            NTS_COOKIE if value.len() <= 1024 => returned.push(value.to_vec()),
            NTS_AUTHENTICATOR => {
                let expected = authenticator(&state.s2c_key, &packet[..offset], unique)?;
                saw_auth = value == expected;
            }
            _ => {}
        }
        offset += len;
    }
    if !saw_unique || !saw_auth {
        return Err(Status::ErrInvalidArgs);
    }
    for cookie in returned.into_iter().take(8) {
        state.cookies.push(cookie);
    }
    state.replay_window.push(unique.to_vec());
    if state.replay_window.len() > 16 {
        state.replay_window.remove(0);
    }
    Ok(sample)
}

pub fn quality_from_sample(sample: SntpSample, monotonic_ns: u64) -> TimeQuality {
    let mut quality = sntp::quality_from_sample(sample, monotonic_ns);
    quality.source = ClockSource::NtsSecure;
    quality
}

fn write_record(out: &mut Vec<u8>, record_type: u16, value: &[u8]) {
    out.extend_from_slice(&record_type.to_be_bytes());
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
}

fn write_ext(
    out: &mut [u8],
    offset: usize,
    field_type: u16,
    value: &[u8],
) -> Result<usize, Status> {
    let padded = value.len().saturating_add(4).next_multiple_of(4);
    let end = offset.checked_add(padded).ok_or(Status::ErrInvalidArgs)?;
    if end > out.len() || padded > u16::MAX as usize {
        return Err(Status::ErrInvalidArgs);
    }
    out[offset..end].fill(0);
    out[offset..offset + 2].copy_from_slice(&field_type.to_be_bytes());
    out[offset + 2..offset + 4].copy_from_slice(&((padded / 4) as u16).to_be_bytes());
    out[offset + 4..offset + 4 + value.len()].copy_from_slice(value);
    Ok(end)
}

fn unique_identifier() -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    let ticks = bexos_userspace::syscall::ticks();
    let freq = bexos_userspace::syscall::frequency();
    for word in [
        ticks,
        ticks.rotate_left(17) ^ freq,
        ticks.wrapping_mul(0x9e37_79b9_7f4a_7c15),
        freq.rotate_left(9) ^ 0x4e54_532d_5549_4431,
    ] {
        out.extend_from_slice(&word.to_be_bytes());
    }
    out
}

fn authenticator(key: &[u8], packet: &[u8], unique: &[u8]) -> Result<Vec<u8>, Status> {
    if key.len() != 32 {
        return Err(Status::ErrInvalidArgs);
    }
    let mut cipher = Aes128Siv::new(GenericArray::from_slice(key));
    let mut plaintext = [];
    let tag = cipher
        .encrypt_in_place_detached([packet, unique], &mut plaintext)
        .map_err(|_| Status::ErrInvalidArgs)?;
    Ok(tag.as_slice().to_vec())
}
