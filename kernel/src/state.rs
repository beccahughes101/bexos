use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use bexos_kernel_core::kernel_services::ControlPlane;

use crate::early_uart::EarlyUart;

/// A small spin lock for state reachable from exception and secondary-CPU
/// paths.  EL1 exception entry masks IRQs, so a holder cannot be interrupted
/// locally by another user of the same global; the atomic lock serializes the
/// remaining CPUs.
pub struct Global<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T> Sync for Global<T> {}

impl<T> Global<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        struct Unlock<'a>(&'a AtomicBool);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let unlock = Unlock(&self.locked);
        let value = unsafe { &mut *self.value.get() };
        let result = f(value);
        drop(unlock);
        result
    }

    /// Replace a global during early single-core initialization.
    ///
    /// # Safety
    ///
    /// Callers must guarantee that no other CPU or interrupt handler can access
    /// this global concurrently.
    pub unsafe fn replace_unlocked(&self, value: T) {
        unsafe {
            *self.value.get() = value;
        }
        self.locked.store(false, Ordering::Release);
    }
}

pub static UART: Global<Option<EarlyUart>> = Global::new(None);
pub static KERNEL_SERVICES: Global<ControlPlane> = Global::new(ControlPlane::empty());
