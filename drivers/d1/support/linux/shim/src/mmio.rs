use crate::error::{LinuxError, Result};
use alloc::vec;
use alloc::vec::Vec;

pub trait Mmio {
    fn read32(&self, offset: usize) -> Result<u32>;
    fn write32(&mut self, offset: usize, value: u32) -> Result<()>;

    fn read64(&self, offset: usize) -> Result<u64> {
        let lo = self.read32(offset)? as u64;
        let hi = self.read32(offset + 4)? as u64;
        Ok(lo | (hi << 32))
    }

    fn write64(&mut self, offset: usize, value: u64) -> Result<()> {
        self.write32(offset, value as u32)?;
        self.write32(offset + 4, (value >> 32) as u32)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MockMmioBar {
    registers: Vec<u32>,
}

impl MockMmioBar {
    pub fn new(size_bytes: usize) -> Self {
        Self {
            registers: vec![0; size_bytes.div_ceil(4)],
        }
    }
}

impl Mmio for MockMmioBar {
    fn read32(&self, offset: usize) -> Result<u32> {
        self.registers
            .get(offset / 4)
            .copied()
            .ok_or(LinuxError::Invalid)
    }

    fn write32(&mut self, offset: usize, value: u32) -> Result<()> {
        let register = self
            .registers
            .get_mut(offset / 4)
            .ok_or(LinuxError::Invalid)?;
        *register = value;
        Ok(())
    }
}
