//! Bounded byte-stream chunks used by uploads and file-state adapters.
use crate::Error;
use alloc::vec::Vec;
pub const CHUNK: usize = 16 * 1024;
pub fn keys(stream: u64, bytes: &[u8]) -> impl Iterator<Item = u64> {
    let count = bytes.len().div_ceil(CHUNK);
    (0..count).map(move |i| (stream << 32) | i as u64)
}
pub fn record(bytes: Option<&[u8]>, key: u64) -> Option<Vec<u8>> {
    let bytes = bytes?;
    let start = (key as u32 as usize).checked_mul(CHUNK)?;
    if start >= bytes.len() {
        return None;
    }
    bytes
        .get(start..bytes.len().min(start + CHUNK))
        .map(|b| b.to_vec())
}
pub fn adopt(bytes: Option<&mut Vec<u8>>, key: u64, data: Option<&[u8]>) -> Result<(), Error> {
    let Some(data) = data else {
        return Ok(());
    };
    if data.len() > CHUNK {
        return Err(Error::Capacity);
    }
    let start = (key as u32 as usize)
        .checked_mul(CHUNK)
        .ok_or(Error::Capacity)?;
    bytes
        .ok_or(Error::InvalidData)?
        .get_mut(start..start + data.len())
        .ok_or(Error::InvalidData)?
        .copy_from_slice(data);
    Ok(())
}
