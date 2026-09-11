//! Bounded allocation for one authenticated boot. No allocation survives the
//! verifier's reserved memory; kernel entry never adopts this arena.
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

const CAPACITY: usize = 256 * 1024;
static NEXT: AtomicUsize = AtomicUsize::new(0);
static mut MEMORY: [u8; CAPACITY] = [0; CAPACITY];

fn reserve(next: &AtomicUsize, base: usize, capacity: usize, layout: Layout) -> Option<usize> {
    let mut current = next.load(Ordering::Relaxed);
    loop {
        // Align the actual address, including the arena base. fetch_update
        // returns the previous cursor, which need not be the aligned start.
        let start =
            base.checked_add(current)?.checked_add(layout.align() - 1)? & !(layout.align() - 1);
        let end = start.checked_add(layout.size())?.checked_sub(base)?;
        if end > capacity {
            return None;
        }
        match next.compare_exchange_weak(current, end, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(start),
            Err(observed) => current = observed,
        }
    }
}

pub struct Bump;
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        reserve(
            &NEXT,
            core::ptr::addr_of_mut!(MEMORY) as usize,
            CAPACITY,
            layout,
        )
        .map_or(core::ptr::null_mut(), |address| address as *mut u8)
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_alignment_accounts_for_padding_and_preserves_bounds() {
        let next = AtomicUsize::new(0);
        let base = 0x1003;
        let mut end = base;
        for (size, alignment) in [(1, 1), (3, 8), (17, 64), (9, 4096)] {
            let address = reserve(
                &next,
                base,
                8192,
                Layout::from_size_align(size, alignment).unwrap(),
            )
            .unwrap();
            assert_eq!(address % alignment, 0);
            assert!(address >= end);
            end = address + size;
            assert_eq!(next.load(Ordering::Relaxed), end - base);
        }
        let saved = next.load(Ordering::Relaxed);
        assert_eq!(
            reserve(&next, base, 8192, Layout::from_size_align(8192, 1).unwrap()),
            None
        );
        assert_eq!(next.load(Ordering::Relaxed), saved);
    }
    #[test]
    fn address_overflow_never_changes_the_cursor() {
        let next = AtomicUsize::new(0);
        for alignment in [1, 8] {
            assert_eq!(
                reserve(
                    &next,
                    usize::MAX - 2,
                    8,
                    Layout::from_size_align(8, alignment).unwrap()
                ),
                None
            );
            assert_eq!(next.load(Ordering::Relaxed), 0);
        }
    }
}
