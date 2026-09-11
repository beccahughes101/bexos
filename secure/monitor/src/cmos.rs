//! Per-domain RTC state. Reuse the RTC driver's pure calendar codec; all writes
//! apply to this register image, never to the host CMOS or NMI control port.
#[path = "cmos_state.rs"]
mod state;
use bexos_d1_cmos::{Cmos, Registers, RtcError};
#[derive(Clone, Copy)]
struct Raw([u8; 128]);
impl Registers for &mut Raw {
    fn read(&self, register: u8) -> Result<u8, RtcError> {
        self.0
            .get(usize::from(register))
            .copied()
            .ok_or(RtcError::Io)
    }
    fn write(&mut self, register: u8, value: u8) -> Result<(), RtcError> {
        *self.0.get_mut(usize::from(register)).ok_or(RtcError::Io)? = value;
        Ok(())
    }
}
pub struct Clock {
    raw: Raw,
    index: u8,
    utc: i64,
    epoch: u64,
    second: i64,
}
impl Clock {
    pub fn new(registers: [u8; 128], now: u64) -> Option<Self> {
        let mut raw = Raw(registers);
        let utc = Cmos::new(&mut raw).read_utc_ns().ok()?;
        raw.0[0x0a] = 0x26;
        raw.0[0x0b] &= 6;
        raw.0[0x0d] = 0x80;
        Some(Self {
            raw,
            index: 0,
            utc,
            epoch: now,
            second: utc / 1_000_000_000,
        })
    }
    fn refresh(&mut self, now: u64) -> Option<()> {
        if self.raw.0[0x0b] & 0x80 == 0 {
            let utc = self
                .utc
                .checked_add(i64::try_from(now.checked_sub(self.epoch)?).ok()?)?;
            if utc / 1_000_000_000 != self.second {
                Cmos::new(&mut self.raw).write_utc_ns(utc).ok()?;
                self.second = utc / 1_000_000_000;
            }
        }
        Some(())
    }
    pub fn read(&mut self, port: u16, now: u64) -> Option<u8> {
        self.refresh(now)?;
        match port {
            0x70 => Some(self.index),
            0x71 => match self.index & 127 {
                register @ (0 | 2 | 4 | 7 | 8 | 9 | 0x0a..=0x0d | 0x32) => {
                    Some(self.raw.0[usize::from(register)])
                }
                _ => None,
            },
            _ => None,
        }
    }
    pub fn write(&mut self, port: u16, value: u8, now: u64) -> bool {
        if self.refresh(now).is_none() {
            return false;
        }
        if port == 0x70 {
            self.index = value;
            return true;
        }
        if port != 0x71 {
            return false;
        }
        let register = usize::from(self.index & 127);
        if register == 0x0b {
            // Periodic/alarm interrupts are not exposed by this backend.
            if value & !0x86 != 0 {
                return false;
            }
            let old = self.raw;
            let converted = if (old.0[register] ^ value) & 6 != 0 {
                match Cmos::new(&mut self.raw).read_utc_ns() {
                    Ok(utc) => Some(utc),
                    Err(_) => return false,
                }
            } else {
                None
            };
            self.raw.0[register] = value;
            if let Some(utc) = converted {
                if Cmos::new(&mut self.raw).write_utc_ns(utc).is_err() {
                    self.raw = old;
                    return false;
                }
            }
            if old.0[register] & 0x80 != 0 && value & 0x80 == 0 {
                let Ok(utc) = Cmos::new(&mut self.raw).read_utc_ns() else {
                    self.raw = old;
                    return false;
                };
                self.utc = utc;
                self.epoch = now;
                self.second = utc / 1_000_000_000;
            }
            true
        } else if matches!(register, 0 | 2 | 4 | 7 | 8 | 9 | 0x32) && self.raw.0[0x0b] & 0x80 != 0 {
            self.raw.0[register] = value;
            true
        } else {
            false
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn clock() -> Clock {
        let mut raw = Raw([0; 128]);
        raw.0[0xb] = 2;
        raw.0[0xd] = 128;
        Cmos::new(&mut raw)
            .write_utc_ns(1_577_836_859_000_000_000)
            .unwrap();
        Clock::new(raw.0, 10).unwrap()
    }
    #[test]
    fn ticking_and_guest_set_are_private_and_keep_calendar_rollover() {
        let mut a = clock();
        let mut b = clock();
        assert!(a.write(0x70, 0x80, 10));
        assert_eq!(a.read(0x71, 10), Some(0x59));
        assert_eq!(a.read(0x71, 1_000_000_010), Some(0));
        assert!(a.write(0x70, 2, 1_000_000_010));
        assert_eq!(a.read(0x71, 1_000_000_010), Some(1));
        assert!(a.write(0x70, 0xb, 1_000_000_010));
        assert!(a.write(0x71, 0x82, 1_000_000_010));
        assert!(a.write(0x70, 0, 1_000_000_010));
        assert!(a.write(0x71, 0x30, 1_000_000_010));
        assert_eq!(a.read(0x71, 9_000_000_010), Some(0x30));
        assert!(a.write(0x70, 0xb, 9_000_000_010));
        assert!(a.write(0x71, 2, 9_000_000_010));
        assert!(a.write(0x70, 0, 10_000_000_010));
        assert_eq!(a.read(0x71, 10_000_000_010), Some(0x31));
        assert_eq!(b.read(0x71, 10), Some(0x59));
        assert!(a.write(0x70, 0xb, 10_000_000_010));
        assert!(!a.write(0x71, 0x42, 10_000_000_010));
    }
    #[test]
    fn format_changes_preserve_the_instant_without_waiting_for_a_tick() {
        let mut rtc = clock();
        assert!(rtc.write(0x70, 0xb, 10));
        assert!(rtc.write(0x71, 6, 10));
        assert!(rtc.write(0x70, 0, 10));
        assert_eq!(rtc.read(0x71, 10), Some(59));
        assert!(rtc.write(0x70, 0xb, 10));
        assert!(rtc.write(0x71, 0, 10));
        assert!(rtc.write(0x70, 4, 10));
        assert_eq!(rtc.read(0x71, 10), Some(0x12));
        assert!(rtc.write(0x70, 0, 10));
        assert_eq!(rtc.read(0x71, 10), Some(0x59));
    }
}
