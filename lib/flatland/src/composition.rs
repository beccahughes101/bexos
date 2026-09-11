//! Allocation-free composition of borrowed, retained surface mappings.
use crate::{Damage, Error, Surface, resolved::Snapshot};

pub fn composite<B: AsRef<[u8]>>(
    snapshot: &Snapshot,
    target: Surface,
    dst: &mut [u8],
    damage: Damage,
    buffer: impl FnMut(u64) -> Option<B>,
) -> Result<(), Error> {
    composite_items(snapshot.items.iter(), target, dst, damage, buffer)
}
pub fn composite_items<'a, B: AsRef<[u8]>>(
    items: impl Iterator<Item = &'a crate::resolved::Item>,
    target: Surface,
    dst: &mut [u8],
    damage: Damage,
    mut buffer: impl FnMut(u64) -> Option<B>,
) -> Result<(), Error> {
    target.validate(dst.len() as u64)?;
    damage.validate(target)?;
    for item in items {
        let x0 = item.pixels.x.max(damage.x);
        let y0 = item.pixels.y.max(damage.y);
        let x1 = (item.pixels.x + item.pixels.width).min(damage.x + damage.width);
        let y1 = (item.pixels.y + item.pixels.height).min(damage.y + damage.height);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let source = buffer(item.buffer).ok_or(Error::Invalid)?;
        let src = source.as_ref();
        item.surface.validate(src.len() as u64)?;
        let (inv_x, inv_y) = item.inverse_scale;
        let opacity = (item.opacity * 255. + 0.5) as u32;
        // Common window translations need no per-pixel floating point mapping.
        let unit = item.effects.corner_radius == 0.
            && item.transform.sx == 1.
            && item.transform.sy == 1.
            && item.transform.x == item.transform.x as i64 as f64
            && item.transform.y == item.transform.y as i64 as f64
            && item.visible.contains(x0 as f64, y0 as f64)
            && item.visible.contains((x1 - 1) as f64, (y1 - 1) as f64);
        if unit {
            let sx = (x0 as f64 - item.transform.x) as usize;
            let sy = (y0 as f64 - item.transform.y) as usize;
            let width = (x1 - x0) as usize * 4;
            for row in 0..(y1 - y0) as usize {
                let a = (sy + row) * item.surface.stride as usize + sx * 4;
                let b = (y0 as usize + row) * target.stride as usize + x0 as usize * 4;
                let input = &src[a..a + width];
                let output = &mut dst[b..b + width];
                if opacity == 255
                    && item.surface.format == target.format
                    && (item.surface.format.is_opaque()
                        || input.chunks_exact(4).all(|p| p[3] == 255))
                {
                    output.copy_from_slice(input);
                } else if item.surface.format == target.format && !target.format.is_opaque() {
                    crate::blend::row(input, output, opacity as u8);
                } else {
                    for (input, output) in input.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
                        blend(
                            input.try_into().unwrap(),
                            output,
                            opacity,
                            item.surface.format,
                            target.format,
                        );
                    }
                }
            }
            continue;
        }
        for y in y0..y1 {
            let sy = (y as f64 - item.transform.y) * inv_y;
            if sy < 0. || sy >= item.surface.height as f64 {
                continue;
            }
            for x in x0..x1 {
                if !item.contains(x as f64 + 0.5, y as f64 + 0.5) {
                    continue;
                }
                let sx = (x as f64 - item.transform.x) * inv_x;
                if sx < 0. || sx >= item.surface.width as f64 {
                    continue;
                }
                let a = (sy as u32 * item.surface.stride + sx as u32 * 4) as usize;
                let b = (y * target.stride + x * 4) as usize;
                blend(
                    src[a..a + 4].try_into().unwrap(),
                    &mut dst[b..b + 4],
                    opacity,
                    item.surface.format,
                    target.format,
                );
            }
        }
    }
    Ok(())
}
#[inline]
fn blend(
    pixel: [u8; 4],
    output: &mut [u8],
    opacity: u32,
    source: crate::Format,
    target: crate::Format,
) {
    // Normalize only source alpha before blending; the target's opaque alpha
    // does not make a translucent source opaque.
    let rgba_target = if target.is_bgra() {
        crate::Format::Bgra
    } else {
        crate::Format::Rgba
    };
    let pixel = source.convert(pixel, rgba_target);
    if target.is_opaque() {
        output[3] = 255;
    }
    if opacity == 255 && pixel[3] == 255 {
        output.copy_from_slice(&pixel);
        return;
    }
    let alpha = (pixel[3] as u32 * opacity + 127) / 255;
    for c in 0..4 {
        let value = (pixel[c] as u32 * opacity + 127) / 255
            + (output[c] as u32 * (255 - alpha) + 127) / 255;
        output[c] = value.min(255) as u8;
    }
}
