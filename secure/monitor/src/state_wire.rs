//! Bounded, canonical primitives for protected device handoff records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidState;
pub type Result<T> = core::result::Result<T, InvalidState>;

pub(crate) struct Writer<'a> {
    bytes: &'a mut [u8],
    offset: usize,
}
impl<'a> Writer<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub fn bytes(&mut self, value: &[u8]) -> Result<()> {
        let end = self.offset.checked_add(value.len()).ok_or(InvalidState)?;
        self.bytes
            .get_mut(self.offset..end)
            .ok_or(InvalidState)?
            .copy_from_slice(value);
        self.offset = end;
        Ok(())
    }
    pub fn u8(&mut self, value: u8) -> Result<()> {
        self.bytes(&[value])
    }
    pub fn u32(&mut self, value: u32) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }
    pub fn u64(&mut self, value: u64) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }
    pub fn boolean(&mut self, value: bool) -> Result<()> {
        self.u8(u8::from(value))
    }
    pub fn optional(&mut self, value: Option<u64>) -> Result<()> {
        self.boolean(value.is_some())?;
        self.u64(value.unwrap_or(0))
    }
    pub fn finish(self) -> Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(InvalidState)
        }
    }
}
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.offset.checked_add(N).ok_or(InvalidState)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(InvalidState)?
            .try_into()
            .map_err(|_| InvalidState)?;
        self.offset = end;
        Ok(value)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes::<1>()?[0])
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }
    pub fn boolean(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(InvalidState),
        }
    }
    pub fn optional(&mut self) -> Result<Option<u64>> {
        let present = self.boolean()?;
        let value = self.u64()?;
        if !present && value != 0 {
            return Err(InvalidState);
        }
        Ok(present.then_some(value))
    }
    pub fn finish(self) -> Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(InvalidState)
        }
    }
}
pub(crate) trait State: Sized {
    const BYTES: usize;
    fn save(&self, writer: &mut Writer<'_>) -> Result<()>;
    fn load(reader: &mut Reader<'_>, now: u64) -> Result<Self>;
}
