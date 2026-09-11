use crate::{c_int, c_void};
use core::ptr::{read_volatile, write_volatile};
// Volatile loops prevent recursive lowering to the exported libc symbols.
pub(crate) unsafe fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    unsafe {
        for index in 0..n {
            write_volatile(
                dst.cast::<u8>().add(index),
                read_volatile(src.cast::<u8>().add(index)),
            );
        }
    }
    dst
}
pub(crate) unsafe fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    if (dst as usize) <= src as usize {
        return unsafe { memcpy(dst, src, n) };
    }
    unsafe {
        for index in (0..n).rev() {
            write_volatile(
                dst.cast::<u8>().add(index),
                read_volatile(src.cast::<u8>().add(index)),
            );
        }
    }
    dst
}
pub(crate) unsafe fn memset(dst: *mut c_void, value: c_int, n: usize) -> *mut c_void {
    unsafe {
        for index in 0..n {
            write_volatile(dst.cast::<u8>().add(index), value as u8);
        }
    }
    dst
}
