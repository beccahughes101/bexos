//! Rebuildable CPU backdrop scratch. Work includes sampling halos; only final
//! damage is copied out, so halo pixels never overwrite unaffected output.
use bexos_graphics::{Damage, Error, Surface, blur::BoxBlur};
pub struct Cache {
    pub pixels: Vec<u8>,
    pub blur: BoxBlur,
}
impl Cache {
    pub fn new(surface: Surface) -> Result<Self, Error> {
        Ok(Self {
            pixels: vec![0; surface.validate(u64::MAX)?],
            blur: BoxBlur::new(surface.width)?,
        })
    }
    pub fn copy_damage(&self, surface: Surface, output: &mut [u8], damage: Damage) {
        for y in damage.y..damage.y + damage.height {
            let start = (y * surface.stride + damage.x * 4) as usize;
            let end = start + damage.width as usize * 4;
            output[start..end].copy_from_slice(&self.pixels[start..end]);
        }
    }
}
