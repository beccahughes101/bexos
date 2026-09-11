use super::*;
use crate::state_wire::{InvalidState, Reader, Result, State, Writer};
impl State for LocalApic {
    const BYTES: usize = 131;
    fn save(&self, w: &mut Writer<'_>) -> Result<()> {
        w.u8(self.id)?;
        w.u8(self.tpr)?;
        for value in [self.svr, self.ldr, self.dfr] {
            w.u32(value)?;
        }
        for value in self.lvt.iter().chain(&self.irr).chain(&self.isr) {
            w.u32(*value)?;
        }
        for value in [self.icr_high, self.icr_low, self.divide, self.initial] {
            w.u32(value)?;
        }
        w.optional(self.deadline_ns)
    }
    fn load(r: &mut Reader<'_>, now: u64) -> Result<Self> {
        let mut value = Self::new(r.u8()?);
        value.tpr = r.u8()?;
        for offset in [0xf0, 0xd0, 0xe0] {
            value
                .write(offset, r.u32()?, now)
                .map_err(|_| InvalidState)?;
        }
        for index in 0..7 {
            let offset = if index == 6 {
                0x2f0
            } else {
                0x320 + index * 16
            };
            value
                .write(offset, r.u32()?, now)
                .map_err(|_| InvalidState)?;
        }
        for word in value.irr.iter_mut().chain(&mut value.isr) {
            *word = r.u32()?;
        }
        value
            .write(0x310, r.u32()?, now)
            .map_err(|_| InvalidState)?;
        let icr = r.u32()?;
        if icr != 0 {
            value.write(0x300, icr, now).map_err(|_| InvalidState)?;
        }
        value
            .write(0x3e0, r.u32()?, now)
            .map_err(|_| InvalidState)?;
        value.initial = r.u32()?;
        value.deadline_ns = r.optional()?;
        if value.id >= 32
            || value.irr[0] & 0xffff != 0
            || value.isr[0] & 0xffff != 0
            || value.deadline_ns.is_some() && value.initial == 0
        {
            return Err(InvalidState);
        }
        Ok(value)
    }
}
impl LocalApic {
    pub(crate) fn identity(&self) -> usize {
        usize::from(self.id)
    }
    pub(crate) fn retains(&self, vector: u8) -> bool {
        vector >= 16 && self.irr[usize::from(vector / 32)] & (1 << (vector % 32)) != 0
    }
}
