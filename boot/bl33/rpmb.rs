//! QEMU-only, polled PCI serial transport for opaque RPMB frames. Slot 7 is
//! reserved by tools/qemu. It never enables bus mastering or UART interrupts.
use bexos_trusty_boot::{ql::Error, storage::Rpmb};
use core::ptr::{read_volatile, write_volatile};
const CONFIG: usize = 0x3f03_8000;
const IO: usize = 0x3eff_1000;
pub struct Uart;
impl Uart {
    pub fn new() -> Result<Self, Error> {
        unsafe {
            if read_volatile(CONFIG as *const u32) != 0x0002_1b36 {
                return Err(Error::Transport);
            }
            write_volatile((CONFIG + 4) as *mut u16, 0);
            write_volatile((CONFIG + 0x10) as *mut u32, 0x1001);
            write_volatile((CONFIG + 4) as *mut u16, 1);
            for (offset, value) in [(1, 0), (3, 0x80), (0, 1), (1, 0), (3, 3), (2, 7), (4, 3)] {
                write_volatile((IO + offset) as *mut u8, value);
            }
        }
        Ok(Self)
    }
}
fn wait(mask: u8, start: u64) -> Result<(), Error> {
    loop {
        let now = super::trusty::now_ns();
        if now < start || now - start >= 30_000_000_000 {
            return Err(Error::Timeout);
        }
        let status = unsafe { read_volatile((IO + 5) as *const u8) };
        if status & 0x1e != 0 {
            return Err(Error::Transport);
        }
        if status & mask == mask {
            return Ok(());
        }
        core::hint::spin_loop();
    }
}
fn write(bytes: &[u8], start: u64) -> Result<(), Error> {
    for byte in bytes {
        wait(0x20, start)?;
        unsafe {
            write_volatile(IO as *mut u8, *byte);
        }
    }
    Ok(())
}
impl Rpmb for Uart {
    fn exchange(&mut self, request: &[u8], response: &mut [u8]) -> Result<(), Error> {
        if request.is_empty()
            || request.len() > 4096
            || request.len() % 512 != 0
            || response.is_empty()
            || response.len() > 4096
            || response.len() % 512 != 0
        {
            return Err(Error::InvalidResponse);
        }
        let start = super::trusty::now_ns();
        write(&((response.len() / 512) as u16).to_le_bytes(), start)?;
        write(&((request.len() / 512) as u16).to_le_bytes(), start)?;
        write(request, start)?;
        for byte in response {
            wait(1, start)?;
            *byte = unsafe { read_volatile(IO as *const u8) };
        }
        Ok(())
    }
}
pub fn release() -> Result<(), Error> {
    let start = super::trusty::now_ns();
    write(&0u16.to_le_bytes(), start)?;
    wait(0x40, start)?;
    unsafe {
        write_volatile((CONFIG + 4) as *mut u16, 0);
    }
    Ok(())
}
