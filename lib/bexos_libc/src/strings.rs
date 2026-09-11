//! Byte-oriented C locale strings used by the Mesa platform layer.
use core::{
    ffi::{c_char, c_int, c_void},
    ptr,
};
unsafe fn byte(p: *const c_char, i: usize) -> u8 {
    unsafe { p.add(i).read() as u8 }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strcmp(a: *const c_char, b: *const c_char) -> c_int {
    unsafe { strncmp(a, b, usize::MAX) }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> c_int {
    for i in 0..n {
        let (a, b) = unsafe { (byte(a, i), byte(b, i)) };
        if a != b || a == 0 {
            return a as c_int - b as c_int;
        }
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strcasecmp(a: *const c_char, b: *const c_char) -> c_int {
    for i in 0..usize::MAX {
        let (a, b) = unsafe {
            (
                byte(a, i).to_ascii_lowercase(),
                byte(b, i).to_ascii_lowercase(),
            )
        };
        if a != b || a == 0 {
            return a as c_int - b as c_int;
        }
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strchr(s: *const c_char, c: c_int) -> *mut c_char {
    for i in 0..usize::MAX {
        let b = unsafe { byte(s, i) };
        if b == c as u8 {
            return unsafe { s.add(i).cast_mut() };
        }
        if b == 0 {
            break;
        }
    }
    ptr::null_mut()
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strrchr(s: *const c_char, c: c_int) -> *mut c_char {
    let mut found = ptr::null_mut();
    for i in 0..usize::MAX {
        let b = unsafe { byte(s, i) };
        if b == c as u8 {
            found = unsafe { s.add(i).cast_mut() };
        }
        if b == 0 {
            break;
        }
    }
    found
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strcpy(out: *mut c_char, s: *const c_char) -> *mut c_char {
    for i in 0..usize::MAX {
        let b = unsafe { s.add(i).read() };
        unsafe {
            out.add(i).write(b);
        }
        if b == 0 {
            break;
        }
    }
    out
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strncpy(out: *mut c_char, s: *const c_char, n: usize) -> *mut c_char {
    let mut ended = false;
    for i in 0..n {
        let b = if ended { 0 } else { unsafe { s.add(i).read() } };
        ended |= b == 0;
        unsafe {
            out.add(i).write(b);
        }
    }
    out
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strnlen(s: *const c_char, n: usize) -> usize {
    (0..n).find(|i| unsafe { byte(s, *i) } == 0).unwrap_or(n)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strndup(s: *const c_char, n: usize) -> *mut c_char {
    let len = unsafe { strnlen(s, n) };
    let Some(size) = len.checked_add(1) else {
        crate::set_errno(crate::ENOMEM);
        return ptr::null_mut();
    };
    let out = unsafe { crate::malloc(size) }.cast::<c_char>();
    if !out.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(s, out, len);
            out.add(len).write(0);
        }
    }
    out
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strdup(s: *const c_char) -> *mut c_char {
    unsafe { strndup(s, usize::MAX) }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strstr(s: *const c_char, needle: *const c_char) -> *mut c_char {
    let n = unsafe { crate::strlen(needle) };
    if n == 0 {
        return s.cast_mut();
    }
    for i in 0..usize::MAX {
        if unsafe { byte(s, i) } == 0 {
            break;
        }
        if unsafe { strncmp(s.add(i), needle, n) } == 0 {
            return unsafe { s.add(i).cast_mut() };
        }
    }
    ptr::null_mut()
}
unsafe fn span(s: *const c_char, chars: *const c_char, accept: bool) -> usize {
    let mut set = [0u64; 4];
    for i in 0..usize::MAX {
        let b = unsafe { byte(chars, i) } as usize;
        if b == 0 {
            break;
        }
        set[b / 64] |= 1 << (b % 64);
    }
    for i in 0..usize::MAX {
        let b = unsafe { byte(s, i) } as usize;
        if b == 0 || (set[b / 64] & (1 << (b % 64)) != 0) != accept {
            return i;
        }
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strspn(s: *const c_char, chars: *const c_char) -> usize {
    unsafe { span(s, chars, true) }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strcspn(s: *const c_char, chars: *const c_char) -> usize {
    unsafe { span(s, chars, false) }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn bsearch(
    key: *const c_void,
    base: *const c_void,
    count: usize,
    size: usize,
    compare: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> c_int>,
) -> *mut c_void {
    let Some(compare) = compare else {
        return ptr::null_mut();
    };
    if size == 0 || count.checked_mul(size).is_none() {
        return ptr::null_mut();
    }
    let (mut low, mut high) = (0, count);
    while low < high {
        let mid = low + (high - low) / 2;
        let entry = unsafe { base.cast::<u8>().add(mid * size).cast() };
        match unsafe { compare(key, entry) }.cmp(&0) {
            core::cmp::Ordering::Less => high = mid,
            core::cmp::Ordering::Greater => low = mid + 1,
            core::cmp::Ordering::Equal => return entry.cast_mut(),
        }
    }
    ptr::null_mut()
}
