use crate::Error;

pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn finish(&self) -> Result<(), Error> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::InvalidEncoding)
        }
    }

    pub fn byte(&mut self) -> Result<u8, Error> {
        let byte = *self.bytes.get(self.pos).ok_or(Error::InvalidEncoding)?;
        self.pos += 1;
        Ok(byte)
    }

    pub fn bytes4(&mut self) -> Result<[u8; 4], Error> {
        let bytes = self.take(4)?;
        Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    pub fn count(&mut self, limit: usize) -> Result<usize, Error> {
        let count = self.u32()? as usize;
        if count > limit {
            return Err(Error::LimitExceeded);
        }
        Ok(count)
    }

    pub fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32, Error> {
        let bytes = self.take(4)?;
        Ok(f32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(len).ok_or(Error::InvalidEncoding)?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or(Error::InvalidEncoding)?;
        self.pos = end;
        Ok(bytes)
    }
}

pub fn u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}
