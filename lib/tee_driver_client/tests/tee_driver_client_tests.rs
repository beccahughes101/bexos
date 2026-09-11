use bexos_tee_driver_client::{
    ABI_VERSION, Driver, STATUS_OK, STATUS_TIMED_OUT, TeeDriverCompletion, TeeDriverError,
    TeeDriverRequest, test_link_map,
};
use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static TEST_LOCK: Mutex<()> = Mutex::new(());

extern "C" fn abi_version() -> u32 {
    ABI_VERSION
}

extern "C" fn bad_abi_version() -> u32 {
    ABI_VERSION + 1
}

unsafe extern "C" fn create(ctx: *mut *mut c_void) -> i32 {
    unsafe {
        *ctx = std::ptr::dangling_mut::<c_void>();
    }
    STATUS_OK
}

unsafe extern "C" fn destroy(_ctx: *mut c_void) {}

unsafe extern "C" fn submit(_ctx: *mut c_void, request: *const TeeDriverRequest) -> i32 {
    let request = unsafe { request.as_ref() }.unwrap();
    SUBMITTED.store(request.request_id, Ordering::Relaxed);
    STATUS_OK
}

unsafe extern "C" fn poll(_ctx: *mut c_void, completion: *mut TeeDriverCompletion) -> i32 {
    unsafe {
        (*completion).request_id = SUBMITTED.load(Ordering::Relaxed);
    }
    STATUS_OK
}

unsafe extern "C" fn cancel(_ctx: *mut c_void, request_id: u64) -> i32 {
    SUBMITTED.store(request_id, Ordering::Relaxed);
    STATUS_OK
}

unsafe extern "C" fn no_completion(
    _ctx: *mut c_void,
    _completion: *mut TeeDriverCompletion,
) -> i32 {
    STATUS_TIMED_OUT
}

fn addr<T>(function: T) -> u64
where
    T: Copy,
{
    unsafe { core::mem::transmute_copy::<T, usize>(&function) as u64 }
}

fn symbols(poll_fn: u64, abi: u64) -> Vec<(&'static [u8], u64)> {
    vec![
        (b"bexos_tee_driver_abi_version".as_slice(), abi),
        (
            b"bexos_tee_driver_context_create".as_slice(),
            addr(create as unsafe extern "C" fn(*mut *mut c_void) -> i32),
        ),
        (
            b"bexos_tee_driver_context_destroy".as_slice(),
            addr(destroy as unsafe extern "C" fn(*mut c_void)),
        ),
        (
            b"bexos_tee_driver_submit".as_slice(),
            addr(submit as unsafe extern "C" fn(*mut c_void, *const TeeDriverRequest) -> i32),
        ),
        (b"bexos_tee_driver_poll".as_slice(), poll_fn),
        (
            b"bexos_tee_driver_cancel".as_slice(),
            addr(cancel as unsafe extern "C" fn(*mut c_void, u64) -> i32),
        ),
    ]
}

#[test]
fn driver_binds_submits_polls_and_cancels() {
    let _guard = TEST_LOCK.lock().unwrap();
    let map = test_link_map(&symbols(
        addr(poll as unsafe extern "C" fn(*mut c_void, *mut TeeDriverCompletion) -> i32),
        addr(abi_version as extern "C" fn() -> u32),
    ));
    let mut driver = Driver::load(&map).unwrap();
    driver
        .submit(&TeeDriverRequest {
            request_id: 42,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(driver.poll().unwrap().unwrap().request_id, 42);
    driver.cancel(99).unwrap();
    assert_eq!(SUBMITTED.load(Ordering::Relaxed), 99);
}

#[test]
fn poll_timeout_is_not_a_completion() {
    let _guard = TEST_LOCK.lock().unwrap();
    let map = test_link_map(&symbols(
        addr(no_completion as unsafe extern "C" fn(*mut c_void, *mut TeeDriverCompletion) -> i32),
        addr(abi_version as extern "C" fn() -> u32),
    ));
    let mut driver = Driver::load(&map).unwrap();
    assert!(driver.poll().unwrap().is_none());
}

#[test]
fn abi_mismatch_fails_closed() {
    let _guard = TEST_LOCK.lock().unwrap();
    let map = test_link_map(&symbols(
        addr(poll as unsafe extern "C" fn(*mut c_void, *mut TeeDriverCompletion) -> i32),
        addr(bad_abi_version as extern "C" fn() -> u32),
    ));
    assert_eq!(Driver::load(&map).err(), Some(TeeDriverError::AbiMismatch));
}
