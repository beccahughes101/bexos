use super::*;
use crate::state_wire::{InvalidState, Reader, Result, State, Writer};
impl State for IoApic {
    const BYTES: usize = 194;
    fn save(&self, w: &mut Writer<'_>) -> Result<()> {
        w.u8(self.selected)?;
        w.u8(self.cpus)?;
        for value in self.low.iter().chain(&self.high) {
            w.u32(*value)?;
        }
        Ok(())
    }
    fn load(r: &mut Reader<'_>, _now: u64) -> Result<Self> {
        let selected = r.u8()?;
        let cpus = r.u8()?;
        if cpus > 32 {
            return Err(InvalidState);
        }
        let mut value = Self::new(cpus).ok_or(InvalidState)?;
        for part in 0..2 {
            for index in 0..24 {
                value.selected = 0x10 + 2 * index + part;
                if !value.write(0x10, r.u32()?) {
                    return Err(InvalidState);
                }
            }
        }
        value.selected = selected;
        Ok(value)
    }
}
impl IoApic {
    pub(crate) fn cpu_count(&self) -> usize {
        usize::from(self.cpus)
    }
}
