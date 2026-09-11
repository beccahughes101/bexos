use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static MAX_REQUEST: Cell<Option<usize>> = const { Cell::new(None) };
}

struct TrackingAllocator;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn record(size: usize) {
    let _ = MAX_REQUEST.try_with(|maximum| {
        if let Some(old) = maximum.get() {
            maximum.set(Some(old.max(size)));
        }
    });
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

pub fn largest_request<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            MAX_REQUEST.set(None);
        }
    }
    MAX_REQUEST.set(Some(0));
    let _reset = Reset;
    let result = operation();
    (result, MAX_REQUEST.get().unwrap())
}
