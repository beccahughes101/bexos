#![no_std]
extern crate alloc;

use core::fmt;

pub const PL011_UART0_BASE: usize = 0x0900_0000;

pub const DR: usize = 0x000;
pub const FR: usize = 0x018;
pub const IBRD: usize = 0x024;
pub const FBRD: usize = 0x028;
pub const LCRH: usize = 0x02c;
pub const CR: usize = 0x030;
pub const IMSC: usize = 0x038;
pub const ICR: usize = 0x044;

pub const FR_TXFF: u32 = 1 << 5;
pub const FR_RXFE: u32 = 1 << 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UartError {
    MmioOutOfRange,
}

pub trait UartMmio {
    fn read32(&self, offset: usize) -> Result<u32, UartError>;
    fn write32(&mut self, offset: usize, value: u32) -> Result<(), UartError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pl011Mmio {
    base: usize,
}

impl Pl011Mmio {
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    fn ptr<T>(&self, offset: usize) -> *mut T {
        (self.base + offset) as *mut T
    }
}

impl UartMmio for Pl011Mmio {
    fn read32(&self, offset: usize) -> Result<u32, UartError> {
        Ok(unsafe { core::ptr::read_volatile(self.ptr::<u32>(offset).cast_const()) })
    }

    fn write32(&mut self, offset: usize, value: u32) -> Result<(), UartError> {
        unsafe {
            core::ptr::write_volatile(self.ptr::<u32>(offset), value);
        }
        Ok(())
    }
}

pub struct Pl011Uart<M> {
    mmio: M,
}

impl<M: UartMmio> Pl011Uart<M> {
    pub fn new(mmio: M) -> Self {
        Self { mmio }
    }

    pub fn init(&mut self) -> Result<(), UartError> {
        self.mmio.write32(CR, 0)?;
        self.mmio.write32(ICR, 0x7ff)?;
        self.mmio.write32(IBRD, 1)?;
        self.mmio.write32(FBRD, 40)?;
        self.mmio.write32(LCRH, 0b11 << 5)?;
        self.mmio.write32(IMSC, 0)?;
        self.mmio.write32(CR, (1 << 0) | (1 << 8) | (1 << 9))
    }

    pub fn write_byte_poll(&mut self, byte: u8) -> Result<(), UartError> {
        while self.mmio.read32(FR)? & FR_TXFF != 0 {}
        self.mmio.write32(DR, byte as u32)
    }

    pub fn try_read_byte(&mut self) -> Result<Option<u8>, UartError> {
        if self.mmio.read32(FR)? & FR_RXFE != 0 {
            return Ok(None);
        }
        Ok(Some((self.mmio.read32(DR)? & 0xff) as u8))
    }

    pub fn read_byte_poll(&mut self) -> Result<u8, UartError> {
        loop {
            if let Some(byte) = self.try_read_byte()? {
                return Ok(byte);
            }
        }
    }

    pub fn write_str_poll(&mut self, value: &str) -> Result<(), UartError> {
        for byte in value.bytes() {
            if byte == b'\n' {
                self.write_byte_poll(b'\r')?;
            }
            self.write_byte_poll(byte)?;
        }
        Ok(())
    }

    pub fn into_mmio(self) -> M {
        self.mmio
    }
}

impl<M: UartMmio> fmt::Write for Pl011Uart<M> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_str_poll(s).map_err(|_| fmt::Error)
    }
}
