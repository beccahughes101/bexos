//! Strict, bounded decoding of the protoc-compiled desktop configuration.
use bexos_graphics::Error;
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopConfig<'a> {
    pub text: &'a str,
    pub font_size: u32,
    pub color: [u8; 4],
    pub insets: [u32; 4],
    pub theme_css: &'a str,
    pub refresh_hz: u32,
    pub profile_gpu: bool,
    pub legacy_standalone: bool,
    pub software_vulkan_timeout_ms: u32,
}
pub(crate) fn varint(bytes: &mut &[u8]) -> Result<u64, Error> {
    let mut result = 0;
    for shift in (0..70).step_by(7) {
        let (&b, rest) = bytes.split_first().ok_or(Error::Invalid)?;
        *bytes = rest;
        if shift == 63 && b > 1 {
            return Err(Error::Invalid);
        }
        result |= u64::from(b & 127) << shift;
        if b < 128 {
            return Ok(result);
        }
    }
    Err(Error::Invalid)
}
impl<'a> DesktopConfig<'a> {
    pub fn frame_period_us(&self) -> u64 {
        if self.refresh_hz == 60 { 16_667 } else { 8_333 }
    }

    pub fn decode(mut bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > 8192 {
            return Err(Error::Bounds);
        }
        let mut out = Self {
            text: "",
            font_size: 0,
            color: [0; 4],
            insets: [0; 4],
            theme_css: "",
            refresh_hz: 120,
            profile_gpu: false,
            legacy_standalone: false,
            software_vulkan_timeout_ms: 2000,
        };
        let mut seen = 0u16;
        while !bytes.is_empty() {
            let tag = varint(&mut bytes)?;
            let field = tag >> 3;
            if !(1..=12).contains(&field) || seen & (1 << field) != 0 {
                return Err(Error::Invalid);
            }
            seen |= 1 << field;
            match (field, tag & 7) {
                (12, 0) => {
                    out.legacy_standalone = match varint(&mut bytes)? {
                        0 => false,
                        1 => true,
                        _ => return Err(Error::Invalid),
                    };
                }
                (11, 0) => {
                    out.software_vulkan_timeout_ms = match varint(&mut bytes)? {
                        0 => 2000,
                        value @ 2000..=30000 => value as u32,
                        _ => return Err(Error::Bounds),
                    };
                }
                (10, 0) => {
                    out.profile_gpu = match varint(&mut bytes)? {
                        0 => false,
                        1 => true,
                        _ => return Err(Error::Invalid),
                    };
                }
                (1 | 8, 2) => {
                    let size = usize::try_from(varint(&mut bytes)?).map_err(|_| Error::Bounds)?;
                    let text = bytes.get(..size).ok_or(Error::Bounds)?;
                    let text = core::str::from_utf8(text).map_err(|_| Error::Invalid)?;
                    if field == 1 {
                        out.text = text;
                    } else {
                        out.theme_css = text;
                    }
                    bytes = &bytes[size..];
                }
                (9, 0) => {
                    out.refresh_hz = match varint(&mut bytes)? {
                        0 | 120 => 120,
                        60 => 60,
                        _ => return Err(Error::Bounds),
                    };
                }
                (2, 0) => {
                    out.font_size = varint(&mut bytes)?.try_into().map_err(|_| Error::Bounds)?
                }
                (3, 5) => {
                    let rgba = u32::from_le_bytes(
                        bytes.get(..4).ok_or(Error::Bounds)?.try_into().unwrap(),
                    );
                    out.color = rgba.to_be_bytes();
                    bytes = &bytes[4..];
                }
                (4..=7, 0) => {
                    out.insets[field as usize - 4] =
                        varint(&mut bytes)?.try_into().map_err(|_| Error::Bounds)?
                }
                _ => return Err(Error::Invalid),
            }
        }
        if out.text.is_empty()
            || out.text.len() > 4096
            || out.theme_css.len() > 4096
            || !(8..=96).contains(&out.font_size)
            || out.color[3] == 0
            || out.insets.iter().any(|v| *v > 256)
        {
            return Err(Error::Bounds);
        }
        Ok(out)
    }
}
