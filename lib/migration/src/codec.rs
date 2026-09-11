//! Canonical little-endian encoding, independent of allocator and enum layouts.
use crate::Error;
use alloc::vec::Vec;

pub fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

#[derive(Default)]
pub struct Encoder {
    bytes: Vec<u8>,
}
impl Encoder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn word(&mut self, word: u64) {
        self.bytes.extend_from_slice(&word.to_le_bytes());
    }
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.word(bytes.len() as u64);
        self.bytes.extend_from_slice(bytes);
    }
    pub fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
pub struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.offset.checked_add(len).ok_or(Error::InvalidData)?;
        let bytes = self.bytes.get(self.offset..end).ok_or(Error::InvalidData)?;
        self.offset = end;
        Ok(bytes)
    }
    pub fn word(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn count(&mut self, max: usize) -> Result<usize, Error> {
        let count = usize::try_from(self.word()?).map_err(|_| Error::Capacity)?;
        if count > max {
            return Err(Error::Capacity);
        }
        Ok(count)
    }
    pub fn bytes(&mut self, max: usize) -> Result<&'a [u8], Error> {
        let len = self.count(max)?;
        self.take(len)
    }
    pub fn text(&mut self, max: usize) -> Result<&'a str, Error> {
        core::str::from_utf8(self.bytes(max)?).map_err(|_| Error::InvalidData)
    }
    pub fn flag(&mut self) -> Result<bool, Error> {
        match self.word()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidData),
        }
    }
    pub fn finish(self) -> Result<(), Error> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::InvalidData)
        }
    }
}
