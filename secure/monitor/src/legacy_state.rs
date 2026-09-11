use super::*;
use crate::state_wire::{InvalidState, Reader, Result, State, Writer};
impl LegacyIrq {
    pub(crate) fn retains(&self, vector: u8) -> bool {
        vector
            .checked_sub(self.master.base)
            .filter(|irq| *irq < 8)
            .is_some_and(|irq| self.master.pending & (1 << irq) != 0)
    }
}
impl State for Pic {
    const BYTES: usize = 9;
    fn save(&self, w: &mut Writer<'_>) -> Result<()> {
        w.bytes(&[
            self.base,
            self.mask,
            self.pending,
            self.in_service,
            self.init,
        ])?;
        for value in [self.icw4, self.read_isr, self.auto_eoi] {
            w.boolean(value)?;
        }
        w.u8(0)
    }
    fn load(r: &mut Reader<'_>, _now: u64) -> Result<Self> {
        let value = Self {
            base: r.u8()?,
            mask: r.u8()?,
            pending: r.u8()?,
            in_service: r.u8()?,
            init: r.u8()?,
            icw4: r.boolean()?,
            read_isr: r.boolean()?,
            auto_eoi: r.boolean()?,
        };
        if r.u8()? != 0 || value.base & 7 != 0 || value.init > 3 || value.init == 3 && !value.icw4 {
            return Err(InvalidState);
        }
        Ok(value)
    }
}
impl State for LegacyIrq {
    const BYTES: usize = 70;
    fn save(&self, w: &mut Writer<'_>) -> Result<()> {
        self.master.save(w)?;
        self.slave.save(w)?;
        w.u8(self.access)?;
        w.u8(self.mode)?;
        w.optional(self.low.map(u64::from))?;
        w.u32(self.count)?;
        w.u64(self.started)?;
        w.optional(self.deadline)?;
        w.optional(self.latched.map(u64::from))?;
        w.optional(self.status_latch.map(u64::from))?;
        w.boolean(self.null_count)?;
        w.boolean(self.read_high)
    }
    fn load(r: &mut Reader<'_>, now: u64) -> Result<Self> {
        let value = Self {
            master: Pic::load(r, now)?,
            slave: Pic::load(r, now)?,
            access: r.u8()?,
            mode: r.u8()?,
            low: r
                .optional()?
                .map(u8::try_from)
                .transpose()
                .map_err(|_| InvalidState)?,
            count: r.u32()?,
            started: r.u64()?,
            deadline: r.optional()?,
            latched: r
                .optional()?
                .map(u16::try_from)
                .transpose()
                .map_err(|_| InvalidState)?,
            status_latch: r
                .optional()?
                .map(u8::try_from)
                .transpose()
                .map_err(|_| InvalidState)?,
            null_count: r.boolean()?,
            read_high: r.boolean()?,
        };
        if !(1..=3).contains(&value.access)
            || !matches!(value.mode, 0 | 2 | 3 | 4)
            || value.count > 65536
            || value.started > now
            || value.deadline.is_some() && (value.count == 0 || value.null_count)
            || value.low.is_some() && value.access != 3
            || value.status_latch.is_some_and(|status| {
                status & 1 != 0 || !matches!((status >> 1) & 7, 0 | 2 | 3 | 4) || status & 0x30 == 0
            })
        {
            return Err(InvalidState);
        }
        Ok(value)
    }
}
