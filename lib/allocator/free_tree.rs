//! Intrusive AVL index of free extents, augmented with subtree capacity.
//! No index operation allocates. Address order permits neighbour coalescing;
//! subtree maxima skip holes too small to satisfy an allocation.
use core::{alloc::Layout, ptr::null_mut};

pub(super) const MIN_EXTENT: usize = (core::mem::size_of::<Free>() + 15) & !15;

#[repr(C)]
pub(super) struct Free {
    pub size: usize,
    maximum: usize,
    left: *mut Free,
    right: *mut Free,
    height: usize,
}

unsafe fn height(node: *mut Free) -> usize {
    unsafe { if node.is_null() { 0 } else { (*node).height } }
}
unsafe fn maximum(node: *mut Free) -> usize {
    unsafe { if node.is_null() { 0 } else { (*node).maximum } }
}
unsafe fn update(node: *mut Free) {
    unsafe {
        (*node).height = 1 + height((*node).left).max(height((*node).right));
        (*node).maximum = (*node)
            .size
            .max(maximum((*node).left))
            .max(maximum((*node).right));
    }
}
unsafe fn rotate_left(node: *mut Free) -> *mut Free {
    unsafe {
        let next = (*node).right;
        (*node).right = (*next).left;
        (*next).left = node;
        update(node);
        update(next);
        next
    }
}
unsafe fn rotate_right(node: *mut Free) -> *mut Free {
    unsafe {
        let next = (*node).left;
        (*node).left = (*next).right;
        (*next).right = node;
        update(node);
        update(next);
        next
    }
}
unsafe fn balance(node: *mut Free) -> *mut Free {
    unsafe {
        update(node);
        if height((*node).left) > height((*node).right) + 1 {
            let left = (*node).left;
            if height((*left).right) > height((*left).left) {
                (*node).left = rotate_left(left);
            }
            return rotate_right(node);
        }
        if height((*node).right) > height((*node).left) + 1 {
            let right = (*node).right;
            if height((*right).left) > height((*right).right) {
                (*node).right = rotate_right(right);
            }
            return rotate_left(node);
        }
        node
    }
}
unsafe fn add(root: *mut Free, node: *mut Free) -> *mut Free {
    unsafe {
        if root.is_null() {
            return node;
        }
        if (node as usize) < root as usize {
            (*root).left = add((*root).left, node);
        } else {
            (*root).right = add((*root).right, node);
        }
        balance(root)
    }
}
unsafe fn detach_min(root: *mut Free) -> (*mut Free, *mut Free) {
    unsafe {
        if (*root).left.is_null() {
            return ((*root).right, root);
        }
        let (left, minimum) = detach_min((*root).left);
        (*root).left = left;
        (balance(root), minimum)
    }
}
unsafe fn erase(root: *mut Free, base: usize) -> *mut Free {
    unsafe {
        if root.is_null() {
            return root;
        }
        if base < root as usize {
            (*root).left = erase((*root).left, base);
        } else if base > root as usize {
            (*root).right = erase((*root).right, base);
        } else {
            if (*root).left.is_null() {
                return (*root).right;
            }
            if (*root).right.is_null() {
                return (*root).left;
            }
            // Move the successor's index links, never its physical extent.
            let (right, successor) = detach_min((*root).right);
            (*successor).left = (*root).left;
            (*successor).right = right;
            return balance(successor);
        }
        balance(root)
    }
}

/// Caller owns the extent and holds the heap lock.
pub(super) unsafe fn insert(root: *mut *mut Free, mut base: usize, mut size: usize) {
    unsafe {
        let (mut previous, mut next) = (null_mut(), null_mut());
        let mut node = *root;
        while !node.is_null() {
            if (node as usize) < base {
                previous = node;
                node = (*node).right;
            } else {
                next = node;
                node = (*node).left;
            }
        }
        if !next.is_null() && base + size == next as usize {
            size += (*next).size;
            *root = erase(*root, next as usize);
        }
        if !previous.is_null() && previous as usize + (*previous).size == base {
            base = previous as usize;
            size += (*previous).size;
            *root = erase(*root, base);
        }
        let node = base as *mut Free;
        node.write(Free {
            size,
            maximum: size,
            left: null_mut(),
            right: null_mut(),
            height: 1,
        });
        *root = add(*root, node);
    }
}

pub(super) unsafe fn remove(root: *mut *mut Free, node: *mut Free) {
    unsafe {
        *root = erase(*root, node as usize);
    }
}

pub(super) unsafe fn at(mut root: *mut Free, base: usize) -> *mut Free {
    unsafe {
        while !root.is_null() && root as usize != base {
            root = if base < root as usize {
                (*root).left
            } else {
                (*root).right
            };
        }
        root
    }
}

pub(super) fn placement(base: usize, layout: Layout) -> Option<(usize, usize)> {
    let align = layout.align().max(16);
    let user = base.checked_add(16)?.checked_add(align - 1)? & !(align - 1);
    let end = user.checked_add(layout.size().max(1))?.checked_add(15)? & !15;
    Some((user, (end - base).max(MIN_EXTENT)))
}

pub(super) unsafe fn find_fit(root: *mut Free, layout: Layout) -> *mut Free {
    unsafe {
        let Some(minimum) = layout.size().max(1).checked_add(16) else {
            return null_mut();
        };
        if maximum(root) < minimum.max(MIN_EXTENT) {
            return null_mut();
        }
        let left = find_fit((*root).left, layout);
        if !left.is_null() {
            return left;
        }
        if placement(root as usize, layout).is_some_and(|(_, used)| used <= (*root).size) {
            return root;
        }
        find_fit((*root).right, layout)
    }
}

#[cfg(test)]
pub(super) unsafe fn validate(root: *mut Free, lower: usize, upper: usize) -> (usize, usize) {
    unsafe {
        if root.is_null() {
            return (0, 0);
        }
        let base = root as usize;
        assert!(lower <= base && base + (*root).size <= upper);
        assert!((*root).size >= MIN_EXTENT);
        let (lh, lm) = validate((*root).left, lower, base);
        let (rh, rm) = validate((*root).right, base + (*root).size, upper);
        assert!(lh.abs_diff(rh) <= 1);
        assert_eq!((*root).height, 1 + lh.max(rh));
        assert_eq!((*root).maximum, (*root).size.max(lm).max(rm));
        ((*root).height, (*root).maximum)
    }
}
