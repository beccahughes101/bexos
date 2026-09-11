#![no_std]

#[cfg(all(target_arch = "aarch64", any(target_os = "linux", target_os = "none")))]
core::arch::global_asm!(
    r#"
    .section .note.gnu.property, "a", %note
    .balign 8
    .long 4
    .long 16
    .long 5
    .asciz "GNU"
    .balign 8
    .long 0xc0000000
    .long 4
    .long 3
    .long 0
    .balign 8
    .previous
"#
);

#[cfg(not(feature = "bexos_libc_runtime"))]
use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

const NET_ABI_VERSION: u32 = 1;
const STATUS_OK: i32 = 0;
const STATUS_INVALID_ARGS: i32 = -1;
const STATUS_BUFFER_TOO_SMALL: i32 = -3;

#[cfg(not(feature = "bexos_libc_runtime"))]
struct NoHeap;

#[cfg(not(feature = "bexos_libc_runtime"))]
unsafe impl GlobalAlloc for NoHeap {
    unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
        core::ptr::null_mut()
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[cfg(not(feature = "bexos_libc_runtime"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: NoHeap = NoHeap;

#[cfg(feature = "bexos_libc_runtime")]
#[global_allocator]
static GLOBAL_ALLOCATOR: bexos_libc::Allocator = bexos_libc::Allocator;

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn bexos_net_abi_version() -> u32 {
    NET_ABI_VERSION
}

#[unsafe(no_mangle)]
#[cfg(not(feature = "bexos_libc_runtime"))]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    for offset in 0..n {
        unsafe {
            *dest.add(offset) = *src.add(offset);
        }
    }
    dest
}

#[unsafe(no_mangle)]
#[cfg(not(feature = "bexos_libc_runtime"))]
pub unsafe extern "C" fn memset(dest: *mut u8, value: i32, n: usize) -> *mut u8 {
    for offset in 0..n {
        unsafe {
            *dest.add(offset) = value as u8;
        }
    }
    dest
}

#[unsafe(no_mangle)]
#[cfg(not(feature = "bexos_libc_runtime"))]
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dest as usize) <= (src as usize) {
        unsafe { memcpy(dest, src, n) }
    } else {
        let mut offset = n;
        while offset > 0 {
            offset -= 1;
            unsafe {
                *dest.add(offset) = *src.add(offset);
            }
        }
        dest
    }
}

#[unsafe(no_mangle)]
#[cfg(not(feature = "bexos_libc_runtime"))]
pub extern "C" fn getauxval(_kind: usize) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}

#[unsafe(no_mangle)]
#[cfg(not(feature = "bexos_libc_runtime"))]
pub extern "C" fn _Unwind_Resume() -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_net_http1_encode_get(
    authority_ptr: *const u8,
    authority_len: usize,
    path_ptr: *const u8,
    path_len: usize,
    out_ptr: *mut u8,
    out_len: usize,
    written: *mut usize,
) -> i32 {
    let Some(authority) = (unsafe { checked_slice(authority_ptr, authority_len) }) else {
        return STATUS_INVALID_ARGS;
    };
    let Some(path) = (unsafe { checked_slice(path_ptr, path_len) }) else {
        return STATUS_INVALID_ARGS;
    };
    let Some(out) = (unsafe { checked_mut_slice(out_ptr, out_len) }) else {
        return STATUS_INVALID_ARGS;
    };
    let Some(written) = (unsafe { written.as_mut() }) else {
        return STATUS_INVALID_ARGS;
    };
    if authority.is_empty() || path.first() != Some(&b'/') {
        return STATUS_INVALID_ARGS;
    }
    let needed = 4 + path.len() + 16 + authority.len() + 23;
    if out.len() < needed {
        *written = needed;
        return STATUS_BUFFER_TOO_SMALL;
    }
    let mut cursor = 0;
    cursor = put(out, cursor, b"GET ");
    cursor = put(out, cursor, path);
    cursor = put(out, cursor, b" HTTP/1.1\r\nHost: ");
    cursor = put(out, cursor, authority);
    cursor = put(out, cursor, b"\r\nConnection: close\r\n\r\n");
    *written = cursor;
    STATUS_OK
}

fn put(out: &mut [u8], cursor: usize, bytes: &[u8]) -> usize {
    out[cursor..cursor + bytes.len()].copy_from_slice(bytes);
    cursor + bytes.len()
}

unsafe fn checked_slice<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() && len != 0 {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

unsafe fn checked_mut_slice<'a>(ptr: *mut u8, len: usize) -> Option<&'a mut [u8]> {
    if ptr.is_null() && len != 0 {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) })
}
