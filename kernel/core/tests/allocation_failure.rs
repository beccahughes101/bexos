//! Per-thread allocation failure injection; other tests keep the system allocator.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static DENIED_SIZE: Cell<usize> = const { Cell::new(0) };
}
struct Allocator;
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let denied = DENIED_SIZE
            .try_with(|size| {
                if size.get() == layout.size() {
                    size.set(0);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if denied {
            core::ptr::null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

pub fn deny_next<T>(bytes: usize, f: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            DENIED_SIZE.with(|size| size.set(0));
        }
    }
    let _reset = Reset;
    DENIED_SIZE.with(|size| size.set(bytes));
    f()
}
