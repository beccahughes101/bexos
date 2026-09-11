//! Linux-layout plain pthread mutexes and once, backed by actual kernel futexes.
//! Unsupported mutex attributes fail explicitly instead of claiming success.
use bexos_futex_sync as sync;
use core::{
    ffi::{c_int, c_void},
    sync::atomic::{AtomicU32, Ordering},
};
use kernel_fidl::*;
pub(crate) struct Kernel;
impl sync::Wait for Kernel {
    fn wait(&self, word: &AtomicU32, expected: u32) -> Result<(), i32> {
        self.wait_for(word, expected, -1)
    }
    fn wake(&self, word: &AtomicU32, count: u32) -> Result<(), i32> {
        let r: TaskControlFutexWakeResponse = bexos_userspace::ipc::kernel_call_buffered(
            3,
            "FutexWake",
            TASK_CONTROL_PUBLIC_METHODS,
            &TaskControlFutexWakeRequest {
                uaddr: word as *const _ as u64,
                wake_count: count,
            },
            &mut [0; 64],
            &mut [0; 32],
        )
        .map_err(crate::errno_from_kernel)?;
        if r.status == Status::Ok {
            Ok(())
        } else {
            Err(crate::errno_from_kernel(r.status))
        }
    }
}
impl Kernel {
    pub(crate) fn wait_for(
        &self,
        word: &AtomicU32,
        expected: u32,
        timeout_nanos: i64,
    ) -> Result<(), i32> {
        let r: TaskControlFutexWaitResponse = bexos_userspace::ipc::kernel_call_buffered(
            3,
            "FutexWait",
            TASK_CONTROL_PUBLIC_METHODS,
            &TaskControlFutexWaitRequest {
                uaddr: word as *const _ as u64,
                expected_val: expected,
                timeout_nanos,
                owner_thread: HandleRef { raw: 0 },
            },
            &mut [0; 64],
            &mut [0; 32],
        )
        .map_err(crate::errno_from_kernel)?;
        match r.status {
            Status::Ok | Status::ErrResourceExhausted => Ok(()),
            s => Err(crate::errno_from_kernel(s)),
        }
    }
}
unsafe fn word<'a>(p: *mut c_void) -> Result<&'a AtomicU32, c_int> {
    if p.is_null() || (p as usize) & 3 != 0 {
        return Err(crate::EINVAL);
    }
    Ok(unsafe { &*p.cast::<AtomicU32>() })
}
pub(crate) unsafe fn mutex<'a>(p: *mut c_void) -> Result<&'a AtomicU32, c_int> {
    let state = unsafe { word(p) }?;
    // Linux/glibc puts __kind at offset 16 on both supported 64-bit ABIs.
    let kind = unsafe { p.cast::<u32>().add(4).read() };
    if kind != 0 {
        return Err(crate::EOPNOTSUPP);
    }
    Ok(state)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutex_init(p: *mut c_void, attr: *const c_void) -> c_int {
    if unsafe { word(p) }.is_err() {
        return crate::EINVAL;
    }
    if !attr.is_null() {
        if (attr as usize) & 3 != 0 {
            return crate::EINVAL;
        }
        if unsafe { attr.cast::<u32>().read() } != 0 {
            return crate::EOPNOTSUPP;
        }
    }
    let size = if cfg!(target_arch = "aarch64") {
        48
    } else {
        40
    };
    unsafe {
        core::ptr::write_bytes(p.cast::<u8>(), 0, size);
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutex_lock(p: *mut c_void) -> c_int {
    unsafe { mutex(p) }
        .and_then(|s| sync::lock(s, &Kernel))
        .map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutex_trylock(p: *mut c_void) -> c_int {
    unsafe { mutex(p) }
        .and_then(sync::try_lock)
        .map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutex_unlock(p: *mut c_void) -> c_int {
    unsafe { mutex(p) }
        .and_then(|s| sync::unlock(s, &Kernel))
        .map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutex_destroy(p: *mut c_void) -> c_int {
    unsafe { mutex(p) }
        .and_then(sync::destroy)
        .map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutexattr_init(p: *mut c_void) -> c_int {
    match unsafe { word(p) } {
        Ok(s) => {
            s.store(0, Ordering::Relaxed);
            0
        }
        Err(e) => e,
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutexattr_settype(p: *mut c_void, kind: c_int) -> c_int {
    if kind != 0 {
        return crate::EOPNOTSUPP;
    }
    pthread_mutexattr_init(p)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_mutexattr_destroy(p: *mut c_void) -> c_int {
    unsafe { word(p) }.map_or_else(|e| e, |_| 0)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_once(p: *mut c_void, initialize: Option<extern "C" fn()>) -> c_int {
    let Some(initialize) = initialize else {
        return crate::EINVAL;
    };
    unsafe { word(p) }
        .and_then(|s| sync::once(s, &Kernel, || initialize()))
        .map_or_else(|e| e, |_| 0)
}
