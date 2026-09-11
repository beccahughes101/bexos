use core::arch::asm;
pub unsafe fn out8(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
pub unsafe fn out16(port: u16, value: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}
pub unsafe fn in8(port: u16) -> u8 {
    let value;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}
pub unsafe fn rdmsr(index: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") index, out("eax") low, out("edx") high, options(nomem, nostack));
    }
    low as u64 | ((high as u64) << 32)
}
pub unsafe fn wrmsr(index: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack));
    }
}
pub unsafe fn read32(address: u64) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32) }
}
pub unsafe fn write32(address: u64, value: u32) {
    unsafe {
        core::ptr::write_volatile(address as *mut u32, value);
    }
}
pub unsafe fn read64(address: u64) -> u64 {
    unsafe { core::ptr::read_volatile(address as *const u64) }
}
pub unsafe fn write64(address: u64, value: u64) {
    unsafe {
        core::ptr::write_volatile(address as *mut u64, value);
    }
}
