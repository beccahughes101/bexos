#![no_std]
#[cfg(test)]
extern crate std;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

mod free_tree;
mod resize;
use free_tree::{Free, MIN_EXTENT};
pub struct Heap {
    held: AtomicBool,
    head: UnsafeCell<*mut Free>,
}
unsafe impl Sync for Heap {}
impl Heap {
    pub const fn new() -> Self {
        Self {
            held: AtomicBool::new(false),
            head: UnsafeCell::new(core::ptr::null_mut()),
        }
    }
    /// The caller grants exclusive ownership of this aligned, writable range.
    pub unsafe fn init(&self, base: usize, size: usize) {
        unsafe {
            assert!(base % 16 == 0 && size >= MIN_EXTENT && size % 16 == 0);
            *self.head.get() = core::ptr::null_mut();
            free_tree::insert(self.head.get(), base, size);
        }
    }
    /// The caller grants ownership of another aligned, writable range.
    pub unsafe fn add_region(&self, base: usize, size: usize) {
        unsafe {
            assert!(base % 16 == 0 && size >= MIN_EXTENT && size % 16 == 0);
            self.lock();
            free_tree::insert(self.head.get(), base, size);
            self.unlock();
        }
    }
    /// Resize an owned allocation without moving it. On failure it is unchanged.
    /// The caller must exclusively own a live pointer allocated by this heap.
    pub unsafe fn resize_in_place(&self, pointer: *mut u8, size: usize) -> bool {
        unsafe { resize::in_place(self, pointer, size) }
    }
    fn lock(&self) {
        while self
            .held
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    fn unlock(&self) {
        self.held.store(false, Ordering::Release);
    }
}
unsafe impl GlobalAlloc for Heap {
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        unsafe {
            let Ok(next_layout) = Layout::from_size_align(size, layout.align()) else {
                return core::ptr::null_mut();
            };
            if self.resize_in_place(pointer, size) {
                return pointer;
            }
            let next = self.alloc(next_layout);
            if !next.is_null() {
                core::ptr::copy_nonoverlapping(pointer, next, layout.size().min(size));
                self.dealloc(pointer, layout);
            }
            next
        }
    }
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            self.lock();
            let node = free_tree::find_fit(*self.head.get(), layout);
            if !node.is_null() {
                let base = node as usize;
                let (user, mut used) = free_tree::placement(base, layout).unwrap();
                let size = (*node).size;
                free_tree::remove(self.head.get(), node);
                if size - used >= MIN_EXTENT {
                    free_tree::insert(self.head.get(), base + used, size - used);
                } else {
                    used = size;
                }
                ((user - 16) as *mut usize).write(base);
                ((user - 8) as *mut usize).write(used);
                self.unlock();
                return user as *mut u8;
            }
            self.unlock();
            core::ptr::null_mut()
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe {
            if ptr.is_null() {
                return;
            }
            self.lock();
            let user = ptr as usize;
            let base = ((user - 16) as *const usize).read();
            let size = ((user - 8) as *const usize).read();
            free_tree::insert(self.head.get(), base, size);
            self.unlock();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[repr(align(4096))]
    struct Arena([u8; 16384]);
    #[test]
    fn reuses_and_coalesces_aligned_allocations() {
        let mut arena = Arena([0; 16384]);
        let h = Heap::new();
        unsafe {
            h.init(arena.0.as_mut_ptr() as usize, 16384);
            let layout = Layout::from_size_align(123, 4096).unwrap();
            let a = h.alloc(layout);
            let b = h.alloc(layout);
            assert!(!a.is_null() && !b.is_null());
            assert_eq!(a as usize % 4096, 0);
            h.dealloc(a, layout);
            h.dealloc(b, layout);
            let full = Layout::from_size_align(16000, 16).unwrap();
            let c = h.alloc(full);
            assert!(!c.is_null());
            h.dealloc(c, full);
        }
    }

    #[test]
    fn allocations_can_grow_into_an_additional_region() {
        let mut first = Arena([0; 16384]);
        let mut second = Arena([0; 16384]);
        let h = Heap::new();
        unsafe {
            h.init(first.0.as_mut_ptr() as usize, 16384);
            let layout = Layout::from_size_align(12000, 16).unwrap();
            let a = h.alloc(layout);
            assert!(!a.is_null());
            assert!(h.alloc(layout).is_null());
            h.add_region(second.0.as_mut_ptr() as usize, 16384);
            let b = h.alloc(layout);
            assert!(!b.is_null());
            h.dealloc(a, layout);
            h.dealloc(b, layout);
        }
    }

    #[test]
    fn insertion_bridges_both_neighbours() {
        let mut arena = Arena([0; 16384]);
        let heap = Heap::new();
        unsafe {
            let base = arena.0.as_mut_ptr() as usize;
            heap.init(base, 4096);
            heap.add_region(base + 8192, 8192);
            heap.add_region(base + 4096, 4096);
            let full = Layout::from_size_align(16000, 16).unwrap();
            let pointer = heap.alloc(full);
            assert!(!pointer.is_null());
            heap.dealloc(pointer, full);
        }
    }

    #[test]
    fn resize_reuses_neighbour_space_and_releases_shrunk_storage() {
        let mut arena = Arena([0; 16384]);
        let heap = Heap::new();
        unsafe {
            heap.init(arena.0.as_mut_ptr() as usize, 16384);
            let layout = Layout::from_size_align(256, 256).unwrap();
            let first = heap.alloc(layout);
            let neighbour = heap.alloc(layout);
            assert!(!first.is_null() && !neighbour.is_null());
            core::ptr::write_bytes(first, 0xa5, layout.size());
            heap.dealloc(neighbour, layout);
            let grown = heap.realloc(first, layout, 4096);
            assert_eq!(grown, first);
            assert!(
                core::slice::from_raw_parts(grown, 256)
                    .iter()
                    .all(|v| *v == 0xa5)
            );
            let shrunk = heap.realloc(grown, Layout::from_size_align(4096, 256).unwrap(), 32);
            assert_eq!(shrunk, first);
            let large_layout = Layout::from_size_align(15000, 16).unwrap();
            let large = heap.alloc(large_layout);
            assert!(!large.is_null(), "shrunk storage must be available again");
            assert!(
                core::slice::from_raw_parts(shrunk, 32)
                    .iter()
                    .all(|v| *v == 0xa5)
            );
            heap.dealloc(large, large_layout);
            heap.dealloc(shrunk, Layout::from_size_align(32, 256).unwrap());
            let full = Layout::from_size_align(16000, 16).unwrap();
            let pointer = heap.alloc(full);
            assert!(!pointer.is_null());
            heap.dealloc(pointer, full);
        }
    }

    #[test]
    fn failed_resize_preserves_the_owner_and_moving_resize_preserves_alignment() {
        let mut arena = Arena([0; 16384]);
        let heap = Heap::new();
        unsafe {
            heap.init(arena.0.as_mut_ptr() as usize, 16384);
            let layout = Layout::from_size_align(128, 256).unwrap();
            let first = heap.alloc(layout);
            let blocker = heap.alloc(layout);
            assert!(!first.is_null() && !blocker.is_null());
            core::ptr::write_bytes(first, 0xa5, 128);
            core::ptr::write_bytes(blocker, 0x5a, 128);
            assert!(!heap.resize_in_place(first, 1024));
            assert!(heap.realloc(first, layout, 32768).is_null());
            assert!(
                core::slice::from_raw_parts(first, 128)
                    .iter()
                    .all(|v| *v == 0xa5)
            );
            let moved = heap.realloc(first, layout, 1024);
            assert!(!moved.is_null());
            assert_ne!(moved, first);
            assert_eq!(moved as usize % 256, 0);
            assert!(
                core::slice::from_raw_parts(moved, 128)
                    .iter()
                    .all(|v| *v == 0xa5)
            );
            assert!(
                core::slice::from_raw_parts(blocker, 128)
                    .iter()
                    .all(|v| *v == 0x5a)
            );
            heap.dealloc(moved, Layout::from_size_align(1024, 256).unwrap());
            heap.dealloc(blocker, layout);
            let full = Layout::from_size_align(16000, 16).unwrap();
            let pointer = heap.alloc(full);
            assert!(!pointer.is_null());
            heap.dealloc(pointer, full);
        }
    }

    #[test]
    fn fragmented_allocations_preserve_live_bytes_and_recover_the_arena() {
        let mut arena = Arena([0; 16384]);
        let heap = Heap::new();
        let mut live = [(core::ptr::null_mut::<u8>(), 0usize, 16usize, 0u8); 48];
        let mut seed = 0xbeef_u64;
        unsafe {
            heap.init(arena.0.as_mut_ptr() as usize, 16384);
            for _ in 0..1024 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let index = seed as usize % live.len();
                let (pointer, size, align, value) = live[index];
                if !pointer.is_null() && seed & 1 == 0 {
                    let resized = (seed as usize >> 8) % 512 + 1;
                    let next = heap.realloc(
                        pointer,
                        Layout::from_size_align(size, align).unwrap(),
                        resized,
                    );
                    if !next.is_null() {
                        assert_eq!(next as usize % align, 0);
                        assert!(
                            core::slice::from_raw_parts(next, size.min(resized))
                                .iter()
                                .all(|byte| *byte == value)
                        );
                        core::ptr::write_bytes(next, value, resized);
                        live[index] = (next, resized, align, value);
                    }
                } else if !pointer.is_null() {
                    heap.dealloc(pointer, Layout::from_size_align(size, align).unwrap());
                    live[index].0 = core::ptr::null_mut();
                } else {
                    let size = (seed as usize >> 8) % 512 + 1;
                    let align = 16 << ((seed as usize >> 20) % 5);
                    let pointer = heap.alloc(Layout::from_size_align(size, align).unwrap());
                    if !pointer.is_null() {
                        assert_eq!(pointer as usize % align, 0);
                        let value = (index + 1) as u8;
                        core::ptr::write_bytes(pointer, value, size);
                        live[index] = (pointer, size, align, value);
                    }
                }
                for &(pointer, size, _, value) in &live {
                    if !pointer.is_null() {
                        assert!(
                            core::slice::from_raw_parts(pointer, size)
                                .iter()
                                .all(|byte| *byte == value)
                        );
                    }
                }
                free_tree::validate(*heap.head.get(), 0, usize::MAX);
            }
            for (pointer, size, align, _) in live {
                if !pointer.is_null() {
                    heap.dealloc(pointer, Layout::from_size_align(size, align).unwrap());
                }
            }
            let full = Layout::from_size_align(16000, 16).unwrap();
            let pointer = heap.alloc(full);
            assert!(!pointer.is_null());
            heap.dealloc(pointer, full);
        }
    }

