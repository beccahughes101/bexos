use super::*;
use crate::state_wire::{InvalidState, Reader, State, Writer};
impl State for Clock {
    const BYTES: usize = 153;
    fn save(&self, w: &mut Writer<'_>) -> Result<(), InvalidState> {
        w.bytes(&self.raw.0)?;
        w.u8(self.index)?;
        w.u64(self.utc as u64)?;
        w.u64(self.epoch)?;
        w.u64(self.second as u64)
    }
    fn load(r: &mut Reader<'_>, now: u64) -> Result<Self, InvalidState> {
        let value = Self {
            raw: Raw(r.bytes()?),
            index: r.u8()?,
            utc: r.u64()? as i64,
            epoch: r.u64()?,
            second: r.u64()? as i64,
        };
        if value.epoch > now
            || value.raw.0[0xa] != 0x26
            || value.raw.0[0xb] & !0x86 != 0
            || value.raw.0[0xd] != 0x80
        {
            return Err(InvalidState);
        }
        // During SET, partially written calendar fields may be invalid. Keep
        // the exact register image; normal validation occurs when SET clears.
        Ok(value)
    }
}
impl Clock {
    pub const STATE_BYTES: usize = <Self as State>::BYTES;
    /// Nested state; the outer record binds domain and private clock epoch.
    pub fn snapshot(&self, output: &mut [u8]) -> Result<(), InvalidState> {
        if output.len() != Self::STATE_BYTES {
            return Err(InvalidState);
        }
        let mut w = Writer::new(output);
        self.save(&mut w)?;
        w.finish()
    }
    pub fn restore_protected(input: &[u8], captured: u64) -> Result<Self, InvalidState> {
        let mut r = Reader::new(input);
        let value = Self::load(&mut r, captured)?;
        r.finish()?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rtc_keeps_elapsed_time_and_incomplete_guest_calendar_update() {
        let mut raw = Raw([0; 128]);
        raw.0[0xb] = 2;
        raw.0[0xd] = 128;
        Cmos::new(&mut raw)
            .write_utc_ns(1_577_836_859_000_000_000)
            .unwrap();
        let mut clock = Clock::new(raw.0, 10).unwrap();
        let mut record = [0; Clock::STATE_BYTES];
        clock.snapshot(&mut record).unwrap();
        let mut restored = Clock::restore_protected(&record, 10).unwrap();
        assert_eq!(restored.read(0x71, 1_000_000_010), Some(0));
        clock.write(0x70, 0xb, 10);
        clock.write(0x71, 0x82, 10);
        clock.write(0x70, 7, 10);
        clock.write(0x71, 0, 10); // Invalid day while SET is asserted.
        clock.snapshot(&mut record).unwrap();
        let mut restored = Clock::restore_protected(&record, 10).unwrap();
        assert_eq!(restored.read(0x71, 10_000_000_010), Some(0));
        restored.write(0x70, 0xb, 10_000_000_010);
        assert!(!restored.write(0x71, 2, 10_000_000_010));
        restored.write(0x70, 7, 10_000_000_010);
        restored.write(0x71, 1, 10_000_000_010);
        restored.write(0x70, 0xb, 10_000_000_010);
        assert!(restored.write(0x71, 2, 10_000_000_010));
    }
}
