use crate::Error;
/// Premultiplied, eight bits per component. Padding bytes are never pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Format {
    Bgra = 1,
    Rgba = 2,
    /// Opaque BGR; the fourth byte has no alpha meaning.
    Bgrx = 3,
    /// Opaque RGB; the fourth byte has no alpha meaning.
    Rgbx = 4,
}
impl Format {
    pub fn is_bgra(self) -> bool {
        matches!(self, Self::Bgra | Self::Bgrx)
    }
    pub fn is_opaque(self) -> bool {
        matches!(self, Self::Bgrx | Self::Rgbx)
    }
    /// Preserve premultiplied color values; discarded alpha composites over black.
    pub fn convert(self, mut pixel: [u8; 4], target: Self) -> [u8; 4] {
        if self.is_bgra() != target.is_bgra() {
            pixel.swap(0, 2);
        }
        if self.is_opaque() || target.is_opaque() {
            pixel[3] = 255;
        }
        pixel
    }
}
impl TryFrom<u32> for Format {
    type Error = Error;
    fn try_from(v: u32) -> Result<Self, Error> {
        match v {
            1 => Ok(Self::Bgra),
            2 => Ok(Self::Rgba),
            3 => Ok(Self::Bgrx),
            4 => Ok(Self::Rgbx),
            _ => Err(Error::Invalid),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: Format,
}
impl Surface {
    pub fn validate(self, len: u64) -> Result<usize, Error> {
        if self.width == 0
            || self.height == 0
            || self.width > 4096
            || self.height > 4096
            || self.stride % 4 != 0
            || self.stride < self.width.checked_mul(4).ok_or(Error::Bounds)?
        {
            return Err(Error::Invalid);
        }
        let bytes = u64::from(self.stride)
            .checked_mul(u64::from(self.height))
            .ok_or(Error::Bounds)?;
        if bytes > len || bytes > 64 * 1024 * 1024 {
            return Err(Error::Bounds);
        }
        usize::try_from(bytes).map_err(|_| Error::Bounds)
    }
    pub fn full(self) -> Damage {
        Damage {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }
    pub fn copy_rgba(self, src: &[u8], dst: &mut [u8], damage: Damage) -> Result<(), Error> {
        self.validate(dst.len() as u64)?;
        let tight = Self {
            stride: self.width * 4,
            format: Format::Rgba,
            ..self
        };
        tight.validate(src.len() as u64)?;
        damage.validate(self)?;
        for y in damage.y..damage.y + damage.height {
            for x in damage.x..damage.x + damage.width {
                let a = ((y * self.width + x) * 4) as usize;
                let b = (y * self.stride + x * 4) as usize;
                let p = &src[a..a + 4];
                let v = Format::Rgba.convert(p.try_into().unwrap(), self.format);
                dst[b..b + 4].copy_from_slice(&v);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Damage {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Damage {
    pub fn validate(self, s: Surface) -> Result<(), Error> {
        if self.width == 0
            || self.height == 0
            || self.x.checked_add(self.width).ok_or(Error::Bounds)? > s.width
            || self.y.checked_add(self.height).ok_or(Error::Bounds)? > s.height
        {
            return Err(Error::Bounds);
        }
        Ok(())
    }
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: self
                .x
                .saturating_add(self.width)
                .max(other.x.saturating_add(other.width))
                .saturating_sub(x),
            height: self
                .y
                .saturating_add(self.height)
                .max(other.y.saturating_add(other.height))
                .saturating_sub(y),
        }
    }
}
