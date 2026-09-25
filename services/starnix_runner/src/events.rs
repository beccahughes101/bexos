use bexos_zircon::{Clock, ClockId};
use starnix_kernel::{EAGAIN, EINVAL, EIO};
use std::collections::BTreeMap;

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET: u32 = 1 << 31;
pub const EPOLL_ALLOWED: u32 =
    EPOLLIN | 0x002 | EPOLLOUT | EPOLLERR | EPOLLHUP | 0x2000 | EPOLLONESHOT | EPOLLET;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpollInterest {
    pub events: u32,
    pub data: u64,
    pub enabled: bool,
}

#[derive(Default)]
pub struct EpollState {
    pub interests: BTreeMap<i32, EpollInterest>,
}

pub struct EventFdState {
    pub counter: u64,
    pub semaphore: bool,
}

impl EventFdState {
    pub fn read(&mut self) -> Result<u64, i64> {
        if self.counter == 0 {
            return Err(EAGAIN);
        }
        let value = if self.semaphore { 1 } else { self.counter };
        self.counter -= value;
        Ok(value)
    }

    pub fn try_write(&mut self, value: u64) -> Result<bool, i64> {
        if value == u64::MAX {
            return Err(EINVAL);
        }
        if self.counter <= (u64::MAX - 1).saturating_sub(value) {
            self.counter += value;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

pub struct TimerFdState {
    pub clock_id: i32,
    pub deadline: u64,
    pub interval: u64,
    pub armed: bool,
}

impl TimerFdState {
    pub fn new(clock_id: i32) -> Self {
        Self {
            clock_id,
            deadline: 0,
            interval: 0,
            armed: false,
        }
    }

    pub fn now(&self) -> Result<u64, i64> {
        Clock::get(match self.clock_id {
            0 => ClockId::Realtime,
            1 => ClockId::Monotonic,
            7 => ClockId::Boot,
            _ => return Err(EINVAL),
        })
        .map_err(|_| EIO)
    }

    pub fn remaining(&self) -> Result<u64, i64> {
        Ok(if self.armed {
            self.deadline.saturating_sub(self.now()?)
        } else {
            0
        })
    }

    pub fn set(&mut self, value: u64, interval: u64, absolute: bool) -> Result<(), i64> {
        self.interval = interval;
        self.armed = value != 0;
        self.deadline = if !self.armed {
            0
        } else if absolute {
            value
        } else {
            self.now()?.checked_add(value).ok_or(EINVAL)?
        };
        Ok(())
    }

    pub fn expirations(&mut self) -> Result<u64, i64> {
        if !self.armed {
            return Ok(0);
        }
        let now = self.now()?;
        if now < self.deadline {
            return Ok(0);
        }
        if self.interval == 0 {
            self.armed = false;
            return Ok(1);
        }
        let count = 1 + (now - self.deadline) / self.interval;
        self.deadline = self
            .deadline
            .saturating_add(count.saturating_mul(self.interval));
        Ok(count)
    }

    pub fn ready(&self) -> Result<bool, i64> {
        Ok(self.armed && self.now()? >= self.deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eventfd_counter_and_semaphore_modes_match_linux_reads() {
        let mut counter = EventFdState {
            counter: 7,
            semaphore: false,
        };
        assert_eq!(counter.read(), Ok(7));
        assert_eq!(counter.read(), Err(EAGAIN));
        assert_eq!(counter.try_write(11), Ok(true));
        assert_eq!(counter.read(), Ok(11));

        let mut semaphore = EventFdState {
            counter: 2,
            semaphore: true,
        };
        assert_eq!(semaphore.read(), Ok(1));
        assert_eq!(semaphore.read(), Ok(1));
        assert_eq!(semaphore.read(), Err(EAGAIN));
    }

    #[test]
    fn eventfd_rejects_reserved_value_and_reports_overflow() {
        let mut state = EventFdState {
            counter: u64::MAX - 2,
            semaphore: false,
        };
        assert_eq!(state.try_write(u64::MAX), Err(EINVAL));
        assert_eq!(state.try_write(2), Ok(false));
        assert_eq!(state.try_write(1), Ok(true));
    }
}
