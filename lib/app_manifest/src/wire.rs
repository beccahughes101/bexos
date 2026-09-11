use crate::Error;
pub fn varint(bytes: &[u8]) -> Result<(u64, usize), Error> {
    let mut value = 0u64;
    for index in 0..10 {
        let byte = *bytes.get(index).ok_or(Error::Malformed)?;
        if index == 9 && byte > 1 {
            return Err(Error::Malformed);
        }
        value |= u64::from(byte & 127) << (index * 7);
        if byte & 128 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(Error::Malformed)
}
pub fn fields(mut bytes: &[u8]) -> impl Iterator<Item = Result<(u32, u8, &[u8]), Error>> {
    core::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        let result = (|| {
            let (key, len) = varint(bytes)?;
            bytes = &bytes[len..];
            if key >> 3 == 0 || key >> 3 > 0x1fff_ffff {
                return Err(Error::Malformed);
            }
            let kind = (key & 7) as u8;
            let size = match kind {
                0 => varint(bytes)?.1,
                1 => 8,
                2 => {
                    let (len, prefix) = varint(bytes)?;
                    bytes = &bytes[prefix..];
                    usize::try_from(len).map_err(|_| Error::Malformed)?
                }
                5 => 4,
                _ => return Err(Error::Malformed),
            };
            let value = bytes.get(..size).ok_or(Error::Malformed)?;
            bytes = &bytes[size..];
            Ok(((key >> 3) as u32, kind, value))
        })();
        if result.is_err() {
            bytes = &[];
        }
        Some(result)
    })
}
