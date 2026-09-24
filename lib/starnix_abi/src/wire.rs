use crate::Error;
use alloc::vec::Vec;

pub struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let value = self.0.get(..len).ok_or(Error::InvalidEncoding)?;
        self.0 = &self.0[len..];
        Ok(value)
    }

    fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for shift in (0..70).step_by(7) {
            let byte = self.take(1)?[0];
            if shift == 63 && byte > 1 {
                return Err(Error::InvalidEncoding);
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Ok(value);
            }
        }
        Err(Error::InvalidEncoding)
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
            2 => {
                let len = usize::try_from(self.varint()?).map_err(|_| Error::InvalidEncoding)?;
                Value::Bytes(self.take(len)?)
            }
            _ => return Err(Error::InvalidEncoding),
        };
        Ok(Some(((key >> 3) as u32, value)))
    }
}

pub enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
}

impl<'a> Value<'a> {
    pub fn number(self) -> Result<u64, Error> {
        match self {
            Self::Number(value) => Ok(value),
            _ => Err(Error::InvalidEncoding),
        }
    }

    pub fn bytes(self) -> Result<&'a [u8], Error> {
        match self {
            Self::Bytes(value) => Ok(value),
            _ => Err(Error::InvalidEncoding),
        }
    }

    pub fn string(self) -> Result<alloc::string::String, Error> {
        core::str::from_utf8(self.bytes()?)
            .map(Into::into)
            .map_err(|_| Error::InvalidEncoding)
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub fn number(out: &mut Vec<u8>, field: u32, value: u64) {
    varint(out, u64::from(field) << 3);
    varint(out, value);
}

pub fn bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    varint(out, (u64::from(field) << 3) | 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}
