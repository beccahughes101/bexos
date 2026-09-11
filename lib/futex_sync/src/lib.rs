//! Shared atomic algorithms for C platform adapters. Zero is an unlocked mutex
//! or unstarted once. Addresses must remain stable until all waiters finish.
#![no_std]
pub mod condition;
use core::sync::atomic::{AtomicU32, Ordering};
pub const INVALID: i32 = 22;
pub const BUSY: i32 = 16;
pub const DESTROYED: u32 = u32::MAX;
pub trait Wait {
    /// Atomically check expected and sleep. Value changes and interruptions are
    /// successful retries; permanent failures must be returned to the caller.
    fn wait(&self, word: &AtomicU32, expected: u32) -> Result<(), i32>;
    fn wake(&self, word: &AtomicU32, count: u32) -> Result<(), i32>;
}
pub fn try_lock(word: &AtomicU32) -> Result<(), i32> {
    match word.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed) {
        Ok(_) => Ok(()),
        Err(1 | 2) => Err(BUSY),
        Err(_) => Err(INVALID),
    }
}
pub fn lock(word: &AtomicU32, wait: &impl Wait) -> Result<(), i32> {
    match try_lock(word) {
        Ok(()) => return Ok(()),
        Err(BUSY) => {}
        Err(e) => return Err(e),
    }
    loop {
        let value = word.load(Ordering::Relaxed);
        if value > 2 {
            return Err(INVALID);
        }
        match word.compare_exchange(value, 2, Ordering::Acquire, Ordering::Relaxed) {
            Ok(0) => return Ok(()),
            Ok(_) => wait.wait(word, 2)?,
            Err(_) => {}
        }
    }
}
/// The caller must own the mutex. Successful release publishes all preceding
/// writes. An already unlocked/destroyed mutex is rejected without mutation.
pub fn unlock(word: &AtomicU32, wait: &impl Wait) -> Result<(), i32> {
    loop {
        let value = word.load(Ordering::Relaxed);
        if !matches!(value, 1 | 2) {
            return Err(INVALID);
        }
        if word
            .compare_exchange(value, 0, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            if value == 2 {
                wait.wake(word, 1)?;
            }
            return Ok(());
        }
    }
}
pub fn destroy(word: &AtomicU32) -> Result<(), i32> {
    match word.compare_exchange(0, DESTROYED, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => Ok(()),
        Err(1 | 2) => Err(BUSY),
        Err(_) => Err(INVALID),
    }
}
pub fn once(word: &AtomicU32, wait: &impl Wait, initialize: impl FnOnce()) -> Result<(), i32> {
    match word.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire) {
        Ok(_) => {
            initialize();
            word.store(2, Ordering::Release);
            wait.wake(word, u32::MAX)
        }
        Err(2) => Ok(()),
        Err(1) => loop {
            wait.wait(word, 1)?;
            match word.load(Ordering::Acquire) {
                2 => return Ok(()),
                1 => {}
                _ => return Err(INVALID),
            }
        },
        Err(_) => Err(INVALID),
    }
}
