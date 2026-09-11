use crate::*;
unsafe extern "C" {
    static __bexos_tls_start: u8;
    static __bexos_tls_file_end: u8;
    static __bexos_pthread_local: u8;
    static __bexos_tls_end: u8;
}

pub(crate) fn tls_offsets() -> (usize, usize, usize) {
    let start = core::ptr::addr_of!(__bexos_tls_start).addr();
    let file_end = core::ptr::addr_of!(__bexos_tls_file_end).addr();
    let pthread_local = core::ptr::addr_of!(__bexos_pthread_local).addr();
    let end = core::ptr::addr_of!(__bexos_tls_end).addr();
    (file_end - start, pthread_local - start, end - start)
}

pub(crate) unsafe fn alloc_thread_state(pthread_id: usize) -> *mut c_void {
    let (file_size, local_offset, mem_size) = tls_offsets();
    let tcb_size = bexos_boot::USER_TLS_TCB_SIZE as usize;
    let mut aggregate_tls = Vec::new();
    let (template_file_size, template_mem_size) = {
        if let Some((aggregate_mem_size, _align)) = bexos_userspace::dynamic_link::tls_layout() {
            aggregate_tls.resize(aggregate_mem_size, 0);
        }
        match bexos_userspace::dynamic_link::copy_tls_template(&mut aggregate_tls) {
            Some((copied_file_size, copied_mem_size, _align))
                if copied_mem_size >= mem_size
                    && copied_mem_size >= local_offset + core::mem::size_of::<ThreadLocal>() =>
            {
                aggregate_tls.resize(copied_mem_size, 0);
                (copied_file_size, copied_mem_size)
            }
            _ => {
                aggregate_tls.clear();
                (file_size, mem_size)
            }
        }
    };
    let Some(allocation_size) = template_mem_size.checked_add(tcb_size) else {
        set_errno(ENOMEM);
        return ptr::null_mut();
    };
    let thread_pointer = unsafe { alloc_with_align(allocation_size, 16) }.cast::<u8>();
    if thread_pointer.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        thread_pointer.write_bytes(0, tcb_size);
        if aggregate_tls.is_empty() {
            ptr::copy_nonoverlapping(
                core::ptr::addr_of!(__bexos_tls_start),
                thread_pointer.add(tcb_size),
                template_file_size,
            );
            thread_pointer
                .add(tcb_size + template_file_size)
                .write_bytes(0, template_mem_size - template_file_size);
        } else {
            ptr::copy_nonoverlapping(
                aggregate_tls.as_ptr(),
                thread_pointer.add(tcb_size),
                template_mem_size,
            );
        }
        initialize_thread_local(
            thread_pointer
                .add(tcb_size + local_offset)
                .cast::<ThreadLocal>(),
            pthread_id,
        );
    }
    thread_pointer.cast()
}

pub(crate) fn set_thread_pointer(thread_pointer: *mut c_void) {
    unsafe {
        core::arch::asm!("msr tpidr_el0, {thread_pointer:x}", thread_pointer = in(reg) thread_pointer as usize);
    }
}

pub(crate) fn current_thread_local() -> *mut ThreadLocal {
    let thread_pointer: usize;
    unsafe {
        core::arch::asm!("mrs {thread_pointer:x}, tpidr_el0", thread_pointer = out(reg) thread_pointer, options(nomem, nostack));
    }
    if thread_pointer == 0 {
        return ptr::null_mut();
    }
    let (_, local_offset, _) = tls_offsets();
    (thread_pointer + bexos_boot::USER_TLS_TCB_SIZE as usize + local_offset) as *mut ThreadLocal
}
