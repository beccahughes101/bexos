use crate::{c_int, c_void};
#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
pub(crate) unsafe fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    unsafe {
        let output = dst.cast::<u8>();
        let input = src.cast::<u8>();
        let remaining = n;
        core::arch::asm!(
            "cmp {remaining}, #8",
            "b.lo 3f",
            "2:",
            "ldr x9, [{input}], #8",
            "str x9, [{output}], #8",
            "sub {remaining}, {remaining}, #8",
            "cmp {remaining}, #8",
            "b.hs 2b",
            "3:",
            "cbz {remaining}, 5f",
            "4:",
            "ldrb w9, [{input}], #1",
            "strb w9, [{output}], #1",
            "subs {remaining}, {remaining}, #1",
            "b.ne 4b",
            "5:",
            output = inout(reg) output => _,
            input = inout(reg) input => _,
            remaining = inout(reg) remaining => _,
            out("w9") _,
            options(nostack),
        );
        return dst;
    }
}
pub(crate) unsafe fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    if n == 0 {
        return dst;
    }
    if (dst as usize) <= src as usize || (dst as usize).wrapping_sub(src as usize) >= n {
        return unsafe { memcpy(dst, src, n) };
    }
    unsafe {
        let output = dst.cast::<u8>().add(n);
        let input = src.cast::<u8>().add(n);
        let remaining = n;
        core::arch::asm!(
            "cmp {remaining}, #8",
            "b.lo 3f",
            "2:",
            "ldr x9, [{input}, #-8]!",
            "str x9, [{output}, #-8]!",
            "sub {remaining}, {remaining}, #8",
            "cmp {remaining}, #8",
            "b.hs 2b",
            "3:",
            "cbz {remaining}, 5f",
            "4:",
            "ldrb w9, [{input}, #-1]!",
            "strb w9, [{output}, #-1]!",
            "subs {remaining}, {remaining}, #1",
            "b.ne 4b",
            "5:",
            output = inout(reg) output => _,
            input = inout(reg) input => _,
            remaining = inout(reg) remaining => _,
            out("w9") _,
            options(nostack),
        );
        return dst;
    }
}
pub(crate) unsafe fn memset(dst: *mut c_void, value: c_int, n: usize) -> *mut c_void {
    unsafe {
        let output = dst.cast::<u8>();
        let remaining = n;
        let repeated = u64::from(value as u8) * 0x0101_0101_0101_0101;
        core::arch::asm!(
            "cmp {remaining}, #8",
            "b.lo 3f",
            "2:",
            "str {value:x}, [{output}], #8",
            "sub {remaining}, {remaining}, #8",
            "cmp {remaining}, #8",
            "b.hs 2b",
            "3:",
            "cbz {remaining}, 5f",
            "4:",
            "strb {value:w}, [{output}], #1",
            "subs {remaining}, {remaining}, #1",
            "b.ne 4b",
            "5:",
            output = inout(reg) output => _,
            remaining = inout(reg) remaining => _,
            value = in(reg) repeated,
            options(nostack),
        );
        return dst;
    }
}
