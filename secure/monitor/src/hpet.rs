//! Virtual HPET. Guest counter/configuration changes cannot alter the private
//! monitor clock used for scheduling and replacement deadlines.
#[path = "hpet_state.rs"]
mod state;
#[derive(Clone)]
pub struct Hpet {
    enabled: bool,
    counter: u64,
    epoch_ns: u64,
    timer_config: u64,
    comparator: u64,
    armed: bool,
}
impl Default for Hpet {
    fn default() -> Self {
        Self::new()
    }
}
impl Hpet {
    pub const fn new() -> Self {
        Self {
            enabled: false,
            counter: 0,
            epoch_ns: 0,
            timer_config: 0,
            comparator: 0,
            armed: false,
        }
    }
    fn counter(&self, now: u64) -> u64 {
        self.counter.wrapping_add(if self.enabled {
            now.saturating_sub(self.epoch_ns) / 10
        } else {
            0
        })
    }
    pub fn read(&self, offset: u16, now: u64) -> Option<u64> {
        Some(match offset {
            0 => (10_000_000u64 << 32) | (0x8086 << 16) | (1 << 13) | 1,
            0x10 => u64::from(self.enabled),
            0x20 => 0,
            0xf0 => self.counter(now),
            // One 64-bit edge-triggered timer, routable to GSI 16.
            0x100 => (1 << 48) | (1 << 5) | self.timer_config,
            0x108 => self.comparator,
            _ => return None,
        })
    }
    pub fn write(&mut self, offset: u16, value: u64, now: u64) -> bool {
        match offset {
            0x10 if value & !1 == 0 => {
                self.counter = self.counter(now);
                self.epoch_ns = now;
                self.enabled = value != 0;
            }
            0x20 if value <= 1 => {} // Edge-triggered interrupt has no level latch.
            0xf0 => {
                self.counter = value;
                self.epoch_ns = now;
            }
            0x100 if value & !(4 | (31 << 9)) == 0 => {
                let route = (value >> 9) & 31;
                if value & 4 != 0 && route != 16 {
                    return false;
                }
                self.timer_config = value;
            }
            0x108 => {
                self.comparator = value;
                self.armed = true;
            }
            _ => return false,
        }
        true
    }
    pub fn tick(&mut self, now: u64) -> Option<u8> {
        if self.enabled
            && self.armed
            && self.timer_config & 4 != 0
            && self.counter(now) >= self.comparator
        {
            self.armed = false;
            Some(16)
        } else {
            None
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_clock_reset_freeze_and_comparator_do_not_change_owner_time() {
        let mut hpet = Hpet::new();
        assert!(hpet.write(0x10, 1, 100));
        assert_eq!(hpet.read(0xf0, 1100), Some(100));
        assert!(hpet.write(0x10, 0, 1100));
        assert_eq!(hpet.read(0xf0, 9999), Some(100));
        assert!(hpet.write(0xf0, 0, 9999));
        assert!(hpet.write(0x10, 1, 10000));
        assert!(hpet.write(0x100, (16 << 9) | 4, 10000));
        assert!(hpet.write(0x108, 100, 10000));
        assert_eq!(hpet.tick(10999), None);
        assert_eq!(hpet.tick(11000), Some(16));
        assert_eq!(hpet.tick(12000), None);
        assert!(!hpet.write(0x100, 4, 12000));
        assert!(!hpet.write(0x10, 3, 12000));
    }
}
