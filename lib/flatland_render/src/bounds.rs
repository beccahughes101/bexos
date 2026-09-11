//! Validation at the shared renderer boundary, independent of a GPU device.
use bexos_flatland::{Damage, Error, Format, Surface, resolved::Item};
pub fn backdrop(item: &Item, width: u32, height: u32) -> Result<Damage, Error> {
    let r = item.visible;
    let t = item.transform;
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || [
            r.x,
            r.y,
            r.width,
            r.height,
            r.x + r.width,
            r.y + r.height,
            t.x,
            t.y,
            t.sx,
            t.sy,
        ]
        .iter()
        .any(|v| !v.is_finite())
        || r.width < 0.
        || r.height < 0.
        || t.sx <= 0.
        || t.sy <= 0.
        || !item.opacity.is_finite()
        || !(0. ..=1.).contains(&item.opacity)
    {
        return Err(Error::Bounds);
    }
    item.effects.validate()?;
    item.surface.validate(u64::MAX)?;
    Ok(r.pixels(Surface {
        width,
        height,
        stride: width * 4,
        format: Format::Rgba,
    }))
}
