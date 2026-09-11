//! Logical input policy; owners load configuration outside report processing.
use crate::{keymap::Keymap, virtio::Device};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
#[derive(Clone, Debug)]
pub struct Settings {
    pub keymap: Keymap,
    pub acceleration: f64,
    pub repeat_delay: u64,
    pub repeat_interval: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            keymap: Keymap::default(),
            acceleration: 0.25,
            repeat_delay: 500_000,
            repeat_interval: 33_333,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), Error> {
        if !self.acceleration.is_finite()
            || !(0. ..=4.).contains(&self.acceleration)
            || !(100_000..=2_000_000).contains(&self.repeat_delay)
            || !(5_000..=1_000_000).contains(&self.repeat_interval)
            || self
                .keymap
                .normal
                .iter()
                .chain(&self.keymap.shifted)
                .any(|c| char::from_u32(*c).is_none())
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    pub fn apply(&self, device: &mut Device) -> Result<(), Error> {
        self.validate()?;
        device.keymap = self.keymap.clone();
        device.acceleration = self.acceleration;
        device.repeat_delay = self.repeat_delay;
        device.repeat_interval = self.repeat_interval;
        Ok(())
    }
    pub fn encode(&self, w: &mut Encoder) {
        w.word(self.acceleration.to_bits());
        w.word(self.repeat_delay);
        w.word(self.repeat_interval);
        for c in self.keymap.normal.iter().chain(&self.keymap.shifted) {
            w.word(*c as u64);
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let mut out = Self::default();
        out.acceleration = f64::from_bits(r.word()?);
        out.repeat_delay = r.word()?;
        out.repeat_interval = r.word()?;
        for c in out.keymap.normal.iter_mut().chain(&mut out.keymap.shifted) {
            *c = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        }
        out.validate()?;
        Ok(out)
    }
}
