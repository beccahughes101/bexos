//! Each domain has a virtual UART configuration and line buffer. Only the
//! monitor writes physical COM1; one guest cannot reconfigure another's UART.
use core::arch::asm;
pub struct Console {
    uart: bexos_secure_monitor::uart::Uart,
    trusty: bool,
}
impl Console {
    pub fn snapshot(
        &self,
        output: &mut [u8],
    ) -> Result<(), bexos_secure_monitor::state_wire::InvalidState> {
        self.uart.snapshot(output)
    }
    pub fn restore_protected(
        &mut self,
        input: &[u8],
    ) -> Result<(), bexos_secure_monitor::state_wire::InvalidState> {
        self.uart.restore_protected(input)
    }
    pub const fn new(trusty: bool) -> Self {
        Self {
            uart: bexos_secure_monitor::uart::Uart::new(),
            trusty,
        }
    }
    pub fn read(&self, offset: u16) -> u8 {
        self.uart.read(offset).expect("validated UART offset")
    }
    pub fn write(&mut self, offset: u16, byte: u8) {
        if let Some(line) = self
            .uart
            .write(offset, byte)
            .expect("validated UART offset")
        {
            crate::log(if self.trusty { "[trusty] " } else { "[bexos] " });
            for value in line {
                unsafe {
                    asm!("out dx, al", in("dx") 0x3f8u16, in("al") *value, options(nomem, nostack));
                }
            }
        }
    }
}
