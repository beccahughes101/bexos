//! Real guest coverage of direct mappings and shared C/Rust allocation counters.
#[cfg(bexos_guest)]
pub fn verify() {
    use bexos_libc::{allocation_stats, calloc, free, malloc, realloc};
    let before = allocation_stats::snapshot();
    unsafe {
        let size = 2 * 1024 * 1024 + 4096;
        let p = malloc(size).cast::<u8>();
        assert!(!p.is_null());
        p.write(37);
        p.add(size - 1).write(83);
        let q = realloc(p.cast(), size + 4096).cast::<u8>();
        assert!(!q.is_null());
        assert_eq!(q.read(), 37);
        assert_eq!(q.add(size - 1).read(), 83);
        assert!(
            core::hint::black_box(
                calloc as unsafe extern "C" fn(usize, usize) -> *mut core::ffi::c_void
            )(core::hint::black_box(usize::MAX), 2)
            .is_null()
        );
        free(q.cast());
    }
    let d = allocation_stats::snapshot().since(before);
    // Direct mapping uses real Memory IPC; its temporary Rust allocations are
    // intentionally included in process totals rather than hidden by the probe.
    assert!(d.allocations >= 1 && d.frees >= 1);
    assert!(d.requested_bytes >= 4 * 1024 * 1024 + 3 * 4096);
    assert_eq!(d.reallocations, 1);
    assert_eq!(d.failures, 1);
    let (thread, process) = bexos_userspace::syscall::runtime_stats().unwrap();
    assert!(thread > 0 && process >= thread);
    bexos_userspace::log("input-fixture: runtime CPU and direct allocation counters verified\n");
}
#[cfg(not(bexos_guest))]
pub fn verify() {}
