use core::arch::asm;
/// INT 0x80 enters a DPL3 interrupt gate with a per-CPU TSS kernel stack.
/// The ten logical FIDL arguments map to rdi,rsi,rdx,r10,r8,r9,r12-r15.
pub fn fidl(
    protocol: u64,
    ordinal: u64,
    request: &[u8],
    handles: &[u64],
    response: &mut [u8],
    out_handles: &mut [u64],
) -> Result<(usize, usize), i32> {
    let mut status = protocol;
    let mut bytes = ordinal;
    let mut count = request.as_ptr() as u64;
    unsafe {
        asm!("int 0x80", in("rax") 1u64, inout("rdi") status, inout("rsi") bytes, inout("rdx") count,
        in("r10") request.len(), in("r8") handles.as_ptr(), in("r9") handles.len(), in("r12") response.as_mut_ptr(), in("r13") response.len(), in("r14") out_handles.as_mut_ptr(), in("r15") out_handles.len(), options(nostack));
    }
    if status as i32 == 0 {
        Ok((bytes as usize, count as usize))
    } else {
        Err(status as i32)
    }
}
fn call(number: u64, mut value: u64, argument: u64) -> u64 {
    unsafe {
        asm!("int 0x80", in("rax") number, inout("rdi") value, in("rsi") argument, options(nostack));
    }
    value
}
pub fn yield_now() {
    call(2, 0, 0);
}
pub fn commit_transplant() {
    call(5, 0, 0);
}
pub fn log(s: &str) {
    call(3, s.as_ptr() as u64, s.len() as u64);
}
pub fn exit() -> ! {
    call(4, 0, 0);
    loop {
        yield_now();
    }
}
pub fn ticks() -> u64 {
    call(7, 0, 0)
}
pub fn frequency() -> u64 {
    1_000_000_000
}
pub fn heap_vmar() -> u64 {
    call(6, 0, 0)
}
pub fn set_thread_pointer(value: u64) {
    assert_eq!(call(8, value, 0), 0);
}
pub fn thread_pointer() -> u64 {
    call(9, 0, 0)
}

pub fn pci_ecam_base() -> u64 {
    call(10, 0, 0)
}
pub fn pci_segment() -> u16 {
    call(13, 0, 0) as u16
}
pub fn pci_bus_range() -> (u8, u8) {
    let value = call(14, 0, 0);
    (value as u8, (value >> 8) as u8)
}
pub fn pci_mmio_window() -> (u64, u64) {
    (call(15, 0, 0), call(16, 0, 0))
}

pub fn cmos(register: u8, value: Option<u8>) -> Result<u8, i32> {
    let result = call(11, register as u64, value.map_or(0, |v| 256 | v as u64));
    if result <= 255 {
        Ok(result as u8)
    } else {
        Err(result as i32)
    }
}

pub fn console_frame(bytes: &[u8]) -> Result<(), i32> {
    let status = call(12, bytes.as_ptr() as u64, bytes.len() as u64);
    if status == 0 {
        Ok(())
    } else {
        Err(status as i32)
    }
}
