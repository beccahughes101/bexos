pub(super) unsafe fn normal_ram(base: u64, bytes: u64) {
    unsafe {
        core::ptr::write_bytes(base as *mut u8, 0, bytes as usize);
    }
}
