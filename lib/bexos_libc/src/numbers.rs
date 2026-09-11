//! Checked C integer conversion with exact end-pointer and overflow behavior.
use core::ffi::{c_char, c_int};
fn digit(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'a'..=b'z' => (c - b'a' + 10) as u32,
        b'A'..=b'Z' => (c - b'A' + 10) as u32,
        _ => 36,
    }
}
unsafe fn signed(s: *const c_char, end: *mut *mut c_char, base: c_int, binary: bool) -> i64 {
    if !end.is_null() {
        unsafe {
            end.write(s.cast_mut());
        }
    }
    if base != 0 && !(2..=36).contains(&base) {
        crate::set_errno(crate::EINVAL);
        return 0;
    }
    let mut p = s.cast::<u8>();
    while matches!(
        unsafe { *p },
        b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c'
    ) {
        p = unsafe { p.add(1) };
    }
    let negative = unsafe { *p } == b'-';
    if matches!(unsafe { *p }, b'-' | b'+') {
        p = unsafe { p.add(1) };
    }
    let mut radix = base as u32;
    if unsafe { *p } == b'0' {
        let marker = unsafe { *p.add(1) };
        let prefix = if matches!(marker, b'x' | b'X') && (radix == 0 || radix == 16) {
            16
        } else if binary && matches!(marker, b'b' | b'B') && (radix == 0 || radix == 2) {
            2
        } else {
            0
        };
        if prefix != 0 && digit(unsafe { *p.add(2) }) < prefix {
            radix = prefix;
            p = unsafe { p.add(2) };
        } else if radix == 0 {
            radix = 8;
        }
    }
    if radix == 0 {
        radix = 10;
    }
    let first = p;
    let max = i64::MAX as u64 + negative as u64;
    let (mut number, mut overflow) = (0u64, false);
    loop {
        let d = digit(unsafe { *p });
        if d >= radix {
            break;
        }
        match number
            .checked_mul(radix as u64)
            .and_then(|n| n.checked_add(d as u64))
            .filter(|n| *n <= max)
        {
            Some(n) => number = n,
            None => overflow = true,
        }
        p = unsafe { p.add(1) };
    }
    if p != first && !end.is_null() {
        unsafe {
            end.write(p.cast::<c_char>().cast_mut());
        }
    }
    if overflow {
        crate::set_errno(34);
        return if negative { i64::MIN } else { i64::MAX };
    }
    if negative {
        (number as i64).wrapping_neg()
    } else {
        number as i64
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strtoll(s: *const c_char, end: *mut *mut c_char, base: c_int) -> i64 {
    unsafe { signed(s, end, base, false) }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn atoi(s: *const c_char) -> c_int {
    unsafe { signed(s, core::ptr::null_mut(), 10, false) as c_int }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn __isoc23_strtoll(
    s: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> i64 {
    unsafe { signed(s, end, base, true) }
}
