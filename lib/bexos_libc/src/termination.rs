//! Bounded C exit callbacks. Callback invocation happens outside the registry
//! lock, so a callback may register another callback during normal shutdown.
use core::ffi::c_int;
static CALLBACKS: crate::Locked<[Option<extern "C" fn()>; 64]> = crate::Locked::new([None; 64]);
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn atexit(callback: Option<extern "C" fn()>) -> c_int {
    let Some(callback) = callback else {
        return -1;
    };
    CALLBACKS.with(|callbacks| {
        let Some(slot) = callbacks.iter_mut().find(|slot| slot.is_none()) else {
            return -1;
        };
        *slot = Some(callback);
        0
    })
}
fn run_callbacks() {
    while let Some(callback) = CALLBACKS.with(|callbacks| {
        callbacks
            .iter_mut()
            .rev()
            .find(|slot| slot.is_some())
            .and_then(Option::take)
    }) {
        callback();
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn exit(status: c_int) -> ! {
    run_callbacks();
    bexos_userspace::syscall::exit_with_status(status)
}
#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};
    static ORDER: AtomicU32 = AtomicU32::new(0);
    extern "C" fn first() {
        assert_eq!(ORDER.fetch_add(1, Ordering::Relaxed), 2);
    }
    extern "C" fn inserted() {
        assert_eq!(ORDER.fetch_add(1, Ordering::Relaxed), 1);
    }
    extern "C" fn last() {
        assert_eq!(ORDER.fetch_add(1, Ordering::Relaxed), 0);
        assert_eq!(atexit(Some(inserted)), 0);
    }
    #[test]
    fn callbacks_reverse_order_and_allow_registration() {
        assert_eq!(atexit(Some(first)), 0);
        assert_eq!(atexit(Some(last)), 0);
        run_callbacks();
        assert_eq!(ORDER.load(Ordering::Relaxed), 3);
    }
}
