#[cfg(all(bexos_guest, target_arch = "aarch64"))]
use core::arch::asm;
pub fn fidl(
    protocol: u64,
    ordinal: u64,
    request: &[u8],
    handles: &[u64],
    response: &mut [u8],
    out_handles: &mut [u64],
) -> Result<(usize, usize), i32> {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let mut status: u64 = protocol;
        let mut out_bytes_len: u64 = ordinal;
        let mut out_handles_len: u64 = request.as_ptr() as u64;
        asm!(
            "svc #1",
            inout("x0") status,
            inout("x1") out_bytes_len,
            inout("x2") out_handles_len,
            in("x3") request.len() as u64,
            in("x4") handles.as_ptr() as u64,
            in("x5") handles.len() as u64,
            in("x6") response.as_mut_ptr() as u64,
            in("x7") response.len() as u64,
            in("x8") out_handles.as_mut_ptr() as u64,
            in("x9") out_handles.len() as u64,
            options(nostack)
        );
        let err = status as i32;
        if err == 0 {
            Ok((out_bytes_len as usize, out_handles_len as usize))
        } else {
            Err(err)
        }
    }
    #[cfg(not(all(bexos_guest, target_arch = "aarch64")))]
    {
        let _ = (protocol, ordinal, request, handles, response, out_handles);
        Err(-1)
    }
}
pub fn yield_now() {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        asm!("svc #2", options(nostack));
    }
}
pub fn commit_transplant() {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        asm!("svc #5", options(nostack));
    }
}
pub fn log(s: &str) {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        asm!(
            "svc #3",
            in("x0") s.as_ptr() as u64,
            in("x1") s.len() as u64,
            options(nostack)
        );
    }
    #[cfg(not(all(bexos_guest, target_arch = "aarch64")))]
    let _ = s;
}
pub fn exit() -> ! {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        asm!("svc #4", options(nostack));
    }
    loop {
        yield_now();
    }
}
pub fn ticks() -> u64 {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let value;
        asm!("mrs {v}, cntvct_el0", v = out(reg) value, options(nomem, nostack));
        value
    }
    #[cfg(not(all(bexos_guest, target_arch = "aarch64")))]
    0
}
pub fn frequency() -> u64 {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let value;
        asm!("mrs {v}, cntfrq_el0", v = out(reg) value, options(nomem, nostack));
        value
    }
    #[cfg(not(all(bexos_guest, target_arch = "aarch64")))]
    1
}
pub fn heap_vmar() -> u64 {
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let value;
        asm!("svc #6", lateout("x0") value, options(nostack));
        value
    }
    #[cfg(not(all(bexos_guest, target_arch = "aarch64")))]
    0
}

pub fn pci_ecam_base() -> u64 {
    0x3f00_0000
}
pub fn pci_segment() -> u16 {
    0
}
pub fn pci_bus_range() -> (u8, u8) {
    (0, 15)
}
pub fn pci_mmio_window() -> (u64, u64) {
    (0x1000_0000, 0x3eff_0000)
}

pub fn cmos(_register: u8, _value: Option<u8>) -> Result<u8, i32> {
    Err(-1)
}

pub fn thread_pointer() -> u64 {
    let value;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) value, options(nomem, nostack));
    }
    value
}

pub fn console_frame(bytes: &[u8]) -> Result<(), i32> {
    let mut status = bytes.as_ptr() as u64;
    unsafe {
        asm!("svc #12", inout("x0") status, in("x1") bytes.len(), options(nostack));
    }
    if status == 0 {
        Ok(())
    } else {
        Err(status as i32)
    }
}
