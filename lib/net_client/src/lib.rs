#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use bexos_userspace::{Channel, Startup, dynamic_link};
use core::sync::atomic::{AtomicU64, Ordering};

const ABI_VERSION: u32 = 1;
const STATUS_OK: i32 = 0;
const STATUS_BUFFER_TOO_SMALL: i32 = -3;

#[derive(Clone, Copy, Default)]
pub struct NetworkServices {
    pub netstack: Option<Channel>,
    pub tls_trust: Option<Channel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetClientError {
    InvalidArgs,
    BufferTooSmall,
    Storage,
    MissingService,
}

type AbiVersionFn = extern "C" fn() -> u32;
type Http1EncodeGetFn =
    unsafe extern "C" fn(*const u8, usize, *const u8, usize, *mut u8, usize, *mut usize) -> i32;

static ABI_VERSION_PTR: AtomicU64 = AtomicU64::new(0);
static HTTP1_ENCODE_GET_PTR: AtomicU64 = AtomicU64::new(0);

pub fn services_from_startup(startup: &Startup) -> NetworkServices {
    let mut services = NetworkServices::default();
    for grant in &startup.service_grants {
        match grant.service.as_str() {
            "bexos.net.Netstack" => services.netstack = Some(Channel(grant.endpoint)),
            "bexos.security.trust.TlsTrustManager" => {
                services.tls_trust = Some(Channel(grant.endpoint))
            }
            _ => {}
        }
    }
    services
}

pub fn init_from_startup(startup: &Startup) -> Result<NetworkServices, NetClientError> {
    let services = services_from_startup(startup);
    init_from_link_map(&[]).map(|_| services)
}

pub fn init_from_link_map(bytes: &[u8]) -> Result<(), NetClientError> {
    if !bytes.is_empty() {
        dynamic_link::install(bytes).map_err(|_| NetClientError::Storage)?;
    }
    for name in [
        b"bexos_net_abi_version".as_slice(),
        b"bexos_net_http1_encode_get".as_slice(),
    ] {
        if let Some(address) = dynamic_link::symbol_address(name) {
            bind_symbol(name, address);
        }
    }
    let abi = required::<AbiVersionFn>(&ABI_VERSION_PTR)?();
    if abi != ABI_VERSION {
        return Err(NetClientError::Storage);
    }
    required::<Http1EncodeGetFn>(&HTTP1_ENCODE_GET_PTR)?;
    Ok(())
}

pub fn encode_http1_get(authority: &str, path: &str) -> Result<Vec<u8>, NetClientError> {
    let func = required::<Http1EncodeGetFn>(&HTTP1_ENCODE_GET_PTR)?;
    let mut out = vec![0; 2048];
    let mut written = 0usize;
    let status = unsafe {
        func(
            authority.as_ptr(),
            authority.len(),
            path.as_ptr(),
            path.len(),
            out.as_mut_ptr(),
            out.len(),
            &mut written,
        )
    };
    match status {
        STATUS_OK => {
            out.truncate(written);
            Ok(out)
        }
        STATUS_BUFFER_TOO_SMALL => Err(NetClientError::BufferTooSmall),
        _ => Err(NetClientError::InvalidArgs),
    }
}

fn bind_symbol(name: &[u8], address: u64) {
    match name {
        b"bexos_net_abi_version" => ABI_VERSION_PTR.store(address, Ordering::Relaxed),
        b"bexos_net_http1_encode_get" => HTTP1_ENCODE_GET_PTR.store(address, Ordering::Relaxed),
        _ => {}
    }
}

fn required<T>(slot: &AtomicU64) -> Result<T, NetClientError>
where
    T: Copy,
{
    let address = slot.load(Ordering::Relaxed);
    if address == 0 {
        return Err(NetClientError::Storage);
    }
    Ok(unsafe { core::mem::transmute_copy(&address) })
}

pub fn test_link_map(symbols: &[(&[u8], u64)]) -> Vec<u8> {
    let owned: Vec<_> = symbols
        .iter()
        .map(|(name, address)| dynamic_link::EncodedSymbol {
            name: core::str::from_utf8(name).unwrap_or(""),
            address: *address,
        })
        .collect();
    dynamic_link::encode_linker_data_v3(&owned, &[], &[], 0, 1, dynamic_link::ARCHITECTURE_ID, &[])
        .unwrap()
}
