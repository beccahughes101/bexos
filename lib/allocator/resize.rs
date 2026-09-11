//! In-place resize preserves allocation headers while updating the free index.
use super::{
    Heap,
    free_tree::{self, MIN_EXTENT},
};

pub(super) unsafe fn in_place(heap: &Heap, pointer: *mut u8, size: usize) -> bool {
    let Some(new_end) = (pointer as usize)
        .checked_add(size.max(1))
        .and_then(|end| end.checked_add(15))
        .map(|end| end & !15)
    else {
        return false;
    };
    unsafe {
        heap.lock();
        let user = pointer as usize;
        let base = ((user - 16) as *const usize).read();
        let used = ((user - 8) as *mut usize).read();
        let end = base + used;
        let new_end = new_end.max(base + MIN_EXTENT);
        if new_end <= end {
            if end - new_end >= MIN_EXTENT {
                ((user - 8) as *mut usize).write(new_end - base);
                free_tree::insert(heap.head.get(), new_end, end - new_end);
            }
            heap.unlock();
            return true;
        }

        let growth = new_end - end;
        let node = free_tree::at(*heap.head.get(), end);
        if node.is_null() || (*node).size < growth {
            heap.unlock();
            return false;
        }
        let remaining = (*node).size - growth;
        let size = (*node).size;
        free_tree::remove(heap.head.get(), node);
        let consumed = if remaining >= MIN_EXTENT {
            free_tree::insert(heap.head.get(), new_end, remaining);
            growth
        } else {
            size
        };
        ((user - 8) as *mut usize).write(used + consumed);
        heap.unlock();
        true
    }
}
