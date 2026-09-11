#![no_std]

pub const PL031_BASE: u64 = 0x0901_0000;
pub const PL031_SIZE: u64 = 4096;
pub const UTC_2020_SECONDS: u32 = 1_577_836_800;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtcError {
    InvalidValue,
    OutOfRange,
}

pub trait RegisterIo {
    fn read32(&self, offset: usize) -> u32;
    fn write32(&mut self, offset: usize, value: u32);
}

pub struct Pl031<Io> {
    io: Io,
}

impl<Io: RegisterIo> Pl031<Io> {
    pub const fn new(io: Io) -> Self {
        Self { io }
    }

    pub fn read_utc_ns(&self) -> Result<i64, RtcError> {
        let seconds = self.io.read32(0x000);
        if seconds < UTC_2020_SECONDS {
            return Err(RtcError::InvalidValue);
        }
        Ok(i64::from(seconds) * 1_000_000_000)
    }

    pub fn write_utc_ns(&mut self, utc_ns: i64) -> Result<(), RtcError> {
        if utc_ns < i64::from(UTC_2020_SECONDS) * 1_000_000_000 {
            return Err(RtcError::InvalidValue);
        }
        let seconds = utc_ns / 1_000_000_000;
        let seconds = u32::try_from(seconds).map_err(|_| RtcError::OutOfRange)?;
        self.io.write32(0x008, seconds);
        Ok(())
    }

    pub fn into_inner(self) -> Io {
        self.io
    }
}

pub struct Mmio {
    base: usize,
}

impl Mmio {
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }
}

impl RegisterIo for Mmio {
    fn read32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    fn write32(&mut self, offset: usize, value: u32) {
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }
}
