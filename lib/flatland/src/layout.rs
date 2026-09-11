//! Logical layout properties, independent of the Taffy process-local cache.
use crate::Error;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Properties {
    /// 0=manual, 1=row, 2=column, 3=grid, 4=none, 5=reverse row, 6=reverse column.
    pub mode: u32,
    /// -1 means auto; nonnegative values are logical pixels.
    pub width: f32,
    pub height: f32,
    pub grow: f32,
    pub gap: f32,
    pub padding: f32,
    pub columns: u32,
}
impl Default for Properties {
    fn default() -> Self {
        Self {
            mode: 0,
            width: -1.,
            height: -1.,
            grow: 0.,
            gap: 0.,
            padding: 0.,
            columns: 1,
        }
    }
}
impl Properties {
    pub fn validate(self) -> Result<(), Error> {
        let dimension = |v: f32| v == -1. || (v.is_finite() && (0.0..=4096.0).contains(&v));
        if self.mode > 6
            || !dimension(self.width)
            || !dimension(self.height)
            || ![self.grow, self.gap, self.padding]
                .iter()
                .all(|v| v.is_finite() && (0.0..=4096.0).contains(v))
            || !(1..=64).contains(&self.columns)
        {
            return Err(Error::Bounds);
        }
        Ok(())
    }
}
