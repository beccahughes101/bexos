//! GNU pthread keys are 32-bit tokens. A generation prevents deleted keys from
//! exposing stale values when a slot is reused. TLS keeps the existing 528-byte
//! executable ABI; one small value record is allocated per used thread/slot.
use crate::{EAGAIN, EINVAL, ENOMEM, Locked};
use core::alloc::Layout;
use core::{
    ffi::{c_int, c_void},
    ptr,
};

type Destructor = Option<extern "C" fn(*mut c_void)>;
#[derive(Clone, Copy)]
struct Key {
    token: u32,
    destructor: Destructor,
    active: bool,
}
static KEYS: Locked<[Key; 64]> = Locked::new(
    [Key {
        token: 0,
        destructor: None,
        active: false,
    }; 64],
);
struct Value {
    token: u32,
    value: *mut c_void,
}

fn destructor(key: u32) -> Option<Destructor> {
    KEYS.with(|keys| {
        let slot = &keys[(key & 63) as usize];
        (slot.active && slot.token == key).then_some(slot.destructor)
    })
}
fn get_slot(slot: usize) -> *mut Value {
    #[cfg(not(bexos_guest))]
    {
        crate::HOST_PTHREAD_VALUES[slot].load(core::sync::atomic::Ordering::Relaxed) as *mut Value
    }
    #[cfg(bexos_guest)]
    {
        let local = crate::ensure_initial_thread_local();
        if local.is_null() {
            ptr::null_mut()
        } else {
            unsafe { (*local).values[slot] as *mut Value }
        }
    }
}
fn set_slot(slot: usize, value: *mut Value) -> Result<(), c_int> {
    #[cfg(not(bexos_guest))]
    {
        crate::HOST_PTHREAD_VALUES[slot]
            .store(value as usize, core::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(bexos_guest)]
    {
        let local = crate::ensure_initial_thread_local();
        if local.is_null() {
            return Err(ENOMEM);
        }
        unsafe {
            (*local).values[slot] = value as usize;
        }
    }
    Ok(())
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_key_create(out: *mut u32, destructor: Destructor) -> c_int {
    if out.is_null() || out.addr() & 3 != 0 {
        return EINVAL;
    }
    KEYS.with(|keys| {
        for (index, key) in keys.iter_mut().enumerate().skip(1) {
            if key.active {
                continue;
            }
            let token = if key.token == 0 {
                index as u32
            } else {
                let Some(token) = key.token.checked_add(64) else {
                    continue;
                };
                token
            };
            *key = Key {
                token,
                destructor,
                active: true,
            };
            unsafe {
                out.write(token);
            }
            return 0;
        }
        EAGAIN
    })
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_key_delete(key: u32) -> c_int {
    KEYS.with(|keys| {
        let slot = &mut keys[(key & 63) as usize];
        if !slot.active || slot.token != key {
            return EINVAL;
        }
        slot.active = false;
        slot.destructor = None;
        0
    })
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_setspecific(key: u32, value: *const c_void) -> c_int {
    if destructor(key).is_none() {
        return EINVAL;
    }
    let slot = (key & 63) as usize;
    let mut record = get_slot(slot);
    if record.is_null() {
        if value.is_null() {
            return 0;
        }
        record = unsafe { alloc::alloc::alloc(Layout::new::<Value>()).cast() };
        if record.is_null() {
            return ENOMEM;
        }
        if let Err(error) = set_slot(slot, record) {
            unsafe {
                alloc::alloc::dealloc(record.cast(), Layout::new::<Value>());
            }
            return error;
        }
    }
    unsafe {
        record.write(Value {
            token: key,
            value: value.cast_mut(),
        });
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_getspecific(key: u32) -> *mut c_void {
    if destructor(key).is_none() {
        return ptr::null_mut();
    }
    let record = get_slot((key & 63) as usize);
    if record.is_null() || unsafe { (*record).token != key } {
        return ptr::null_mut();
    }
    unsafe { (*record).value }
}

pub(crate) fn finish_thread() {
    // POSIX permits a destructor to install another value; give it the
    // required four passes, without holding the key registry lock.
    for _ in 0..4 {
        let mut called = false;
        for slot in 1..64 {
            let record = get_slot(slot);
            if record.is_null() {
                continue;
            }
            let (token, value) = unsafe { ((*record).token, (*record).value) };
            unsafe {
                (*record).value = ptr::null_mut();
            }
            if !value.is_null() {
                if let Some(Some(destroy)) = destructor(token) {
                    destroy(value);
                    called = true;
                }
            }
        }
        if !called {
            break;
        }
    }
    for slot in 1..64 {
        let record = get_slot(slot);
        let _ = set_slot(slot, ptr::null_mut());
        if !record.is_null() {
            unsafe {
                alloc::alloc::dealloc(record.cast(), Layout::new::<Value>());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};
    static CALLS: AtomicU32 = AtomicU32::new(0);
    extern "C" fn repeat(value: *mut c_void) {
        CALLS.fetch_add(1, Ordering::Relaxed);
        assert_eq!(pthread_setspecific(value.addr() as u32, value), 0);
    }
    #[test]
    fn destructor_rearming_is_bounded() {
        let mut key = 0;
        assert_eq!(pthread_key_create(&mut key, Some(repeat)), 0);
        assert_eq!(pthread_setspecific(key, key as usize as *const c_void), 0);
        finish_thread();
        assert_eq!(CALLS.load(Ordering::Relaxed), 4);
        assert!(pthread_getspecific(key).is_null());
        assert_eq!(pthread_key_delete(key), 0);
    }
}
