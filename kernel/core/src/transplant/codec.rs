//! Canonical little-endian snapshot words. No Rust pointers, padding, or enum layouts.
use super::TransplantError;

pub type Result<T> = core::result::Result<T, TransplantError>;

pub struct Writer<'a> {
    bytes: &'a mut [u8],
    pos: usize,
}
impl<'a> Writer<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    pub fn len(&self) -> usize {
        self.pos
    }
    pub fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let end = self
            .pos
            .checked_add(bytes.len())
            .ok_or(TransplantError::PayloadTooLarge)?;
        self.bytes
            .get_mut(self.pos..end)
            .ok_or(TransplantError::PayloadTooLarge)?
            .copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }
    pub fn word(&mut self, word: u64) -> Result<()> {
        self.bytes(&word.to_le_bytes())
    }
    pub fn text(&mut self, text: &str) -> Result<()> {
        self.word(text.len() as u64)?;
        self.bytes(text.as_bytes())
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    pub fn finished(&self) -> bool {
        self.pos == self.bytes.len()
    }
    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(TransplantError::LengthMismatch)?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or(TransplantError::LengthMismatch)?;
        self.pos = end;
        Ok(bytes)
    }
    pub fn word(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    pub fn count(&mut self, max: usize) -> Result<usize> {
        let value =
            usize::try_from(self.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
        if value > max || value > self.bytes.len() - self.pos {
            return Err(TransplantError::InvalidRuntimeSnapshot);
        }
        Ok(value)
    }
    pub fn index(&mut self) -> Result<usize> {
        usize::try_from(self.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)
    }
    pub fn flag(&mut self) -> Result<bool> {
        match self.word()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(TransplantError::InvalidRuntimeSnapshot),
        }
    }
    pub fn text(&mut self, max: usize) -> Result<&'a str> {
        let len = self.count(max)?;
        core::str::from_utf8(self.bytes(len)?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)
    }
}
