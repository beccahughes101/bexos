use crate::{c_int, c_void};
pub(crate) unsafe fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    unsafe {
        core::arch::asm!("rep movsb", inout("rdi") dst => _, inout("rsi") src => _, inout("rcx") n => _, options(nostack, preserves_flags));
    }
    dst
}
pub(crate) unsafe fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    if n == 0 {
        return dst;
    }
    if (dst as usize) <= src as usize || (dst as usize).wrapping_sub(src as usize) >= n {
        return unsafe { memcpy(dst, src, n) };
    }
    // Trap entry clears DF for the kernel and restores the saved user flags.
    // Restore the ABI-required clear direction flag before returning to Rust.
    unsafe {
        core::arch::asm!("std", "rep movsb", "cld",
            inout("rdi") dst.cast::<u8>().add(n - 1) => _,
            inout("rsi") src.cast::<u8>().add(n - 1) => _,
            inout("rcx") n => _, options(nostack));
    }
    dst
}
pub(crate) unsafe fn memset(dst: *mut c_void, value: c_int, n: usize) -> *mut c_void {
    unsafe {
        core::arch::asm!("rep stosb", inout("rdi") dst => _, inout("rcx") n => _, in("al") value as u8, options(nostack, preserves_flags));
    }
    dst
}
