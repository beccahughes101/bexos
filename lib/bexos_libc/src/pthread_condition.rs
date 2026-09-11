//! GNU-layout condition variables with allocation-free kernel deadline waits.
use crate::{
    CLOCK_MONOTONIC, CLOCK_REALTIME, EINVAL, EOPNOTSUPP, ETIMEDOUT, Timespec,
    pthread_sync::{Kernel, mutex},
};
use bexos_futex_sync::{self as sync, Wait, condition::Condition};
use core::{
    ffi::{c_int, c_void},
    sync::atomic::{AtomicU32, Ordering},
};

#[repr(C)]
#[derive(Default)]
struct Cond {
    state: Condition,
    clock: AtomicU32,
}
const _: () = assert!(core::mem::size_of::<Cond>() <= 48);

unsafe fn cond<'a>(p: *mut c_void) -> Result<&'a Cond, c_int> {
    if p.is_null() || p.addr() % core::mem::align_of::<Cond>() != 0 {
        return Err(EINVAL);
    }
    Ok(unsafe { &*p.cast::<Cond>() })
}
unsafe fn attr<'a>(p: *const c_void) -> Result<&'a AtomicU32, c_int> {
    if p.is_null() || p.addr() % 4 != 0 {
        return Err(EINVAL);
    }
    Ok(unsafe { &*p.cast::<AtomicU32>() })
}

pub(crate) struct Deadline {
    clock: c_int,
    nanos: i128,
}
impl Deadline {
    pub(crate) fn new(clock: c_int, time: *const c_void) -> Result<Self, c_int> {
        if !matches!(clock, CLOCK_REALTIME | CLOCK_MONOTONIC)
            || time.is_null()
            || time.addr() % core::mem::align_of::<Timespec>() != 0
        {
            return Err(EINVAL);
        }
        let time = unsafe { time.cast::<Timespec>().read() };
        if !(0..1_000_000_000).contains(&time.tv_nsec) {
            return Err(EINVAL);
        }
        Ok(Self {
            clock,
            nanos: time.tv_sec as i128 * 1_000_000_000 + time.tv_nsec as i128,
        })
    }
    fn remaining(&self) -> Result<i64, c_int> {
        let mut now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if crate::clock_gettime(self.clock, (&mut now as *mut Timespec).cast()) != 0 {
            return Err(EINVAL);
        }
        let left = self.nanos - (now.tv_sec as i128 * 1_000_000_000 + now.tv_nsec as i128);
        if left <= 0 {
            Err(ETIMEDOUT)
        } else {
            Ok(left.min(i64::MAX as i128) as i64)
        }
    }
}
impl Wait for Deadline {
    fn wait(&self, word: &AtomicU32, expected: u32) -> Result<(), i32> {
        let result = Kernel.wait_for(word, expected, self.remaining()?);
        // A value transition can race deadline expiry. Let the caller inspect
        // its predicate before timing out; neither event loses the mutex.
        if word.load(Ordering::Acquire) != expected {
            return Ok(());
        }
        result?;
        self.remaining().map(|_| ())
    }
    fn wake(&self, word: &AtomicU32, count: u32) -> Result<(), i32> {
        Kernel.wake(word, count)
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_cond_init(p: *mut c_void, attributes: *const c_void) -> c_int {
    if unsafe { cond(p) }.is_err() {
        return EINVAL;
    }
    let clock = if attributes.is_null() {
        CLOCK_REALTIME as u32
    } else {
        match unsafe { attr(attributes) } {
            Ok(a) => a.load(Ordering::Relaxed),
            Err(e) => return e,
        }
    };
    if clock > CLOCK_MONOTONIC as u32 {
        return EINVAL;
    }
    unsafe {
        core::ptr::write_bytes(p.cast::<u8>(), 0, 48);
        p.cast::<Cond>().write(Cond {
            state: Condition::default(),
            clock: AtomicU32::new(clock),
        });
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_cond_destroy(p: *mut c_void) -> c_int {
    unsafe { cond(p) }
        .and_then(|c| c.state.destroy())
        .map_or_else(|e| e, |_| 0)
}
fn signal(p: *mut c_void, all: bool) -> c_int {
    unsafe { cond(p) }
        .and_then(|c| c.state.signal(all, &Kernel))
        .map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_cond_signal(p: *mut c_void) -> c_int {
    signal(p, false)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_cond_broadcast(p: *mut c_void) -> c_int {
    signal(p, true)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_cond_wait(p: *mut c_void, m: *mut c_void) -> c_int {
    (|| unsafe { cond(p)?.state.wait(mutex(m)?, &Kernel, &Kernel) })().map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_cond_clockwait(
    p: *mut c_void,
    m: *mut c_void,
    clock: c_int,
    time: *const c_void,
) -> c_int {
    (|| {
        let deadline = Deadline::new(clock, time)?;
        unsafe { cond(p)?.state.wait(mutex(m)?, &deadline, &Kernel) }
    })()
    .map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_cond_timedwait(
    p: *mut c_void,
    m: *mut c_void,
    time: *const c_void,
) -> c_int {
    match unsafe { cond(p) } {
        Ok(c) => pthread_cond_clockwait(p, m, c.clock.load(Ordering::Relaxed) as c_int, time),
        Err(e) => e,
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_condattr_init(p: *mut c_void) -> c_int {
    match unsafe { attr(p) } {
        Ok(a) => {
            a.store(0, Ordering::Relaxed);
            0
        }
        Err(e) => e,
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_condattr_destroy(p: *mut c_void) -> c_int {
    unsafe { attr(p) }.map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_condattr_setclock(p: *mut c_void, clock: c_int) -> c_int {
    if !matches!(clock, CLOCK_REALTIME | CLOCK_MONOTONIC) {
        return EINVAL;
    }
    match unsafe { attr(p) } {
        Ok(a) => {
            a.store(clock as u32, Ordering::Relaxed);
            0
        }
        Err(e) => e,
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_condattr_getclock(p: *const c_void, clock: *mut c_int) -> c_int {
    if clock.is_null() || clock.addr() % 4 != 0 {
        return EINVAL;
    }
    match unsafe { attr(p) } {
        Ok(a) => {
            unsafe {
                clock.write(a.load(Ordering::Relaxed) as c_int);
            }
            0
        }
        Err(e) => e,
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_condattr_setpshared(p: *mut c_void, shared: c_int) -> c_int {
    if shared != 0 {
        return if shared == 1 { EOPNOTSUPP } else { EINVAL };
    }
    unsafe { attr(p) }.map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutex_clocklock(
    p: *mut c_void,
    clock: c_int,
    time: *const c_void,
) -> c_int {
    (|| {
        let deadline = Deadline::new(clock, time)?;
        sync::lock(unsafe { mutex(p) }?, &deadline)
    })()
    .map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutex_timedlock(p: *mut c_void, time: *const c_void) -> c_int {
    pthread_mutex_clocklock(p, CLOCK_REALTIME, time)
}
