//! Monitor-only PCI configuration mechanism 1 for the fixed Q35 root bus.
//! Guest ECAM accesses are filtered separately and never call this API directly.
use core::arch::asm;
/// # Safety
/// The caller owns physical PCI configuration access and serializes it.
pub unsafe fn read(requester: u8, register: u8) -> u32 {
    let value: u32;
    unsafe {
        asm!("out dx, eax", in("dx") 0xcf8u16, in("eax") 0x80000000u32 | (u32::from(requester) << 8) | u32::from(register & !3), options(nomem, nostack));
        asm!("in eax, dx", in("dx") 0xcfcu16, out("eax") value, options(nomem, nostack));
    }
    value
}
/// # Safety
/// The caller owns and validates writes to the selected physical function.
pub unsafe fn write(requester: u8, register: u8, value: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") 0xcf8u16, in("eax") 0x80000000u32 | (u32::from(requester) << 8) | u32::from(register & !3), options(nomem, nostack));
        asm!("out dx, eax", in("dx") 0xcfcu16, in("eax") value, options(nomem, nostack));
    }
}
/// # Safety
/// This platform has no downstream bridges. Disable every root-bus master
/// before changing DMA ownership; reject bridges requiring a wider inventory.
pub unsafe fn quiesce_root_bus() -> bool {
    unsafe {
        for requester in 0..=255 {
            if read(requester, 0) as u16 == 0xffff {
                continue;
            }
            if read(requester, 8) >> 16 == 0x0604 {
                return false;
            }
            let command = read(requester, 4) as u16;
            let clear = if read(requester, 8) >> 24 == 6 { 4 } else { 7 };
            write(requester, 4, u32::from((command & !clear) | (1 << 10)));
        }
        true
    }
}
