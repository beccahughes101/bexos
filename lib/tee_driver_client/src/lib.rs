#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use bexos_userspace::dynamic_link;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

pub const ABI_VERSION: u32 = 1;
pub const STATUS_OK: i32 = 0;
pub const STATUS_INVALID_ARGS: i32 = -1;
pub const STATUS_ACCESS_DENIED: i32 = -2;
pub const STATUS_NO_MEMORY: i32 = -3;
pub const STATUS_BUFFER_TOO_SMALL: i32 = -4;
pub const STATUS_PEER_CLOSED: i32 = -5;
pub const STATUS_TIMED_OUT: i32 = -6;
pub const STATUS_ALREADY_EXISTS: i32 = -7;
pub const STATUS_RESOURCE_EXHAUSTED: i32 = -9;
pub const STATUS_NOT_FOUND: i32 = -10;
pub const STATUS_UNAVAILABLE: i32 = -20;
pub const STATUS_VERIFY_FAILED: i32 = -21;

pub const TEE_KIND_SOFTWARE: u32 = 5;
pub const TEE_KIND_TRUSTY: u32 = 6;

pub const OP_PROBE: u32 = 1;
pub const OP_CONNECT: u32 = 2;
pub const OP_CLOSE: u32 = 3;
pub const OP_INVOKE: u32 = 4;
pub const OP_LOAD_APP: u32 = 5;
pub const OP_UNLOAD_APP: u32 = 6;
pub const OP_QUERY_APP: u32 = 7;
pub const OP_TRANSPORT_RESET: u32 = 8;
pub const OP_CORE_STAGE: u32 = 9;
pub const OP_CORE_ACTIVATE: u32 = 10;
pub const OP_CORE_STATUS: u32 = 11;
pub const OP_BUILTIN_DISCOVERY: u32 = 12;
/// Receive one message from an already-connected TIPC session without first
/// sending a request. This is used by normal-world proxy services whose Trusty
/// peer initiates each transaction.
pub const OP_RECV: u32 = 13;
/// Send one message on an already-connected TIPC session without waiting for a
/// response.
pub const OP_SEND: u32 = 14;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeDriverInfo {
    pub present: u32,
    pub kind: u32,
    pub secure_os_version: u32,
    pub anti_rollback_version: u32,
    pub abi_version: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeEndpoint {
    pub uuid: [u8; 16],
    pub package_id_ptr: *const u8,
    pub package_id_len: usize,
    pub port_ptr: *const u8,
    pub port_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeBuffer {
    pub ptr: *const u8,
    pub len: usize,
    pub vmo: u64,
    pub physical: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeOutBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub written: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeAppDescriptor {
    pub provider: u32,
    pub uuid: [u8; 16],
    pub version: u64,
    pub package_id_ptr: *const u8,
    pub package_id_len: usize,
    pub ports_ptr: *const u8,
    pub ports_len: usize,
    pub protected_policy: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeCoreImage {
    pub generation: u64,
    pub activation: u32,
    pub target_ptr: *const u8,
    pub target_len: usize,
    pub hash: [u8; 32],
    pub image: TeeBuffer,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeUpdateState {
    pub status: u32,
    pub phase: u32,
    pub active_slot: [u8; 16],
    pub pending_slot: [u8; 16],
    pub generation: u64,
    pub rollback_available: u32,
    pub reboot_required: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TeeDriverRequest {
    pub op: u32,
    pub flags: u32,
    pub request_id: u64,
    pub session_id: u64,
    pub command_id: u32,
    pub endpoint: TeeEndpoint,
    pub app: TeeAppDescriptor,
    pub input: TeeBuffer,
    pub output: TeeOutBuffer,
    pub core: TeeCoreImage,
}

impl Default for TeeDriverRequest {
    fn default() -> Self {
        Self {
            op: 0,
            flags: 0,
            request_id: 0,
            session_id: 0,
            command_id: 0,
            endpoint: TeeEndpoint::default(),
            app: TeeAppDescriptor::default(),
            input: TeeBuffer::default(),
            output: TeeOutBuffer::default(),
            core: TeeCoreImage::default(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct TeeDriverCompletion {
    pub request_id: u64,
    pub status: i32,
    pub session_id: u64,
    pub value: u64,
    pub info: TeeDriverInfo,
    pub update: TeeUpdateState,
}

type AbiVersionFn = extern "C" fn() -> u32;
type CreateFn = unsafe extern "C" fn(*mut *mut c_void) -> i32;
type DestroyFn = unsafe extern "C" fn(*mut c_void);
type SubmitFn = unsafe extern "C" fn(*mut c_void, *const TeeDriverRequest) -> i32;
type PollFn = unsafe extern "C" fn(*mut c_void, *mut TeeDriverCompletion) -> i32;
type CancelFn = unsafe extern "C" fn(*mut c_void, u64) -> i32;

static ABI_VERSION_PTR: AtomicU64 = AtomicU64::new(0);
static CREATE_PTR: AtomicU64 = AtomicU64::new(0);
static DESTROY_PTR: AtomicU64 = AtomicU64::new(0);
static SUBMIT_PTR: AtomicU64 = AtomicU64::new(0);
static POLL_PTR: AtomicU64 = AtomicU64::new(0);
static CANCEL_PTR: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeeDriverError {
    Storage,
    AbiMismatch,
    MissingSymbol,
    Driver(i32),
}

pub struct Driver {
    ctx: *mut c_void,
}

impl Driver {
    pub fn load(linker_data: &[u8]) -> Result<Self, TeeDriverError> {
        init_from_link_map(linker_data)?;
        let mut ctx = core::ptr::null_mut();
        let status = unsafe { required::<CreateFn>(&CREATE_PTR)?(&mut ctx) };
        if status != STATUS_OK {
            return Err(TeeDriverError::Driver(status));
        }
        if ctx.is_null() {
            return Err(TeeDriverError::Driver(STATUS_INVALID_ARGS));
        }
        Ok(Self { ctx })
    }

    pub fn submit(&mut self, request: &TeeDriverRequest) -> Result<(), TeeDriverError> {
        let status = unsafe { required::<SubmitFn>(&SUBMIT_PTR)?(self.ctx, request) };
        if status == STATUS_OK {
            Ok(())
        } else {
            Err(TeeDriverError::Driver(status))
        }
    }

    pub fn poll(&mut self) -> Result<Option<TeeDriverCompletion>, TeeDriverError> {
        let mut completion = TeeDriverCompletion::default();
        let status = unsafe { required::<PollFn>(&POLL_PTR)?(self.ctx, &mut completion) };
        match status {
            STATUS_OK => Ok(Some(completion)),
            STATUS_TIMED_OUT => Ok(None),
            other => Err(TeeDriverError::Driver(other)),
        }
    }

    pub fn cancel(&mut self, request_id: u64) -> Result<(), TeeDriverError> {
        let status = unsafe { required::<CancelFn>(&CANCEL_PTR)?(self.ctx, request_id) };
        if status == STATUS_OK {
            Ok(())
        } else {
            Err(TeeDriverError::Driver(status))
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        if let Ok(destroy) = required::<DestroyFn>(&DESTROY_PTR) {
            unsafe { destroy(self.ctx) };
        }
    }
}

pub fn init_from_link_map(bytes: &[u8]) -> Result<(), TeeDriverError> {
    reset_bindings();
    if !bytes.is_empty() {
        dynamic_link::install(bytes).map_err(|_| TeeDriverError::Storage)?;
    }
    for name in [
        b"bexos_tee_driver_abi_version".as_slice(),
        b"bexos_tee_driver_context_create".as_slice(),
        b"bexos_tee_driver_context_destroy".as_slice(),
        b"bexos_tee_driver_submit".as_slice(),
        b"bexos_tee_driver_poll".as_slice(),
        b"bexos_tee_driver_cancel".as_slice(),
    ] {
        if let Some(address) = dynamic_link::symbol_address(name) {
            bind_symbol(name, address);
        }
    }
    let abi = required::<AbiVersionFn>(&ABI_VERSION_PTR)?();
    if abi != ABI_VERSION {
        return Err(TeeDriverError::AbiMismatch);
    }
    required::<CreateFn>(&CREATE_PTR)?;
    required::<DestroyFn>(&DESTROY_PTR)?;
    required::<SubmitFn>(&SUBMIT_PTR)?;
    required::<PollFn>(&POLL_PTR)?;
    required::<CancelFn>(&CANCEL_PTR)?;
    Ok(())
}

fn reset_bindings() {
    ABI_VERSION_PTR.store(0, Ordering::Relaxed);
    CREATE_PTR.store(0, Ordering::Relaxed);
    DESTROY_PTR.store(0, Ordering::Relaxed);
    SUBMIT_PTR.store(0, Ordering::Relaxed);
    POLL_PTR.store(0, Ordering::Relaxed);
    CANCEL_PTR.store(0, Ordering::Relaxed);
}

fn bind_symbol(name: &[u8], address: u64) {
    match name {
        b"bexos_tee_driver_abi_version" => ABI_VERSION_PTR.store(address, Ordering::Relaxed),
        b"bexos_tee_driver_context_create" => CREATE_PTR.store(address, Ordering::Relaxed),
        b"bexos_tee_driver_context_destroy" => DESTROY_PTR.store(address, Ordering::Relaxed),
        b"bexos_tee_driver_submit" => SUBMIT_PTR.store(address, Ordering::Relaxed),
        b"bexos_tee_driver_poll" => POLL_PTR.store(address, Ordering::Relaxed),
        b"bexos_tee_driver_cancel" => CANCEL_PTR.store(address, Ordering::Relaxed),
        _ => {}
    }
}

fn required<T>(slot: &AtomicU64) -> Result<T, TeeDriverError>
where
    T: Copy,
{
    let address = slot.load(Ordering::Relaxed);
    if address == 0 {
        return Err(TeeDriverError::MissingSymbol);
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
        .unwrap_or_else(|| vec![])
}

/// Install a process-local storage-proxy handler. The callback remains owned by
/// teed and must be reinstalled after driver reload or heart transplant.
pub const OP_STORAGE_PROXY_HANDLER: u32 = 15;
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StorageProxyHandler {
    pub context: *mut c_void,
    pub dispatch:
        unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut u8, usize, *mut usize) -> i32,
}
