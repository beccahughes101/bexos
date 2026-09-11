//! BexOS uses explicitly linked libraries, without a dynamic loader.
//! Keep loader probes linkable and report failure through the dlfcn interface.
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

static FALLBACK_ERROR: AtomicBool = AtomicBool::new(false);

fn pending_error(value: bool) -> bool {
    #[cfg(bexos_guest)]
    {
        let local = crate::current_thread_local();
        if !local.is_null() {
            unsafe {
                if (*local).self_ptr != local {
                    crate::initialize_thread_local(local, 1);
                }
                // Reserved TLS slot zero holds the 32-bit errno followed by
                // this 32-bit loader error flag, preserving the TLS ABI.
                let flag = ptr::addr_of_mut!((*local).values[0]).cast::<c_int>().add(1);
                return flag.replace(c_int::from(value)) != 0;
            }
        }
    }
    FALLBACK_ERROR.swap(value, Ordering::Relaxed)
}

fn unsupported() {
    crate::set_errno(crate::ENOSYS);
    pending_error(true);
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn dlopen(_filename: *const c_char, _flags: c_int) -> *mut c_void {
    unsupported();
    ptr::null_mut()
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn dlsym(_handle: *mut c_void, _symbol: *const c_char) -> *mut c_void {
    unsupported();
    ptr::null_mut()
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn dlclose(_handle: *mut c_void) -> c_int {
    unsupported();
    -1
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn dlerror() -> *mut c_char {
    if pending_error(false) {
        c"dynamic loading is not supported".as_ptr().cast_mut()
    } else {
        ptr::null_mut()
    }
}
