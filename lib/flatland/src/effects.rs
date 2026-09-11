//! Content effects shared by render backends and hit testing. Rounded bounds
//! are expressed in content coordinates; backdrop radius is in display pixels.
use crate::{Damage, Error, Surface};
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Effects {
    pub corner_radius: f32,
    pub backdrop_radius: u32,
}
impl Effects {
    pub fn validate(self) -> Result<(), Error> {
        if !self.corner_radius.is_finite()
            || !(0.0..=2048.0).contains(&self.corner_radius)
            || self.backdrop_radius > crate::blur::MAX_RADIUS
        {
            return Err(Error::Bounds);
        }
        Ok(())
    }
    pub fn contains(self, surface: Surface, x: f64, y: f64) -> bool {
        if x < 0. || y < 0. || x >= surface.width as f64 || y >= surface.height as f64 {
            return false;
        }
        let r = (self.corner_radius as f64)
            .min(surface.width as f64 * 0.5)
            .min(surface.height as f64 * 0.5);
        if r == 0. {
            return true;
        }
        let dx = (x - x.clamp(r, surface.width as f64 - r)).abs();
        let dy = (y - y.clamp(r, surface.height as f64 - r)).abs();
        dx * dx + dy * dy <= r * r
    }
}
pub fn expand(damage: Damage, radius: u32, surface: Surface) -> Damage {
    let x = damage.x.saturating_sub(radius);
    let y = damage.y.saturating_sub(radius);
    Damage {
        x,
        y,
        width: damage
            .x
            .saturating_add(damage.width)
            .saturating_add(radius)
            .min(surface.width)
            - x,
        height: damage
            .y
            .saturating_add(damage.height)
            .saturating_add(radius)
            .min(surface.height)
            - y,
    }
}
pub fn intersect(a: Damage, b: Damage) -> Option<Damage> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    (right > x && bottom > y).then_some(Damage {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    })
}
