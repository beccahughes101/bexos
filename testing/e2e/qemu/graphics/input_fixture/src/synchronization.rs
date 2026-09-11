//! Exercise the C platform mutex/once ABI on actual guest threads before Mesa
//! depends on it. Threads finish before this fixture becomes transplantable.
use core::{
    cell::UnsafeCell,
    ffi::c_void,
    sync::atomic::{AtomicU32, Ordering},
};
unsafe extern "C" {
    fn pthread_mutex_init(p: *mut c_void, attr: *const c_void) -> i32;
    fn pthread_mutex_lock(p: *mut c_void) -> i32;
    fn pthread_mutex_unlock(p: *mut c_void) -> i32;
    fn pthread_mutex_destroy(p: *mut c_void) -> i32;
    fn pthread_once(p: *mut c_void, f: extern "C" fn()) -> i32;
    fn pthread_create(
        thread: *mut usize,
        attr: *const c_void,
        f: extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> i32;
    fn pthread_join(thread: usize, value: *mut *mut c_void) -> i32;
    fn pthread_key_create(key: *mut u32, destructor: Option<extern "C" fn(*mut c_void)>) -> i32;
    fn pthread_key_delete(key: u32) -> i32;
    fn pthread_setspecific(key: u32, value: *const c_void) -> i32;
    fn pthread_getspecific(key: u32) -> *mut c_void;
    fn __errno_location() -> *mut i32;
}
#[repr(C, align(8))]
struct Mutex(UnsafeCell<[u8; 48]>);
unsafe impl Sync for Mutex {}
static MUTEX: Mutex = Mutex(UnsafeCell::new([0; 48]));
static ONCE: AtomicU32 = AtomicU32::new(0);
static INITIALIZED: AtomicU32 = AtomicU32::new(0);
static COUNT: AtomicU32 = AtomicU32::new(0);
static KEY: AtomicU32 = AtomicU32::new(0);
static DESTROYED: AtomicU32 = AtomicU32::new(0);
extern "C" fn destroy(value: *mut c_void) {
    assert!((1..=4).contains(&value.addr()));
    DESTROYED.fetch_add(1, Ordering::Relaxed);
}
extern "C" fn initialize() {
    INITIALIZED.fetch_add(1, Ordering::Relaxed);
    bexos_userspace::yield_now();
}
extern "C" fn worker(value: *mut c_void) -> *mut c_void {
    let key = KEY.load(Ordering::Acquire);
    unsafe {
        assert_eq!(pthread_once(ONCE.as_ptr().cast(), initialize), 0);
        assert!(pthread_getspecific(key).is_null());
        assert_eq!(pthread_setspecific(key, value), 0);
        *__errno_location() = value.addr() as i32;
    }
    for _ in 0..64 {
        unsafe {
            assert_eq!(pthread_mutex_lock(MUTEX.0.get().cast()), 0);
        }
        let count = COUNT.load(Ordering::Relaxed);
        bexos_userspace::yield_now();
        unsafe {
            assert_eq!(pthread_getspecific(key), value);
            assert_eq!(*__errno_location(), value.addr() as i32);
        }
        COUNT.store(count + 1, Ordering::Relaxed);
        unsafe {
            assert_eq!(pthread_mutex_unlock(MUTEX.0.get().cast()), 0);
        }
    }
    core::ptr::null_mut()
}
pub fn verify() {
    bexos_userspace::log("input-fixture: starting C synchronization checks\n");
    let mut guarded_key = [0u32, 0xdeadbeef];
    unsafe {
        assert_eq!(
            pthread_mutex_init(MUTEX.0.get().cast(), core::ptr::null()),
            0
        );
        assert_eq!(
            pthread_key_create(guarded_key.as_mut_ptr(), Some(destroy)),
            0
        );
        assert_eq!(guarded_key[1], 0xdeadbeef);
        KEY.store(guarded_key[0], Ordering::Release);
        *__errno_location() = 77;
    }
    let mut threads = [0usize; 4];
    for (index, id) in threads.iter_mut().enumerate() {
        unsafe {
            assert_eq!(
                pthread_create(id, core::ptr::null(), worker, (index + 1) as *mut c_void),
                0
            );
        }
    }
    bexos_userspace::log("input-fixture: C synchronization workers started\n");
    for id in threads {
        unsafe {
            assert_eq!(pthread_join(id, core::ptr::null_mut()), 0);
        }
    }
    assert_eq!(COUNT.load(Ordering::Relaxed), 256);
    assert_eq!(INITIALIZED.load(Ordering::Relaxed), 1);
    assert_eq!(DESTROYED.load(Ordering::Relaxed), 4);
    unsafe {
        assert_eq!(*__errno_location(), 77);
        assert!(pthread_getspecific(guarded_key[0]).is_null());
        assert_eq!(pthread_key_delete(guarded_key[0]), 0);
        assert_eq!(pthread_mutex_destroy(MUTEX.0.get().cast()), 0);
    }
    bexos_userspace::log("input-fixture: C mutex contention and once verified\n");
    bexos_userspace::log("input-fixture: C key ABI, thread-local errno and destructors verified\n");
}
