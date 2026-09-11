//! Bounded VirtIO-GPU wire decoding shared by transport and host fixtures.
#![no_std]
extern crate alloc;
pub mod aperture;
pub mod discovery;
pub mod transport;
pub use discovery::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Capacity,
}
/// Validate a fenced response header before its payload becomes CPU-visible.
pub fn completion(header: &[u8], expected: u32, fence: u64) -> Result<(), Error> {
    completion_for_context(header, expected, fence, 0)
}
/// CPU control acknowledgement only. This must never satisfy a GPU timeline or
/// presentation fence; the command defines the acknowledged control operation.
pub fn control_acknowledgement(header: &[u8], expected: u32) -> Result<(), Error> {
    if header.len() != 24
        || word(header, 0)? != expected
        || header[4..].iter().any(|byte| *byte != 0)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub fn completion_for_context(
    header: &[u8],
    expected: u32,
    fence: u64,
    context: u32,
) -> Result<(), Error> {
    if header.len() != 24
        || word(header, 0)? != expected
        || word(header, 4)? & 1 == 0
        || u64::from_le_bytes(header[8..16].try_into().unwrap()) != fence
        || word(header, 16)? != context
        || header[20] != 0
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub fn word(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset.checked_add(4).ok_or(Error::Invalid)?)
            .ok_or(Error::Invalid)?
            .try_into()
            .unwrap(),
    ))
}
