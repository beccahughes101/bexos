//! Verify sleeping C condition variables, deadlines, and mutex reacquisition
//! on the real guest scheduler before renderer workers depend on them.
use core::{
    cell::UnsafeCell,
    ffi::c_void,
    sync::atomic::{AtomicU32, Ordering},
};
unsafe extern "C" {
    fn pthread_mutex_lock(p: *mut c_void) -> i32;
    fn pthread_mutex_trylock(p: *mut c_void) -> i32;
    fn pthread_mutex_unlock(p: *mut c_void) -> i32;
    fn pthread_mutex_destroy(p: *mut c_void) -> i32;
    fn pthread_cond_wait(p: *mut c_void, m: *mut c_void) -> i32;
    fn pthread_cond_clockwait(
        p: *mut c_void,
        m: *mut c_void,
        clock: i32,
        time: *const Timespec,
    ) -> i32;
    fn pthread_cond_signal(p: *mut c_void) -> i32;
    fn pthread_cond_broadcast(p: *mut c_void) -> i32;
    fn pthread_cond_destroy(p: *mut c_void) -> i32;
    fn pthread_create(
        id: *mut usize,
        attr: *const c_void,
        f: extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> i32;
    fn pthread_join(id: usize, result: *mut *mut c_void) -> i32;
    fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
}
#[repr(C)]
struct Timespec {
    sec: i64,
    nanos: i64,
}
#[repr(C, align(8))]
struct Storage(UnsafeCell<[u8; 48]>);
unsafe impl Sync for Storage {}
static MUTEX: Storage = Storage(UnsafeCell::new([0; 48]));
static CONDITION: Storage = Storage(UnsafeCell::new([0; 48]));
static REGISTERED: AtomicU32 = AtomicU32::new(0);
static TOKENS: AtomicU32 = AtomicU32::new(0);
static COMPLETED: AtomicU32 = AtomicU32::new(0);
extern "C" fn worker(_: *mut c_void) -> *mut c_void {
    unsafe {
        assert_eq!(pthread_mutex_lock(MUTEX.0.get().cast()), 0);
        REGISTERED.fetch_add(1, Ordering::Relaxed);
        while TOKENS.load(Ordering::Relaxed) == 0 {
            assert_eq!(
                pthread_cond_wait(CONDITION.0.get().cast(), MUTEX.0.get().cast()),
                0
            );
        }
        TOKENS.fetch_sub(1, Ordering::Relaxed);
        assert_eq!(pthread_mutex_trylock(MUTEX.0.get().cast()), 16);
        COMPLETED.fetch_add(1, Ordering::Release);
        assert_eq!(pthread_mutex_unlock(MUTEX.0.get().cast()), 0);
    }
    core::ptr::null_mut()
}
fn now() -> u64 {
    let mut time = Timespec { sec: 0, nanos: 0 };
    assert_eq!(unsafe { clock_gettime(1, &mut time) }, 0);
    time.sec as u64 * 1_000_000_000 + time.nanos as u64
}
pub fn verify() {
    crate::allocation_probe::verify();
    #[cfg(bexos_guest)]
    unsafe {
        unsafe extern "C" {
            fn bexos_c_format_probe() -> i32;
        }
        assert_eq!(bexos_c_format_probe(), 0);
        bexos_userspace::log("input-fixture: C formatting and allocation alignment verified\n");
    }
    let mut threads = [0; 4];
    for id in &mut threads {
        assert_eq!(
            unsafe { pthread_create(id, core::ptr::null(), worker, core::ptr::null_mut()) },
            0
        );
    }
    let limit = now() + 5_000_000_000;
    loop {
        assert!(now() < limit, "condition workers did not register");
        assert_eq!(unsafe { pthread_mutex_lock(MUTEX.0.get().cast()) }, 0);
        if REGISTERED.load(Ordering::Relaxed) == 4 {
            break;
        }
        assert_eq!(unsafe { pthread_mutex_unlock(MUTEX.0.get().cast()) }, 0);
        bexos_userspace::yield_now();
    }
    unsafe {
        assert_eq!(pthread_cond_destroy(CONDITION.0.get().cast()), 16);
        TOKENS.store(1, Ordering::Relaxed);
        assert_eq!(pthread_cond_signal(CONDITION.0.get().cast()), 0);
        assert_eq!(pthread_mutex_unlock(MUTEX.0.get().cast()), 0);
    }
    while COMPLETED.load(Ordering::Acquire) == 0 {
        assert!(now() < limit, "condition signal did not wake a worker");
        bexos_userspace::yield_now();
    }
    unsafe {
        assert_eq!(pthread_mutex_lock(MUTEX.0.get().cast()), 0);
        TOKENS.store(3, Ordering::Relaxed);
        assert_eq!(pthread_cond_broadcast(CONDITION.0.get().cast()), 0);
        assert_eq!(pthread_mutex_unlock(MUTEX.0.get().cast()), 0);
    }
    for id in threads {
        assert_eq!(unsafe { pthread_join(id, core::ptr::null_mut()) }, 0);
    }
    assert_eq!(COMPLETED.load(Ordering::Acquire), 4);
    unsafe {
        assert_eq!(pthread_mutex_lock(MUTEX.0.get().cast()), 0);
        let deadline = now() + 5_000_000;
        let time = Timespec {
            sec: (deadline / 1_000_000_000) as i64,
            nanos: (deadline % 1_000_000_000) as i64,
        };
        assert_eq!(
            pthread_cond_clockwait(CONDITION.0.get().cast(), MUTEX.0.get().cast(), 1, &time),
            110
        );
        assert!(now() >= deadline);
        assert_eq!(pthread_mutex_trylock(MUTEX.0.get().cast()), 16);
        assert_eq!(pthread_mutex_unlock(MUTEX.0.get().cast()), 0);
        assert_eq!(pthread_cond_destroy(CONDITION.0.get().cast()), 0);
        assert_eq!(pthread_mutex_destroy(MUTEX.0.get().cast()), 0);
    }
    bexos_userspace::log(
        "input-fixture: C condition signal, broadcast and sleeping deadlines verified\n",
    );
}
