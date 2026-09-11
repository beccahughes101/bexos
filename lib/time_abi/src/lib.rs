#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};

pub const TIME_PAGE_MAGIC: u64 = u64::from_le_bytes(*b"BEXTIME1");
pub const TIME_PAGE_VERSION: u32 = 1;
pub const TIME_PAGE_SIZE: usize = core::mem::size_of::<TimePageV1>();
pub const MAX_SLEW_RATE_PPM: i32 = 500;

/// Convert an architectural counter value to nanoseconds without overflowing
/// at the few-minute boundary reached by high-frequency counters.
pub fn ticks_to_nanos(ticks: u64, frequency_hz: u64) -> u64 {
    if frequency_hz == 0 {
        return 0;
    }
    saturating_scale(ticks, 1_000_000_000, frequency_hz)
}

/// Convert an absolute nanosecond deadline to architectural counter ticks
/// without overflowing before the deadline is reached.
pub fn nanos_to_ticks(nanos: u64, frequency_hz: u64) -> u64 {
    saturating_scale(nanos, frequency_hz, 1_000_000_000)
}

fn saturating_scale(value: u64, numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    let scaled = u128::from(value) * u128::from(numerator) / u128::from(denominator);
    scaled.min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlewError {
    InvalidRate,
    DirectionMismatch,
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlewState {
    pub realtime_offset_ns: i64,
    pub slew_start_monotonic_ns: u64,
    pub slew_remaining_ns: i64,
    pub slew_rate_ppm: i32,
}

impl SlewState {
    pub const fn new() -> Self {
        Self {
            realtime_offset_ns: 0,
            slew_start_monotonic_ns: 0,
            slew_remaining_ns: 0,
            slew_rate_ppm: 0,
        }
    }

    pub fn materialized_at(mut self, monotonic_ns: u64) -> Result<Self, SlewError> {
        let applied = accrued_slew(
            self.slew_start_monotonic_ns,
            monotonic_ns,
            self.slew_remaining_ns,
            self.slew_rate_ppm,
        )?;
        self.realtime_offset_ns = self
            .realtime_offset_ns
            .checked_add(applied)
            .ok_or(SlewError::Overflow)?;
        self.slew_remaining_ns = self
            .slew_remaining_ns
            .checked_sub(applied)
            .ok_or(SlewError::Overflow)?;
        self.slew_start_monotonic_ns = monotonic_ns;
        if self.slew_remaining_ns == 0 {
            self.slew_rate_ppm = 0;
        }
        Ok(self)
    }

    pub fn adjust(
        self,
        monotonic_ns: u64,
        offset_delta_ns: i64,
        slew_rate_ppm: i32,
    ) -> Result<Self, SlewError> {
        if !(-MAX_SLEW_RATE_PPM..=MAX_SLEW_RATE_PPM).contains(&slew_rate_ppm) {
            return Err(SlewError::InvalidRate);
        }
        let mut state = self.materialized_at(monotonic_ns)?;
        if slew_rate_ppm == 0 {
            state.realtime_offset_ns = state
                .realtime_offset_ns
                .checked_add(offset_delta_ns)
                .ok_or(SlewError::Overflow)?;
            state.slew_remaining_ns = 0;
            state.slew_rate_ppm = 0;
            return Ok(state);
        }
        let pending = state
            .slew_remaining_ns
            .checked_add(offset_delta_ns)
            .ok_or(SlewError::Overflow)?;
        if pending == 0 {
            state.slew_remaining_ns = 0;
            state.slew_rate_ppm = 0;
            return Ok(state);
        }
        if pending.signum() != i64::from(slew_rate_ppm).signum() {
            return Err(SlewError::DirectionMismatch);
        }
        state.slew_remaining_ns = pending;
        state.slew_rate_ppm = slew_rate_ppm;
        state.slew_start_monotonic_ns = monotonic_ns;
        Ok(state)
    }

    pub fn realtime_at(&self, monotonic_ns: u64) -> Option<u64> {
        let applied = accrued_slew(
            self.slew_start_monotonic_ns,
            monotonic_ns,
            self.slew_remaining_ns,
            self.slew_rate_ppm,
        )
        .ok()?;
        let realtime =
            i128::from(monotonic_ns) + i128::from(self.realtime_offset_ns) + i128::from(applied);
        u64::try_from(realtime).ok()
    }
}

impl Default for SlewState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimePageSnapshot {
    pub base_ticks: u64,
    pub tick_frequency_hz: u64,
    pub base_monotonic_ns: u64,
    pub realtime_offset_ns: i64,
    pub slew_start_monotonic_ns: u64,
    pub slew_remaining_ns: i64,
    pub slew_rate_ppm: i32,
}

impl TimePageSnapshot {
    pub fn monotonic_from_ticks(&self, ticks: u64) -> u64 {
        let elapsed_ticks = ticks.wrapping_sub(self.base_ticks);
        self.base_monotonic_ns
            .saturating_add(ticks_to_nanos(elapsed_ticks, self.tick_frequency_hz))
    }

    pub fn realtime_from_ticks(&self, ticks: u64) -> Option<u64> {
        let monotonic = self.monotonic_from_ticks(ticks);
        SlewState {
            realtime_offset_ns: self.realtime_offset_ns,
            slew_start_monotonic_ns: self.slew_start_monotonic_ns,
            slew_remaining_ns: self.slew_remaining_ns,
            slew_rate_ppm: self.slew_rate_ppm,
        }
        .realtime_at(monotonic)
    }
}

#[repr(C, align(64))]
pub struct TimePageV1 {
    pub magic: u64,
    pub abi_version: u32,
    pub size: u32,
    pub sequence: AtomicU32,
    pub reserved0: u32,
    pub base_ticks: u64,
    pub tick_frequency_hz: u64,
    pub base_monotonic_ns: u64,
    pub realtime_offset_ns: i64,
    pub slew_start_monotonic_ns: u64,
    pub slew_remaining_ns: i64,
    pub slew_rate_ppm: i32,
    pub reserved1: u32,
}

impl TimePageV1 {
    pub const fn zeroed() -> Self {
        Self {
            magic: TIME_PAGE_MAGIC,
            abi_version: TIME_PAGE_VERSION,
            size: TIME_PAGE_SIZE as u32,
            sequence: AtomicU32::new(0),
            reserved0: 0,
            base_ticks: 0,
            tick_frequency_hz: 1,
            base_monotonic_ns: 0,
            realtime_offset_ns: 0,
            slew_start_monotonic_ns: 0,
            slew_remaining_ns: 0,
            slew_rate_ppm: 0,
            reserved1: 0,
        }
    }

    pub fn write_snapshot(&mut self, snapshot: TimePageSnapshot) {
        let start = self.sequence.load(Ordering::Relaxed);
        self.sequence.store(start | 1, Ordering::Release);
        self.base_ticks = snapshot.base_ticks;
        self.tick_frequency_hz = snapshot.tick_frequency_hz.max(1);
        self.base_monotonic_ns = snapshot.base_monotonic_ns;
        self.realtime_offset_ns = snapshot.realtime_offset_ns;
        self.slew_start_monotonic_ns = snapshot.slew_start_monotonic_ns;
        self.slew_remaining_ns = snapshot.slew_remaining_ns;
        self.slew_rate_ppm = snapshot.slew_rate_ppm;
        self.sequence
            .store((start.wrapping_add(2)) & !1, Ordering::Release);
    }

    pub fn read_snapshot(&self) -> Option<TimePageSnapshot> {
        let magic = unsafe { core::ptr::read_volatile(&self.magic) };
        let abi_version = unsafe { core::ptr::read_volatile(&self.abi_version) };
        if magic != TIME_PAGE_MAGIC || abi_version != TIME_PAGE_VERSION {
            return None;
        }
        for _ in 0..8 {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            let read_u64 = |value: &u64| unsafe { core::ptr::read_volatile(value) };
            let read_i64 = |value: &i64| unsafe { core::ptr::read_volatile(value) };
            let read_i32 = |value: &i32| unsafe { core::ptr::read_volatile(value) };
            let snapshot = TimePageSnapshot {
                base_ticks: read_u64(&self.base_ticks),
                tick_frequency_hz: read_u64(&self.tick_frequency_hz).max(1),
                base_monotonic_ns: read_u64(&self.base_monotonic_ns),
                realtime_offset_ns: read_i64(&self.realtime_offset_ns),
                slew_start_monotonic_ns: read_u64(&self.slew_start_monotonic_ns),
                slew_remaining_ns: read_i64(&self.slew_remaining_ns),
                slew_rate_ppm: read_i32(&self.slew_rate_ppm),
            };
            let after = self.sequence.load(Ordering::Acquire);
            if before == after && after & 1 == 0 {
                return Some(snapshot);
            }
            core::hint::spin_loop();
        }
        None
    }
}

const fn abs_i64(value: i64) -> u64 {
    if value < 0 {
        value.wrapping_neg() as u64
    } else {
        value as u64
    }
}

fn accrued_slew(
    start_monotonic_ns: u64,
    now_monotonic_ns: u64,
    remaining_ns: i64,
    rate_ppm: i32,
) -> Result<i64, SlewError> {
    if remaining_ns == 0 || rate_ppm == 0 {
        return Ok(0);
    }
    if remaining_ns.signum() != i64::from(rate_ppm).signum() {
        return Err(SlewError::DirectionMismatch);
    }
    let elapsed = now_monotonic_ns.saturating_sub(start_monotonic_ns);
    let magnitude = (u128::from(elapsed) * u128::from(rate_ppm.unsigned_abs()) / 1_000_000u128)
        .min(u128::from(abs_i64(remaining_ns))) as u64;
    if remaining_ns < 0 {
        i64::try_from(magnitude)
            .map(|value| -value)
            .map_err(|_| SlewError::Overflow)
    } else {
        i64::try_from(magnitude).map_err(|_| SlewError::Overflow)
    }
}
