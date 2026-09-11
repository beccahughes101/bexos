use crate::Error;
use alloc::string::String;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Progress {
    pub stage: u8,
    pub percent: u8,
    pub message: String,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            stage: 1,
            percent: 0,
            message: String::new(),
        }
    }
}
impl Progress {
    pub fn report(&mut self, stage: u8, percent: u8, message: &str) -> Result<bool, Error> {
        if !(1..=5).contains(&stage) || percent > 100 || message.len() > 64 {
            return Err(Error::Invalid);
        }
        if stage < self.stage || percent < self.percent {
            return Ok(false);
        }
        let changed = self.stage != stage || self.percent != percent || self.message != message;
        self.stage = stage;
        self.percent = percent;
        self.message = message.into();
        Ok(changed)
    }
}
/// Deadlines advance directly past missed frames rather than replaying them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameClock {
    pub next_us: u64,
}
impl FrameClock {
    pub const PERIOD_US: u64 = 16_667;
    pub fn due(&mut self, now: u64) -> bool {
        self.due_with_period(now, Self::PERIOD_US)
    }
    pub fn due_with_period(&mut self, now: u64, period_us: u64) -> bool {
        if period_us == 0 {
            return false;
        }
        if now < self.next_us {
            return false;
        }
        let base = if self.next_us == 0 { now } else { self.next_us };
        let skipped = now.saturating_sub(base) / period_us;
        self.next_us = base.saturating_add(skipped.saturating_add(1).saturating_mul(period_us));
        true
    }
}
