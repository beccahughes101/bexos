//! Direct-scanout eligibility uses committed geometry and explicit opaque
//! formats. An opaque fullscreen top item hides all lower scene content.
use crate::{Surface, resolved::Item};
pub fn candidate<'a>(
    items: impl Iterator<Item = &'a Item>,
    display: Surface,
    supported_formats: u32,
    overlays: bool,
) -> Option<Item> {
    if overlays {
        return None;
    }
    let top = *items.last()?;
    display.validate(u64::MAX).ok()?;
    top.surface.validate(u64::MAX).ok()?;
    let format = top.surface.format;
    if top.effects != Default::default()
        || top.buffer == 0
        || !format.is_opaque()
        || supported_formats & (1 << format as u32) == 0
        || top.opacity != 1.
        || top.transform.x != 0.
        || top.transform.y != 0.
        || top.transform.sx != 1.
        || top.transform.sy != 1.
        || top.inverse_scale != (1., 1.)
        || top.surface.width != display.width
        || top.surface.height != display.height
        || top.surface.stride != top.surface.width * 4
        || top.visible.x != 0.
        || top.visible.y != 0.
        || top.visible.width != display.width as f64
        || top.visible.height != display.height as f64
        || top.pixels != display.full()
    {
        return None;
    }
    Some(top)
}
