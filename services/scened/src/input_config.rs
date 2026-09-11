//! Decode Bazel/protoc output once at startup; no config parsing on input paths.
use crate::desktop_config::varint;
use bexos_flatland_input::settings::Settings;
use bexos_graphics::Error;
pub fn decode(mut bytes: &[u8]) -> Result<Settings, Error> {
    if bytes.len() > 8192 {
        return Err(Error::Bounds);
    }
    let mut out = Settings::default();
    // Absent proto3 acceleration is its zero value. Repeat bounds are required.
    out.acceleration = 0.;
    out.repeat_delay = 0;
    out.repeat_interval = 0;
    let mut seen = 0u8;
    let mut keys = [false; 128];
    while !bytes.is_empty() {
        let tag = varint(&mut bytes)?;
        match tag {
            8 | 16 | 24 => {
                let field = (tag >> 3) as u8;
                if seen & (1 << field) != 0 {
                    return Err(Error::Invalid);
                }
                seen |= 1 << field;
                let value = varint(&mut bytes)?;
                match field {
                    1 => {
                        if value > 4000 {
                            return Err(Error::Bounds);
                        }
                        out.acceleration = value as f64 / 1000.;
                    }
                    2 => out.repeat_delay = value,
                    3 => out.repeat_interval = value,
                    _ => unreachable!(),
                }
            }
            34 => {
                let size = usize::try_from(varint(&mut bytes)?).map_err(|_| Error::Bounds)?;
                let mut key = bytes.get(..size).ok_or(Error::Bounds)?;
                bytes = &bytes[size..];
                let mut values = [0u32; 3];
                let mut fields = 0u8;
                while !key.is_empty() {
                    let tag = varint(&mut key)?;
                    if !matches!(tag, 8 | 16 | 24) {
                        return Err(Error::Invalid);
                    }
                    let index = (tag / 8 - 1) as usize;
                    if fields & (1 << index) != 0 {
                        return Err(Error::Invalid);
                    }
                    fields |= 1 << index;
                    values[index] = varint(&mut key)?.try_into().map_err(|_| Error::Bounds)?;
                }
                let [code, normal, shifted] = values;
                if code == 0
                    || code >= 128
                    || keys[code as usize]
                    || char::from_u32(normal).is_none()
                    || char::from_u32(shifted).is_none()
                {
                    return Err(Error::Invalid);
                }
                keys[code as usize] = true;
                out.keymap.normal[code as usize] = normal;
                out.keymap.shifted[code as usize] = shifted;
            }
            _ => return Err(Error::Invalid),
        }
    }
    out.validate().map_err(|_| Error::Bounds)?;
    Ok(out)
}
