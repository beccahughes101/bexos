#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtcError {
    InvalidValue,
    Unstable,
    Io,
}
pub trait Registers {
    fn read(&self, register: u8) -> Result<u8, RtcError>;
    fn write(&mut self, register: u8, value: u8) -> Result<(), RtcError>;
}
pub struct Cmos<Io> {
    io: Io,
}
impl<Io: Registers> Cmos<Io> {
    pub const fn new(io: Io) -> Self {
        Self { io }
    }
    fn sample(&self) -> Result<[u8; 8], RtcError> {
        if self.io.read(0x0a)? & 0x80 != 0 {
            return Err(RtcError::Unstable);
        }
        let values = [
            self.io.read(0)?,
            self.io.read(2)?,
            self.io.read(4)?,
            self.io.read(7)?,
            self.io.read(8)?,
            self.io.read(9)?,
            self.io.read(0x32)?,
            self.io.read(0x0b)?,
        ];
        if self.io.read(0x0a)? & 0x80 != 0 {
            return Err(RtcError::Unstable);
        }
        Ok(values)
    }
    pub fn read_utc_ns(&self) -> Result<i64, RtcError> {
        for _ in 0..128 {
            let Ok(a) = self.sample() else {
                continue;
            };
            if self.sample().ok() != Some(a) {
                continue;
            }
            if self.io.read(0x0d)? & 0x80 == 0 {
                return Err(RtcError::InvalidValue);
            }
            let decode = |v| if a[7] & 4 != 0 { Ok(v as u32) } else { bcd(v) };
            let (second, minute, mut hour) = (decode(a[0])?, decode(a[1])?, decode(a[2] & 0x7f)?);
            if a[7] & 2 == 0 {
                if !(1..=12).contains(&hour) {
                    return Err(RtcError::InvalidValue);
                }
                hour = hour % 12 + if a[2] & 0x80 != 0 { 12 } else { 0 };
            }
            let (day, month, year) = (
                decode(a[3])?,
                decode(a[4])?,
                decode(a[6])? * 100 + decode(a[5])?,
            );
            if !(2020..=2262).contains(&year)
                || !(1..=12).contains(&month)
                || day == 0
                || day > month_days(year, month)
                || hour >= 24
                || minute >= 60
                || second >= 60
            {
                return Err(RtcError::InvalidValue);
            }
            let days: u32 = (1970..year)
                .map(|y| if leap(y) { 366 } else { 365 })
                .sum::<u32>()
                + (1..month).map(|m| month_days(year, m)).sum::<u32>()
                + day
                - 1;
            return (i64::from(days) * 86400 + i64::from(hour * 3600 + minute * 60 + second))
                .checked_mul(1_000_000_000)
                .ok_or(RtcError::InvalidValue);
        }
        Err(RtcError::Unstable)
    }
    pub fn write_utc_ns(&mut self, value: i64) -> Result<(), RtcError> {
        if value < 1_577_836_800_000_000_000 {
            return Err(RtcError::InvalidValue);
        }
        let seconds = value / 1_000_000_000;
        let mut days = (seconds / 86400) as u32;
        let mut year = 1970;
        while days >= if leap(year) { 366 } else { 365 } {
            days -= if leap(year) { 366 } else { 365 };
            year += 1;
        }
        let mut month = 1;
        while days >= month_days(year, month) {
            days -= month_days(year, month);
            month += 1;
        }
        let control = self.io.read(0x0b)?;
        let encode = |v: u32| {
            if control & 4 != 0 {
                v as u8
            } else {
                ((v / 10) * 16 + v % 10) as u8
            }
        };
        let hour = ((seconds / 3600) % 24) as u32;
        let hour = if control & 2 != 0 {
            encode(hour)
        } else {
            encode(if hour % 12 == 0 { 12 } else { hour % 12 }) | if hour >= 12 { 0x80 } else { 0 }
        };
        self.io.write(0x0b, control | 0x80)?;
        let result = (|| {
            for (register, value) in [
                (0, encode((seconds % 60) as u32)),
                (2, encode(((seconds / 60) % 60) as u32)),
                (4, hour),
                (7, encode(days + 1)),
                (8, encode(month)),
                (9, encode(year % 100)),
                (0x32, encode(year / 100)),
            ] {
                self.io.write(register, value)?;
            }
            Ok(())
        })();
        let restored = self.io.write(0x0b, control);
        result.and(restored)
    }
}
fn bcd(value: u8) -> Result<u32, RtcError> {
    if value & 15 > 9 || value >> 4 > 9 {
        Err(RtcError::InvalidValue)
    } else {
        Ok((value >> 4) as u32 * 10 + (value & 15) as u32)
    }
}
fn leap(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
fn month_days(year: u32, month: u32) -> u32 {
    match month {
        2 => {
            if leap(year) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake([u8; 128]);
    impl Registers for Fake {
        fn read(&self, r: u8) -> Result<u8, RtcError> {
            Ok(self.0[r as usize])
        }
        fn write(&mut self, r: u8, v: u8) -> Result<(), RtcError> {
            self.0[r as usize] = v;
            Ok(())
        }
    }
    #[test]
    fn roundtrip_bcd_binary_and_twelve_hour_modes() {
        for control in [0, 2, 4, 6] {
            let mut io = Fake([0; 128]);
            io.0[11] = control;
            io.0[13] = 128;
            let mut rtc = Cmos::new(io);
            for time in [
                1_577_836_800_000_000_000,
                1_709_251_199_000_000_000,
                1_709_294_400_000_000_000,
            ] {
                rtc.write_utc_ns(time).unwrap();
                assert_eq!(rtc.read_utc_ns(), Ok(time));
                assert_eq!(rtc.io.0[11], control);
            }
        }
    }
    #[test]
    fn rejects_updating_or_invalid_clock() {
        let mut rtc = Cmos::new(Fake([0; 128]));
        assert!(rtc.read_utc_ns().is_err());
        rtc.io.0[10] = 128;
        assert_eq!(rtc.read_utc_ns(), Err(RtcError::Unstable));
    }
}
