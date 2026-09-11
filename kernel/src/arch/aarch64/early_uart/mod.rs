use core::fmt;

pub const EARLY_BASE: usize = 0x0900_0000;

const DR: usize = 0x000;
const FR: usize = 0x018;
const IBRD: usize = 0x024;
const FBRD: usize = 0x028;
const LCRH: usize = 0x02c;
const CR: usize = 0x030;
const IMSC: usize = 0x038;
const ICR: usize = 0x044;

const FR_TXFF: u32 = 1 << 5;

#[derive(Clone, Copy)]
pub struct EarlyUart {
    base: usize,
}

impl EarlyUart {
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    pub fn init(&mut self) {
        self.write_reg(CR, 0);
        self.write_reg(ICR, 0x7ff);
        self.write_reg(IBRD, 1);
        self.write_reg(FBRD, 40);
        self.write_reg(LCRH, 0b11 << 5);
        self.write_reg(IMSC, 0);
        self.write_reg(CR, (1 << 0) | (1 << 8) | (1 << 9));
    }

    pub fn write_byte_poll(&mut self, byte: u8) {
        while self.read_reg(FR) & FR_TXFF != 0 {}
        self.write_reg(DR, byte as u32);
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + offset) as *mut u32, value);
        }
    }
}

impl fmt::Write for EarlyUart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte_poll(b'\r');
            }
            self.write_byte_poll(byte);
        }
        Ok(())
    }
}
