//! Allocation-free condition waits. Register while the associated mutex is
//! held, release it before sleeping, and always reacquire it before returning.
use crate::{BUSY, INVALID, Wait, lock, unlock};
use core::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
#[derive(Default)]
pub struct Condition {
    sequence: AtomicU32,
    waiters: AtomicU32,
    closed: AtomicU32,
}
impl Condition {
    pub fn wait(
        &self,
        mutex: &AtomicU32,
        sleep: &impl Wait,
        reacquire: &impl Wait,
    ) -> Result<(), i32> {
        if self.closed.load(Ordering::Acquire) != 0 {
            return Err(INVALID);
        }
        let sequence = self.sequence.load(Ordering::Acquire);
        self.waiters
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| BUSY)?;
        if self.closed.load(Ordering::Acquire) != 0 {
            self.waiters.fetch_sub(1, Ordering::Release);
            return Err(INVALID);
        }
        if let Err(error) = unlock(mutex, reacquire) {
            self.waiters.fetch_sub(1, Ordering::Release);
            return Err(error);
        }
        let result = if self.sequence.load(Ordering::Acquire) == sequence {
            sleep.wait(&self.sequence, sequence)
        } else {
            Ok(())
        };
        let locked = lock(mutex, reacquire);
        self.waiters.fetch_sub(1, Ordering::Release);
        locked.and(result)
    }
    pub fn signal(&self, all: bool, wait: &impl Wait) -> Result<(), i32> {
        if self.closed.load(Ordering::Acquire) != 0 {
            return Err(INVALID);
        }
        self.sequence.fetch_add(1, Ordering::Release);
        if self.waiters.load(Ordering::Acquire) != 0 {
            wait.wake(&self.sequence, if all { u32::MAX } else { 1 })?;
        }
        Ok(())
    }
    pub fn destroy(&self) -> Result<(), i32> {
        if self.waiters.load(Ordering::Acquire) != 0 {
            return Err(BUSY);
        }
        self.closed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| INVALID)?;
        if self.waiters.load(Ordering::Acquire) != 0 {
            self.closed.store(0, Ordering::Release);
            return Err(BUSY);
        }
        Ok(())
    }
}
