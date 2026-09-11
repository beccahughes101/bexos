//! Scrub reclaimed normal RAM before returning it to the frame allocator.
use core::arch::asm;

/// The caller owns the entire writable, identity-mapped normal-RAM range.
pub(super) unsafe fn normal_ram(base: u64, bytes: u64) {
    let dczid: u64;
    unsafe {
        asm!("mrs {value}, dczid_el0", value = out(reg) dczid, options(nomem, nostack, preserves_flags));
    }
    let block = 4u64 << (dczid & 15);
    if dczid & 16 == 0 && block <= 4096 && base.is_multiple_of(block) && bytes.is_multiple_of(block)
    {
        for offset in (0..bytes).step_by(block as usize) {
            unsafe {
                asm!("dc zva, {address}", address = in(reg) (base + offset), options(nostack, preserves_flags));
            }
        }
        unsafe {
            asm!("dsb ishst", options(nostack, preserves_flags));
        }
    } else {
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, bytes as usize);
        }
    }
}
