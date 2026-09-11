use super::*;
use crate::state_wire::{InvalidState, Reader, Result, State, Writer};
impl State for Hpet {
    const BYTES: usize = 34;
    fn save(&self, w: &mut Writer<'_>) -> Result<()> {
        w.boolean(self.enabled)?;
        for value in [
            self.counter,
            self.epoch_ns,
            self.timer_config,
            self.comparator,
        ] {
            w.u64(value)?;
        }
        w.boolean(self.armed)
    }
    fn load(r: &mut Reader<'_>, now: u64) -> Result<Self> {
        let value = Self {
            enabled: r.boolean()?,
            counter: r.u64()?,
            epoch_ns: r.u64()?,
            timer_config: r.u64()?,
            comparator: r.u64()?,
            armed: r.boolean()?,
        };
        let mut check = Self::new();
        if value.epoch_ns > now || !check.write(0x100, value.timer_config, now) {
            return Err(InvalidState);
        }
        Ok(value)
    }
}
