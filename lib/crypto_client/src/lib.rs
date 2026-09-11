#![no_std]

extern crate alloc;

use bexos_userspace::{Startup, dynamic_link};
use core::sync::atomic::{AtomicU64, Ordering};

pub const KEY_LEN_256: usize = 32;
pub const AES_GCM_NONCE_LEN: usize = 12;
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;
pub const ED25519_SIGNATURE_LEN: usize = 64;

const ABI_VERSION: u32 = 1;
const STATUS_OK: i32 = 0;
const STATUS_INVALID_ARGS: i32 = -1;
const STATUS_AUTH_FAILED: i32 = -2;
const STATUS_BUFFER_TOO_SMALL: i32 = -3;
const STATUS_INVALID_RESPONSE: i32 = -4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoError {
    InvalidWrappedKey,
    AccessDenied,
    Storage,
    InvalidKey,
    BadSignature,
}

type AbiVersionFn = extern "C" fn() -> u32;
type Blake3Fn = unsafe extern "C" fn(*const u8, usize, *mut u8) -> i32;
type VerifyEd25519Fn = unsafe extern "C" fn(*const u8, *const u8, usize, *const u8) -> i32;
static ABI_VERSION_PTR: AtomicU64 = AtomicU64::new(0);
static BLAKE3_PTR: AtomicU64 = AtomicU64::new(0);
static VERIFY_ED25519_PTR: AtomicU64 = AtomicU64::new(0);

pub fn init_from_startup(startup: &Startup) -> Result<(), CryptoError> {
    let _ = startup;
    init_from_link_map(&[])
}

pub fn init_from_link_map(bytes: &[u8]) -> Result<(), CryptoError> {
    if !bytes.is_empty() {
        dynamic_link::install(bytes).map_err(|_| CryptoError::Storage)?;
    }
    bind_installed_symbols();
    let abi = required::<AbiVersionFn>(&ABI_VERSION_PTR)?();
    if abi != ABI_VERSION {
        return Err(CryptoError::Storage);
    }
    required::<Blake3Fn>(&BLAKE3_PTR)?;
    required::<VerifyEd25519Fn>(&VERIFY_ED25519_PTR)?;
    Ok(())
}

fn bind_installed_symbols() {
    for name in [
        b"bexos_crypto_abi_version".as_slice(),
        b"bexos_crypto_blake3_256".as_slice(),
        b"bexos_crypto_verify_ed25519".as_slice(),
    ] {
        if let Some(address) = dynamic_link::symbol_address(name) {
            bind_symbol(name, address);
        }
    }
}

pub fn blake3_256(bytes: &[u8]) -> [u8; KEY_LEN_256] {
    let func = required::<Blake3Fn>(&BLAKE3_PTR).expect("crypto client initialized");
    let mut out = [0u8; KEY_LEN_256];
    let status = unsafe { func(bytes.as_ptr(), bytes.len(), out.as_mut_ptr()) };
    status_to_crypto(status).expect("blake3 status");
    out
}

pub fn verify_ed25519(
    public_key: &[u8; ED25519_PUBLIC_KEY_LEN],
    message: &[u8],
    signature: &[u8; ED25519_SIGNATURE_LEN],
) -> Result<(), CryptoError> {
    let func = required::<VerifyEd25519Fn>(&VERIFY_ED25519_PTR)?;
    status_to_crypto(unsafe {
        func(
            public_key.as_ptr(),
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
        )
    })
}

fn bind_symbol(name: &[u8], address: u64) {
    let slot = match name {
        b"bexos_crypto_abi_version" => &ABI_VERSION_PTR,
        b"bexos_crypto_blake3_256" => &BLAKE3_PTR,
        b"bexos_crypto_verify_ed25519" => &VERIFY_ED25519_PTR,
        _ => return,
    };
    slot.store(address, Ordering::Release);
}

fn required<F>(slot: &AtomicU64) -> Result<F, CryptoError>
where
    F: Copy,
{
    let address = slot.load(Ordering::Acquire);
    if address == 0 {
        return Err(CryptoError::Storage);
    }
    Ok(unsafe { core::mem::transmute_copy(&address) })
}

fn status_to_crypto(status: i32) -> Result<(), CryptoError> {
    match status {
        STATUS_OK => Ok(()),
        STATUS_INVALID_ARGS | STATUS_BUFFER_TOO_SMALL | STATUS_INVALID_RESPONSE => {
            Err(CryptoError::InvalidWrappedKey)
        }
        STATUS_AUTH_FAILED => Err(CryptoError::AccessDenied),
        _ => Err(CryptoError::Storage),
    }
}