    #[test]
    fn thousands_of_holes_remain_balanced_and_coalesce_after_reuse() {
        // Allocate the backing arena directly on the host heap: a debug build
        // can otherwise copy the 1 MiB array through several test-stack slots.
        let mut arena = std::vec![0u128; 1 << 16];
        let arena_bytes = arena.len() * core::mem::size_of::<u128>();
        let heap = Heap::new();
        let mut pointers = [core::ptr::null_mut(); 8192];
        let small = Layout::from_size_align(32, 16).unwrap();
        let medium = Layout::from_size_align(256, 256).unwrap();
        unsafe {
            heap.init(arena.as_mut_ptr() as usize, arena_bytes);
            for pointer in &mut pointers {
                *pointer = heap.alloc(small);
                assert!(!pointer.is_null());
                core::ptr::write_bytes(*pointer, 0x5a, small.size());
            }
            // Monotonic insertion is the worst case for an unbalanced index.
            for index in (0..pointers.len()).step_by(2) {
                heap.dealloc(pointers[index], small);
                pointers[index] = core::ptr::null_mut();
            }
            let (height, _) = free_tree::validate(*heap.head.get(), 0, usize::MAX);
            assert!(
                height <= 18,
                "fragmented heap index grew to {height} levels"
            );
            for _ in 0..8192 {
                let pointer = heap.alloc(medium);
                assert!(!pointer.is_null());
                assert_eq!(pointer as usize % 256, 0);
                heap.dealloc(pointer, medium);
            }
            // Interleave removal and coalescing in a non-address order.
            for step in 0..4096 {
                let index = ((step * 2053) % 4096) * 2 + 1;
                let pointer = pointers[index];
                assert!(
                    core::slice::from_raw_parts(pointer, 32)
                        .iter()
                        .all(|b| *b == 0x5a)
                );
                heap.dealloc(pointer, small);
                if step % 64 == 0 {
                    free_tree::validate(*heap.head.get(), 0, usize::MAX);
                }
            }
            let (height, maximum) = free_tree::validate(*heap.head.get(), 0, usize::MAX);
            assert_eq!((height, maximum), (1, arena_bytes));
            let full = Layout::from_size_align(arena_bytes - 16, 16).unwrap();
            let pointer = heap.alloc(full);
            assert!(!pointer.is_null());
            heap.dealloc(pointer, full);
        }
    }
}
