//! Private representation inside the pinned 64-bit GNU pthread_attr_t ABI.
//! Stack sizes are honored; unsupported guard requests fail explicitly.
use crate::{EINVAL, EOPNOTSUPP, THREADS, c_int, c_void};
const MAGIC: usize = 0x4245_5841_5454_5231;
const MIN_STACK: usize = 16 * 1024;
const MAX_STACK: usize = 16 * 1024 * 1024;
const ATTR_WORDS: usize = if cfg!(target_arch = "aarch64") { 8 } else { 7 };
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Attributes {
    magic: usize,
    stack_address: usize,
    pub stack_size: usize,
    guard: usize,
    pub detached: usize,
    reserved: [usize; ATTR_WORDS - 5],
}
const _: () = assert!(core::mem::size_of::<Attributes>() == ATTR_WORDS * 8);
impl Default for Attributes {
    fn default() -> Self {
        Self {
            magic: MAGIC,
            stack_address: 0,
            stack_size: 2 * 1024 * 1024,
            guard: 0,
            detached: 0,
            reserved: [0; ATTR_WORDS - 5],
        }
    }
}
fn read(attr: *const c_void) -> Result<Attributes, c_int> {
    if attr.is_null() {
        return Err(EINVAL);
    }
    let value = unsafe { attr.cast::<Attributes>().read() };
    if value.magic != MAGIC
        || !(MIN_STACK..=MAX_STACK).contains(&value.stack_size)
        || !matches!(value.guard, 0 | 4096)
        || value.detached > 1
    {
        return Err(EINVAL);
    }
    Ok(value)
}
pub(crate) fn read_or_default(attr: *const c_void) -> Result<Attributes, c_int> {
    if attr.is_null() {
        Ok(Attributes::default())
    } else {
        let value = read(attr)?;
        if value.stack_address != 0 || value.guard != 0 {
            return Err(EOPNOTSUPP);
        }
        Ok(value)
    }
}
fn update(attr: *mut c_void, change: impl FnOnce(&mut Attributes) -> Result<(), c_int>) -> c_int {
    let mut value = match read(attr) {
        Ok(value) => value,
        Err(e) => return e,
    };
    if let Err(e) = change(&mut value) {
        return e;
    }
    unsafe {
        attr.cast::<Attributes>().write(value);
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_attr_init(attr: *mut c_void) -> c_int {
    if attr.is_null() {
        return EINVAL;
    }
    unsafe {
        attr.cast::<Attributes>().write(Attributes::default());
    }
    0
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_attr_destroy(attr: *mut c_void) -> c_int {
    update(attr, |value| {
        value.magic = 0;
        Ok(())
    })
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_attr_setstacksize(attr: *mut c_void, size: usize) -> c_int {
    update(attr, |value| {
        if !(MIN_STACK..=MAX_STACK).contains(&size) {
            return Err(EINVAL);
        }
        value.stack_size = size;
        Ok(())
    })
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_attr_setguardsize(attr: *mut c_void, size: usize) -> c_int {
    update(attr, |value| {
        if size != 0 {
            return Err(EOPNOTSUPP);
        }
        value.guard = 0;
        Ok(())
    })
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_attr_setdetachstate(attr: *mut c_void, state: c_int) -> c_int {
    update(attr, |value| {
        if !(0..=1).contains(&state) {
            return Err(EINVAL);
        }
        value.detached = state as usize;
        Ok(())
    })
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_attr_getguardsize(attr: *const c_void, guard: *mut usize) -> c_int {
    if guard.is_null() {
        return EINVAL;
    }
    match read(attr) {
        Ok(value) => {
            unsafe {
                guard.write(value.guard);
            }
            0
        }
        Err(e) => e,
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_attr_getstack(
    attr: *const c_void,
    address: *mut *mut c_void,
    size: *mut usize,
) -> c_int {
    if address.is_null() || size.is_null() {
        return EINVAL;
    }
    match read(attr) {
        Ok(value) => {
            unsafe {
                address.write(value.stack_address as *mut c_void);
                size.write(value.stack_size);
            }
            0
        }
        Err(e) => e,
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_getattr_np(thread: usize, attr: *mut c_void) -> c_int {
    if attr.is_null() {
        return EINVAL;
    }
    let value = if thread == 1 {
        Some(Attributes {
            stack_address: (bexos_boot::USER_STACK_TOP - bexos_boot::USER_STACK_SIZE) as usize,
            stack_size: bexos_boot::USER_STACK_SIZE as usize,
            guard: 4096,
            ..Default::default()
        })
    } else {
        THREADS.with(|threads| {
            threads
                .entries
                .iter()
                .flatten()
                .find(|e| e.id == thread && !e.done)
                .map(|e| Attributes {
                    stack_address: e.stack as usize,
                    stack_size: e.stack_size,
                    detached: e.detached as usize,
                    ..Default::default()
                })
        })
    };
    match value {
        Some(value) => {
            unsafe {
                attr.cast::<Attributes>().write(value);
            }
            0
        }
        None => 3, /* ESRCH */
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stack_attributes_preserve_bounds_and_reject_unsupported_guards() {
        let mut storage = [0usize; ATTR_WORDS + 2];
        storage[0] = 7;
        storage[ATTR_WORDS + 1] = 9;
        let attr = storage[1..].as_mut_ptr().cast();
        assert_eq!(pthread_attr_init(attr), 0);
        assert_eq!(pthread_attr_setstacksize(attr, 512 * 1024), 0);
        assert_eq!(pthread_attr_setstacksize(attr, 1), EINVAL);
        assert_eq!(pthread_attr_setguardsize(attr, 4096), EOPNOTSUPP);
        assert_eq!(read(attr).unwrap().stack_size, 512 * 1024);
        assert_eq!(pthread_attr_setdetachstate(attr, 1), 0);
        assert_eq!(read(attr).unwrap().detached, 1);
        assert_eq!((storage[0], storage[ATTR_WORDS + 1]), (7, 9));
        assert_eq!(pthread_attr_destroy(attr), 0);
        assert_eq!(pthread_attr_setstacksize(attr, 64 * 1024), EINVAL);
    }
}
