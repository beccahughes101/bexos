use crate::*;
unsafe extern "C" {
    static __bexos_tls_start: u8;
    static __bexos_tls_alignment: u8;
    static __bexos_tls_file_end: u8;
    static __bexos_pthread_local: u8;
    static __bexos_tls_end: u8;
}
fn tls_offsets() -> (usize, usize, usize) {
    let start = core::ptr::addr_of!(__bexos_tls_start) as usize;
    (
        core::ptr::addr_of!(__bexos_tls_file_end) as usize - start,
        core::ptr::addr_of!(__bexos_pthread_local) as usize - start,
        core::ptr::addr_of!(__bexos_tls_end) as usize - start,
    )
}
fn executable_tls_size() -> usize {
    let (_, _, size) = tls_offsets();
    {
        let align = (core::ptr::addr_of!(__bexos_tls_alignment) as usize).max(1);
        (size + align - 1) & !(align - 1)
    }
}
pub(crate) unsafe fn alloc_thread_state(pthread_id: usize) -> *mut c_void {
    let (file_size, local_offset, _) = tls_offsets();
    let (mem_size, align) =
        bexos_userspace::dynamic_link::tls_layout().unwrap_or((executable_tls_size(), 4096));
    let Some(size) = mem_size.checked_add(16) else {
        set_errno(ENOMEM);
        return ptr::null_mut();
    };
    let base = unsafe { alloc_with_align(size, align.max(4096) as usize) }.cast::<u8>();
    if base.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        base.write_bytes(0, size);
        let target = core::slice::from_raw_parts_mut(base, mem_size);
        if bexos_userspace::dynamic_link::copy_tls_template(target).is_none() {
            ptr::copy_nonoverlapping(
                core::ptr::addr_of!(__bexos_tls_start),
                base.add(mem_size - executable_tls_size()),
                file_size,
            );
        }
        let tp = base.add(mem_size);
        tp.cast::<usize>().write(tp as usize);
        tp.add(8).cast::<usize>().write(base as usize);
        initialize_thread_local(
            tp.sub(executable_tls_size()).add(local_offset).cast(),
            pthread_id,
        );
        tp.cast()
    }
}
pub(crate) fn set_thread_pointer(pointer: *mut c_void) {
    bexos_userspace::syscall::set_thread_pointer(pointer as u64);
}
pub(crate) fn current_thread_local() -> *mut ThreadLocal {
    let tp = bexos_userspace::syscall::thread_pointer() as usize;
    if tp == 0 {
        return ptr::null_mut();
    }
    let (_, local_offset, _) = tls_offsets();
    (tp - executable_tls_size() + local_offset) as *mut ThreadLocal
}
