//! Run the real component contract against BexOS's allocator on the host.
//! This catches allocation-heavy startup regressions without booting QEMU.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::sync::{
    Once,
    atomic::{AtomicUsize, Ordering},
};

const CAPACITY: usize = 256 << 20;
#[repr(align(4096))]
struct Arena(UnsafeCell<[u8; CAPACITY]>);
unsafe impl Sync for Arena {}
static ARENA: Arena = Arena(UnsafeCell::new([0; CAPACITY]));
static HEAP: bexos_allocator::Heap = bexos_allocator::Heap::new();
static INITIALIZED: Once = Once::new();
static FALLBACKS: AtomicUsize = AtomicUsize::new(0);
static FALLBACK_SIZE: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let arena = ARENA.0.get() as usize;
        let in_heap = (arena..arena + CAPACITY).contains(&(pointer as usize));
        let result = if in_heap {
            let next = unsafe { HEAP.realloc(pointer, layout, size) };
            if next.is_null() {
                FALLBACKS.fetch_add(1, Ordering::Relaxed);
                FALLBACK_SIZE.fetch_max(size, Ordering::Relaxed);
                let next = unsafe {
                    System.alloc(Layout::from_size_align_unchecked(size, layout.align()))
                };
                if !next.is_null() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(pointer, next, layout.size().min(size));
                        HEAP.dealloc(pointer, layout);
                    }
                }
                next
            } else {
                next
            }
        } else {
            unsafe { System.realloc(pointer, layout, size) }
        };
        if !result.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        result
    }

    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        INITIALIZED.call_once(|| unsafe {
            HEAP.init(ARENA.0.get() as usize, CAPACITY);
        });
        let result = unsafe { HEAP.alloc(layout) };
        if result.is_null() {
            FALLBACKS.fetch_add(1, Ordering::Relaxed);
            FALLBACK_SIZE.fetch_max(layout.size(), Ordering::Relaxed);
            unsafe { System.alloc(layout) }
        } else {
            result
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        let base = ARENA.0.get() as usize;
        if (base..base + CAPACITY).contains(&(pointer as usize)) {
            unsafe { HEAP.dealloc(pointer, layout) };
        } else {
            unsafe { System.dealloc(pointer, layout) };
        }
    }
}

pub fn assert_no_fallback() {
    assert_eq!(
        FALLBACKS.load(Ordering::Relaxed),
        0,
        "BexOS heap exhausted; largest fallback={} peak live={}",
        FALLBACK_SIZE.load(Ordering::Relaxed),
        PEAK.load(Ordering::Relaxed)
    );
}
