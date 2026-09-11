use super::io;
use core::fmt;
pub const EARLY_BASE: usize = 0x3f8;
#[derive(Clone, Copy)]
pub struct EarlyUart {
    base: u16,
}
impl EarlyUart {
    pub const unsafe fn new(base: usize) -> Self {
        Self { base: base as u16 }
    }
    pub fn init(&mut self) {
        unsafe {
            io::out8(self.base + 1, 0);
            io::out8(self.base + 3, 0x80);
            io::out8(self.base, 1);
            io::out8(self.base + 1, 0);
            io::out8(self.base + 3, 3);
            io::out8(self.base + 2, 0xc7);
            io::out8(self.base + 4, 0x0b);
        }
    }
    pub fn write_byte_poll(&mut self, byte: u8) {
        unsafe {
            while io::in8(self.base + 5) & 0x20 == 0 {
                core::hint::spin_loop();
            }
            io::out8(self.base, byte);
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
