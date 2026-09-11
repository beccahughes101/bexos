//! Compositor accessibility geometry and pixel operations, independent of IPC.
use crate::{
    Damage, Error, Format, Surface,
    resolved::{Rect, Transform},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayTransform {
    pub scale: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub filter: u32,
}
impl Default for DisplayTransform {
    fn default() -> Self {
        Self {
            scale: 1.,
            origin_x: 0.,
            origin_y: 0.,
            filter: 0,
        }
    }
}
impl DisplayTransform {
    pub fn validate(self) -> Result<(), Error> {
        if !self.scale.is_finite()
            || !(1. ..=8.).contains(&self.scale)
            || !self.origin_x.is_finite()
            || !self.origin_y.is_finite()
            || self.origin_x < 0.
            || self.origin_y < 0.
            || self.origin_x > 4096.
            || self.origin_y > 4096.
            || self.filter > 5
        {
            return Err(Error::Bounds);
        }
        Ok(())
    }
    pub fn local(self, x: f64, y: f64) -> (f64, f64) {
        (
            x / self.scale + self.origin_x,
            y / self.scale + self.origin_y,
        )
    }
    pub fn world(self, t: Transform) -> Transform {
        Transform {
            x: (t.x - self.origin_x) * self.scale,
            y: (t.y - self.origin_y) * self.scale,
            sx: t.sx * self.scale,
            sy: t.sy * self.scale,
        }
    }
    pub fn rect(self, r: Rect) -> Rect {
        Rect {
            x: (r.x - self.origin_x) * self.scale,
            y: (r.y - self.origin_y) * self.scale,
            width: r.width * self.scale,
            height: r.height * self.scale,
        }
    }
    /// Source and destination are distinct retained mappings, so magnification
    /// never samples pixels overwritten by an earlier destination write.
    pub fn apply(
        self,
        surface: Surface,
        source: &[u8],
        destination: &mut [u8],
        damage: Damage,
    ) -> Result<(), Error> {
        self.validate()?;
        surface.validate(source.len() as u64)?;
        surface.validate(destination.len() as u64)?;
        damage.validate(surface)?;
        for y in damage.y..damage.y + damage.height {
            let (_, sy) = self.local(0., y as f64);
            for x in damage.x..damage.x + damage.width {
                let (sx, _) = self.local(x as f64, 0.);
                let dst = (y * surface.stride + x * 4) as usize;
                let mut pixel = if sx < surface.width as f64 && sy < surface.height as f64 {
                    let src = (sy as u32 * surface.stride + sx as u32 * 4) as usize;
                    source[src..src + 4].try_into().unwrap()
                } else {
                    [0, 0, 0, 255]
                };
                filter(&mut pixel, self.filter, surface.format);
                destination[dst..dst + 4].copy_from_slice(&pixel);
            }
        }
        Ok(())
    }
}
fn filter(p: &mut [u8; 4], kind: u32, format: Format) {
    if format.is_opaque() {
        p[3] = 255;
    }
    if kind == 0 {
        return;
    }
    if format.is_bgra() {
        p.swap(0, 2);
    }
    let [r, g, b, a] = p.map(u32::from);
    let luma = (54 * r + 183 * g + 19 * b + 128) / 256;
    let rgb = match kind {
        1 => [luma; 3],
        2 => [
            a.saturating_sub(r),
            a.saturating_sub(g),
            a.saturating_sub(b),
        ],
        // Bounded linear approximations operating on premultiplied components.
        3 => [
            (145 * r + 111 * g) / 256,
            (143 * r + 113 * g) / 256,
            (62 * g + 194 * b) / 256,
        ],
        4 => [
            (160 * r + 96 * g) / 256,
            (179 * r + 77 * g) / 256,
            (77 * g + 179 * b) / 256,
        ],
        5 => [if luma * 2 >= a { a } else { 0 }; 3],
        _ => unreachable!(),
    };
    for i in 0..3 {
        p[i] = rgb[i].min(a) as u8;
    }
    if format.is_bgra() {
        p.swap(0, 2);
    }
}
pub fn valid_rect(r: Rect) -> bool {
    [r.x, r.y, r.width, r.height, r.x + r.width, r.y + r.height]
        .iter()
        .all(|v| v.is_finite())
        && r.width > 0.
        && r.height > 0.
        && r.width <= 4096.
        && r.height <= 4096.
        && r.x.abs() <= 4096.
        && r.y.abs() <= 4096.
}
/// Draw a border clipped to the committed view. Interior rows visit only two
/// border spans; the caller includes old and new bounds in its frame damage.
pub fn focus_ring(
    surface: Surface,
    pixels: &mut [u8],
    bounds: Rect,
    clip: Rect,
    width: u32,
    rgba: u32,
) -> Result<(), Error> {
    surface.validate(pixels.len() as u64)?;
    if ![
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        bounds.x + bounds.width,
        bounds.y + bounds.height,
    ]
    .iter()
    .all(|v| v.is_finite())
        || bounds.width <= 0.
        || bounds.height <= 0.
        || !(1..=16).contains(&width)
        || rgba & 255 != 255
    {
        return Err(Error::Bounds);
    }
    let outer = bounds.intersect(clip).pixels(surface);
    let inner = Rect {
        x: bounds.x + width as f64,
        y: bounds.y + width as f64,
        width: (bounds.width - 2. * width as f64).max(0.),
        height: (bounds.height - 2. * width as f64).max(0.),
    };
    let mut color = rgba.to_be_bytes();
    if surface.format.is_bgra() {
        color.swap(0, 2);
    }
    for y in outer.y..outer.y + outer.height {
        let end = outer.x + outer.width;
        let ceil = |v: f64| {
            let v = v.max(outer.x as f64).min(end as f64);
            let n = v as u32;
            n + u32::from(v > n as f64)
        };
        let spans = if inner.height > 0.
            && inner.width > 0.
            && y as f64 >= inner.y
            && (y as f64) < inner.y + inner.height
        {
            [(outer.x, ceil(inner.x)), (ceil(inner.x + inner.width), end)]
        } else {
            [(outer.x, end), (end, end)]
        };
        for (begin, end) in spans {
            for x in begin..end {
                if !inner.contains(x as f64, y as f64)
                    && bounds.contains(x as f64, y as f64)
                    && clip.contains(x as f64, y as f64)
                {
                    let i = (y * surface.stride + x * 4) as usize;
                    pixels[i..i + 4].copy_from_slice(&color);
                }
            }
        }
    }
    Ok(())
}
