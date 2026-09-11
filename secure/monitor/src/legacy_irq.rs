//! Virtual 8259/PIT devices used by pinned x86 Trusty. Physical PIC/PIT state
//! belongs to the monitor and is never modified by guest port I/O.
#[path = "legacy_state.rs"]
mod state;
#[derive(Clone)]
struct Pic {
    base: u8,
    mask: u8,
    pending: u8,
    in_service: u8,
    init: u8,
    icw4: bool,
    read_isr: bool,
    auto_eoi: bool,
}
impl Pic {
    const fn new(base: u8) -> Self {
        Self {
            base,
            mask: 0xff,
            pending: 0,
            in_service: 0,
            init: 0,
            icw4: false,
            read_isr: false,
            auto_eoi: false,
        }
    }
    fn write(&mut self, data: bool, value: u8) -> bool {
        if data {
            match self.init {
                1 => {
                    self.base = value & 0xf8;
                    self.init = 2;
                }
                2 => self.init = if self.icw4 { 3 } else { 0 },
                3 => {
                    if value & !0xf != 0 || value & 1 == 0 {
                        return false;
                    }
                    self.auto_eoi = value & 2 != 0;
                    self.init = 0;
                }
                _ => self.mask = value,
            }
        } else if value & 0x10 != 0 {
            if value & !0x11 != 0 {
                return false;
            }
            self.init = 1;
            self.icw4 = value & 1 != 0;
            self.mask = 0;
            self.pending = 0;
            self.in_service = 0;
        } else if value & 0x18 == 8 {
            if value & !0x0b != 0 {
                return false;
            }
            if value & 2 != 0 {
                self.read_isr = value & 1 != 0;
            }
        } else if value & 0x20 != 0 {
            if value & !0x67 != 0 {
                return false;
            }
            let irq = if value & 0x40 != 0 {
                value & 7
            } else {
                self.in_service.trailing_zeros() as u8
            };
            if irq < 8 {
                self.in_service &= !(1 << irq);
            }
        } else {
            return false;
        }
        true
    }
    fn irq(&self) -> Option<u8> {
        let irq = (self.pending & !self.mask).trailing_zeros() as u8;
        (irq < 8 && irq < self.in_service.trailing_zeros() as u8).then_some(irq)
    }
}
#[derive(Clone)]
pub struct LegacyIrq {
    master: Pic,
    slave: Pic,
    access: u8,
    mode: u8,
    low: Option<u8>,
    count: u32,
    started: u64,
    deadline: Option<u64>,
    latched: Option<u16>,
    status_latch: Option<u8>,
    null_count: bool,
    read_high: bool,
}
impl Default for LegacyIrq {
    fn default() -> Self {
        Self::new()
    }
}
impl LegacyIrq {
    pub const fn new() -> Self {
        Self {
            master: Pic::new(8),
            slave: Pic::new(0x70),
            access: 3,
            mode: 0,
            low: None,
            count: 0,
            started: 0,
            deadline: None,
            latched: None,
            status_latch: None,
            null_count: true,
            read_high: false,
        }
    }
    fn remaining(&self, now: u64) -> u16 {
        if self.count == 0 {
            return 0;
        }
        let ticks = (now.saturating_sub(self.started) as u128 * 1_193_182 / 1_000_000_000) as u64;
        (if self.mode == 2 || self.mode == 3 {
            self.count - (ticks % u64::from(self.count)) as u32
        } else {
            self.count
                .saturating_sub(ticks.min(u64::from(u32::MAX)) as u32)
        }) as u16
    }
    fn interval(&self) -> u64 {
        // Software strobe (mode 4) raises OUT one input cycle after terminal
        // count; mode 0 raises it at terminal count.
        ((u64::from(self.count) + u64::from(self.mode == 4)) * 1_000_000_000).div_ceil(1_193_182)
    }
    pub fn tick(&mut self, now: u64) {
        if !self.deadline.is_some_and(|deadline| now >= deadline) {
            return;
        }
        self.master.pending |= 1;
        let interval = self.interval();
        self.deadline = if matches!(self.mode, 2 | 3) && interval != 0 {
            let elapsed = now.saturating_sub(self.started);
            self.started
                .checked_add(elapsed / interval * interval)
                .and_then(|base| base.checked_add(interval))
        } else {
            None
        };
    }
    pub fn pending(&self) -> Option<u8> {
        self.master.irq().map(|irq| self.master.base + irq)
    }
    pub fn delivered(&mut self, vector: u8) {
        let Some(irq) = vector.checked_sub(self.master.base).filter(|irq| *irq < 8) else {
            return;
        };
        self.master.pending &= !(1 << irq);
        if !self.master.auto_eoi {
            self.master.in_service |= 1 << irq;
        }
    }
    pub fn read(&mut self, port: u16, now: u64) -> Option<u8> {
        if matches!(port, 0x20 | 0x21 | 0xa0 | 0xa1) {
            let pic = if port < 0xa0 {
                &self.master
            } else {
                &self.slave
            };
            return Some(if port & 1 != 0 {
                pic.mask
            } else if pic.read_isr {
                pic.in_service
            } else {
                pic.pending
            });
        }
        if port != 0x40 {
            return None;
        }
        if let Some(status) = self.status_latch.take() {
            return Some(status);
        }
        let value = self.latched.unwrap_or_else(|| self.remaining(now));
        let high = self.access == 2 || self.access == 3 && self.read_high;
        if self.access == 3 {
            self.read_high = !self.read_high;
        }
        if self.access != 3 || !self.read_high {
            self.latched = None;
        }
        Some(if high {
            (value >> 8) as u8
        } else {
            value as u8
        })
    }
    pub fn write(&mut self, port: u16, value: u8, now: u64) -> bool {
        if matches!(port, 0x20 | 0x21 | 0xa0 | 0xa1) {
            let pic = if port < 0xa0 {
                &mut self.master
            } else {
                &mut self.slave
            };
            return pic.write(port & 1 != 0, value);
        }
        if port == 0x43 {
            if value & 0xc0 == 0xc0 {
                if value & 0x0d != 0 {
                    return false;
                } // Only counter 0 is implemented.
                if value & 2 == 0 {
                    return true;
                }
                if value & 0x20 == 0 && self.latched.is_none() {
                    self.latched = Some(self.remaining(now));
                    self.read_high = false;
                }
                if value & 0x10 == 0 && self.status_latch.is_none() {
                    let output = !self.null_count && (self.mode != 0 || self.remaining(now) == 0);
                    self.status_latch = Some(
                        (u8::from(output) << 7)
                            | (u8::from(self.null_count) << 6)
                            | (self.access << 4)
                            | (self.mode << 1),
                    );
                }
                return true;
            }
            if value >> 6 != 0 || value & 1 != 0 {
                return false;
            } // Counter 0, binary.
            let access = (value >> 4) & 3;
            if access == 0 {
                if self.latched.is_none() {
                    self.latched = Some(self.remaining(now));
                    self.read_high = false;
                }
                return true;
            }
            let mode = (value >> 1) & 7;
            if !matches!(mode, 0 | 2 | 3 | 4 | 6 | 7) {
                return false;
            }
            self.access = access;
            self.mode = if mode >= 6 { mode - 4 } else { mode };
            self.low = None;
            self.read_high = false;
            self.latched = None;
            self.status_latch = None;
            self.null_count = true;
            self.deadline = None;
            return true;
        }
        if port != 0x40 {
            return false;
        }
        let reload = match self.access {
            1 => u16::from(value),
            2 => u16::from(value) << 8,
            _ => {
                if let Some(low) = self.low.take() {
                    u16::from(low) | (u16::from(value) << 8)
                } else {
                    self.low = Some(value);
                    return true;
                }
            }
        };
        self.count = if reload == 0 {
            65536
        } else {
            u32::from(reload)
        };
        self.null_count = false;
        self.started = now;
        self.deadline = now.checked_add(self.interval());
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readback_status_tracks_terminal_count_and_stop_cancels_irq() {
        let mut device = LegacyIrq::new();
        for (port, value) in [(0x43, 0x30), (0x40, 0xa9), (0x40, 4)] {
            assert!(device.write(port, value, 0));
        }
        assert!(device.write(0x43, 0xe2, 0));
        assert_eq!(device.read(0x40, 2_000_000), Some(0x30));
        assert!(device.write(0x43, 0xe2, 2_000_000));
        assert_eq!(device.read(0x40, 2_000_000), Some(0xb0));
        assert!(device.write(0x43, 0x38, 2_000_000));
        device.tick(1_000_000_000);
        assert_eq!(device.master.pending, 0);
    }
    #[test]
    fn timer_obeys_masks_latches_eoi_and_reload() {
        let mut device = LegacyIrq::new();
        for (port, value) in [
            (0x20, 0x11),
            (0x21, 0x20),
            (0x21, 4),
            (0x21, 5),
            (0x21, 0xff),
            (0x43, 0x30),
            (0x40, 0xa9),
            (0x40, 4),
        ] {
            assert!(device.write(port, value, 0));
        }
        device.tick(2_000_000);
        assert_eq!(device.pending(), None);
        assert!(device.write(0x21, 0xfe, 0));
        assert_eq!(device.pending(), Some(32));
        device.delivered(32);
        assert_eq!(device.pending(), None);
        assert!(device.write(0x43, 0x36, 0));
        assert!(device.write(0x40, 0, 0));
        assert!(device.write(0x40, 0, 0));
        assert!(device.write(0x43, 0, 0));
        assert_eq!(device.read(0x40, 0), Some(0));
        assert_eq!(device.read(0x40, 1_000_000), Some(0));
        device.tick(1_000_000_000);
        assert_eq!(device.pending(), None);
        assert!(device.write(0x20, 0x20, 0));
        assert_eq!(device.pending(), Some(32));
    }
}
