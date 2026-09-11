use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub struct SpinLock<T> {
    held: AtomicBool,
    inner: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
pub struct SpinLockGuard<'a, T>(&'a SpinLock<T>);
impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.0.inner.get() }
    }
}
impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.0.inner.get() }
    }
}
impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.0.held.store(false, Ordering::Release);
    }
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            held: AtomicBool::new(false),
            inner: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .held
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinLockGuard(self)
    }
}
