//! Process-wide logical allocator calls, shared by Rust and C. Requested bytes
//! are allocation traffic, not resident memory. Realloc counts once, whether it
//! moves or grows in place. VMOs created outside malloc are not heap allocations.
use crate::Locked;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub allocations: u64,
    pub reallocations: u64,
    pub frees: u64,
    pub requested_bytes: u64,
    pub failures: u64,
}
static COUNTERS: Locked<Snapshot> = Locked::new(Snapshot {
    allocations: 0,
    reallocations: 0,
    frees: 0,
    requested_bytes: 0,
    failures: 0,
});
pub fn snapshot() -> Snapshot {
    COUNTERS.with(|c| *c)
}
pub(crate) fn allocated(bytes: usize, failed: bool) {
    COUNTERS.with(|c| {
        if failed {
            c.failures = c.failures.saturating_add(1);
        } else {
            c.allocations = c.allocations.saturating_add(1);
            c.requested_bytes = c.requested_bytes.saturating_add(bytes as u64);
        }
    });
}
pub(crate) fn reallocated(bytes: usize, failed: bool) {
    COUNTERS.with(|c| {
        if failed {
            c.failures = c.failures.saturating_add(1);
        } else {
            c.reallocations = c.reallocations.saturating_add(1);
            c.requested_bytes = c.requested_bytes.saturating_add(bytes as u64);
        }
    });
}
pub(crate) fn freed() {
    COUNTERS.with(|c| {
        c.frees = c.frees.saturating_add(1);
    });
}
pub(crate) fn failed() {
    COUNTERS.with(|c| {
        c.failures = c.failures.saturating_add(1);
    });
}
impl Snapshot {
    pub fn since(self, before: Self) -> Self {
        Self {
            allocations: self.allocations.saturating_sub(before.allocations),
            reallocations: self.reallocations.saturating_sub(before.reallocations),
            frees: self.frees.saturating_sub(before.frees),
            requested_bytes: self.requested_bytes.saturating_sub(before.requested_bytes),
            failures: self.failures.saturating_sub(before.failures),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    #[test]
    fn logical_requests_count_realloc_once_and_preserve_failures() {
        // Exercise real Rust/C entry points against a private test heap.
        use core::alloc::{GlobalAlloc, Layout};
        let mut storage = std::vec![0u128; 8192];
        unsafe {
            crate::HEAP.init(storage.as_mut_ptr() as usize, storage.len() * 16);
        }
        crate::INITIALIZED.store(true, core::sync::atomic::Ordering::Release);
        let mut local = core::mem::MaybeUninit::<crate::ThreadLocal>::uninit();
        unsafe {
            crate::initialize_thread_local(local.as_mut_ptr(), 1);
        }
        let previous_tls = crate::HOST_THREAD_LOCAL.swap(
            local.as_mut_ptr() as usize,
            core::sync::atomic::Ordering::AcqRel,
        );
        let before = snapshot();
        unsafe {
            let a = crate::malloc(64);
            assert!(!a.is_null());
            a.cast::<u8>().write_bytes(0x5a, 64);
            let b = crate::realloc(a, 128);
            assert_eq!(a, b);
            assert!(
                core::slice::from_raw_parts(b.cast::<u8>(), 64)
                    .iter()
                    .all(|v| *v == 0x5a)
            );
            let blocker = crate::malloc(256);
            assert!(!blocker.is_null());
            let c = crate::realloc(b, 1024);
            assert!(!c.is_null());
            assert_ne!(b, c);
            assert!(crate::realloc(c, usize::MAX).is_null());
            assert!(
                core::slice::from_raw_parts(c.cast::<u8>(), 64)
                    .iter()
                    .all(|v| *v == 0x5a)
            );
            let mut aligned = core::ptr::null_mut();
            assert_eq!(crate::posix_memalign(&mut aligned, 256, 100), 0);
            assert_eq!(aligned as usize % 256, 0);
            let saved = aligned;
            assert_eq!(crate::posix_memalign(&mut aligned, 3, 100), crate::EINVAL);
            assert_eq!(aligned, saved);
            // C realloc(NULL,n) is an allocation, realloc(p,0) a free.
            let nullable = crate::realloc(core::ptr::null_mut(), 32);
            assert!(!nullable.is_null());
            assert!(crate::realloc(nullable, 0).is_null());
            crate::free(core::ptr::null_mut());
            let rust = crate::Allocator.alloc(Layout::from_size_align(48, 16).unwrap());
            assert!(!rust.is_null());
            crate::free(c);
            crate::free(blocker);
            crate::free(aligned);
            crate::Allocator.dealloc(rust, Layout::from_size_align(48, 16).unwrap());
        }
        let d = snapshot().since(before);
        assert_eq!(
            d,
            Snapshot {
                allocations: 5,
                reallocations: 2,
                frees: 5,
                requested_bytes: 64 + 256 + 100 + 48 + 128 + 1024 + 32,
                failures: 2
            }
        );
        crate::HOST_THREAD_LOCAL.store(previous_tls, core::sync::atomic::Ordering::Release);
        std::mem::forget(storage); // Heap storage remains valid for the test process.
        // The next test may initialize its own heap.
        crate::INITIALIZED.store(false, core::sync::atomic::Ordering::Release);
    }
}
