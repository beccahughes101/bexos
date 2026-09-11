#![no_std]

extern crate alloc;

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
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

const CRYPTO_ABI_VERSION: u32 = 1;
const STATUS_OK: i32 = 0;
const STATUS_INVALID_ARGS: i32 = -1;
const STATUS_AUTH_FAILED: i32 = -2;

const KEY_LEN_256: usize = 32;
const ED25519_PUBLIC_KEY_LEN: usize = 32;
const ED25519_SIGNATURE_LEN: usize = 64;

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
pub extern "C" fn bexos_crypto_abi_version() -> u32 {
    CRYPTO_ABI_VERSION
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
pub unsafe extern "C" fn bexos_crypto_blake3_256(
    input_ptr: *const u8,
    input_len: usize,
    out_32: *mut u8,
) -> i32 {
    let Some(input) = (unsafe { checked_slice(input_ptr, input_len) }) else {
        return STATUS_INVALID_ARGS;
    };
    let Some(out) = (unsafe { checked_mut_slice(out_32, KEY_LEN_256) }) else {
        return STATUS_INVALID_ARGS;
    };
    out.copy_from_slice(blake3::hash(input).as_bytes());
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_crypto_verify_ed25519(
    public_key_32: *const u8,
    message_ptr: *const u8,
    message_len: usize,
    signature_64: *const u8,
) -> i32 {
    let Some(public_key) = (unsafe { checked_array::<ED25519_PUBLIC_KEY_LEN>(public_key_32) })
    else {
        return STATUS_INVALID_ARGS;
    };
    let Some(message) = (unsafe { checked_slice(message_ptr, message_len) }) else {
        return STATUS_INVALID_ARGS;
    };
    let Some(signature) = (unsafe { checked_array::<ED25519_SIGNATURE_LEN>(signature_64) }) else {
        return STATUS_INVALID_ARGS;
    };
    let Ok(key) = VerifyingKey::from_bytes(&public_key) else {
        return STATUS_INVALID_ARGS;
    };
    let sig = Signature::from_bytes(&signature);
    match key.verify(message, &sig) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_AUTH_FAILED,
    }
}

unsafe fn checked_array<const N: usize>(ptr: *const u8) -> Option<[u8; N]> {
    if ptr.is_null() {
        return None;
    }
    let mut out = [0u8; N];
    out.copy_from_slice(unsafe { core::slice::from_raw_parts(ptr, N) });
    Some(out)
}

unsafe fn checked_slice<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

unsafe fn checked_mut_slice<'a>(ptr: *mut u8, len: usize) -> Option<&'a mut [u8]> {
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) })
}
