//! Allocation regression for idle waits and bounded input/report writes. The
//! host syscall shim returns an error; guest tests verify actual IPC delivery.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! { static TRACK: Cell<Option<usize>> = const { Cell::new(None) }; }
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACK.try_with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = TRACK.try_with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
#[test]
fn bounded_event_loop_ipc_allocates_no_scratch_buffers() {
    use bexos_userspace::Channel;
    let channels = [Channel(1); 64];
    let payload = [0; 2048];
    TRACK.with(|count| count.set(Some(0)));
    for _ in 0..100 {
        bexos_graphics_runtime::wait(std::hint::black_box(&channels), 1);
        let _ = Channel(1).send(std::hint::black_box(&payload), &[2, 3]);
        let _ = Channel(1).try_recv();
        let _ = bexos_graphics_runtime::stream::read_no_handles(Channel(1), &mut [0; 4096]);
    }
    let allocations = TRACK.with(|count| count.replace(None).unwrap());
    assert_eq!(allocations, 0);
}
