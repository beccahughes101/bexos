//! C clock-relative and absolute sleeps using scheduler deadlines.
use crate::{CLOCK_MONOTONIC, CLOCK_REALTIME, EINVAL, ETIMEDOUT, pthread_condition::Deadline};
use bexos_futex_sync::Wait;
use core::{
    ffi::{c_int, c_void},
    sync::atomic::AtomicU32,
};

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn clock_nanosleep(
    clock: c_int,
    flags: c_int,
    requested: *const c_void,
    remaining: *mut c_void,
) -> c_int {
    if !matches!(clock, CLOCK_MONOTONIC | CLOCK_REALTIME) || !matches!(flags, 0 | 1) {
        return EINVAL;
    }
    if flags == 0 {
        // Unlike nanosleep, clock_nanosleep returns the error number directly.
        let errno = unsafe { crate::__errno_location().read() };
        let result = crate::nanosleep(requested, remaining);
        let error = if result == 0 {
            0
        } else {
            unsafe { crate::__errno_location().read() }
        };
        unsafe {
            crate::__errno_location().write(errno);
        }
        return error;
    }
    let deadline = match Deadline::new(clock, requested) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let sleeper = AtomicU32::new(0);
    loop {
        match deadline.wait(&sleeper, 0) {
            Ok(()) => {} // A spurious kernel wake never shortens the sleep.
            Err(ETIMEDOUT) => return 0,
            Err(error) => return error,
        }
    }
}
