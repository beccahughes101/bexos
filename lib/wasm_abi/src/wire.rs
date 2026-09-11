//! Small protobuf wire codec; schema sources remain authoritative.
use crate::Error;
use alloc::vec::Vec;
pub struct Reader<'a>(pub &'a [u8]);
impl<'a> Reader<'a> {
    pub fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let bytes = self.0.get(..len).ok_or(Error::InvalidEncoding)?;
        self.0 = &self.0[len..];
        Ok(bytes)
    }
    pub fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let b = self.take(1)?[0];
            if shift == 63 && b > 1 {
                return Err(Error::InvalidEncoding);
            }
            value |= u64::from(b & 127) << shift;
            if b < 128 {
                return Ok(value);
            }
        }
        Err(Error::InvalidEncoding)
    }
    pub fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let len = usize::try_from(self.varint()?).map_err(|_| Error::InvalidEncoding)?;
        self.take(len)
    }
    pub fn field(&mut self) -> Result<Option<(u32, Value<'a>)>, Error> {
        if self.0.is_empty() {
            return Ok(None);
        }
        let key = self.varint()?;
        if key >> 3 == 0 || key >> 3 > 0x1fff_ffff {
            return Err(Error::InvalidEncoding);
        }
        let value = match key & 7 {
            0 => Value::Number(self.varint()?),
            1 => Value::Fixed(self.take(8)?),
            2 => Value::Bytes(self.bytes()?),
            5 => Value::Fixed(self.take(4)?),
            _ => return Err(Error::InvalidEncoding),
        };
        Ok(Some(((key >> 3) as u32, value)))
    }
}
pub enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Fixed(&'a [u8]),
}
impl<'a> Value<'a> {
    pub fn number(self) -> Result<u64, Error> {
        match self {
            Self::Number(v) => Ok(v),
            _ => Err(Error::InvalidEncoding),
        }
    }
    pub fn bytes(self) -> Result<&'a [u8], Error> {
        match self {
            Self::Bytes(v) => Ok(v),
            _ => Err(Error::InvalidEncoding),
        }
    }
    pub fn string(self) -> Result<alloc::string::String, Error> {
        Ok(core::str::from_utf8(self.bytes()?)
            .map_err(|_| Error::InvalidEncoding)?
            .into())
    }
}
pub fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 128 {
        out.push((v as u8 & 127) | 128);
        v >>= 7;
    }
    out.push(v as u8);
}
pub fn number(out: &mut Vec<u8>, key: u32, v: u64) {
    varint(out, u64::from(key) << 3);
    varint(out, v);
}
pub fn bytes(out: &mut Vec<u8>, key: u32, v: &[u8]) {
    varint(out, (u64::from(key) << 3) | 2);
    varint(out, v.len() as u64);
    out.extend_from_slice(v);
}
