use crate::{DEFAULT_CUTOVER_MS, DEFAULT_PREPARATION_MS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    BadState,
    InvalidData,
    UnsupportedVersion,
    Checksum,
    Sequence,
    Capacity,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Phase {
    Staged = 1,
    Bulk = 2,
    CatchUp = 3,
    Quiesced = 4,
    Ready = 5,
    Committed = 6,
    Reclaimed = 7,
    Aborted = 8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeouts {
    pub preparation_ms: u64,
    pub cutover_ms: u64,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            preparation_ms: DEFAULT_PREPARATION_MS,
            cutover_ms: DEFAULT_CUTOVER_MS,
        }
    }
}

/// Times are monotonic milliseconds supplied by the adapter, never wall time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    generation: u64,
    phase: Phase,
    preparation_deadline: u64,
    cutover_deadline: Option<u64>,
    timeouts: Timeouts,
    last_time: u64,
    final_sequence: Option<u64>,
}
impl Session {
    pub fn new(generation: u64, now: u64, timeouts: Timeouts) -> Result<Self, Error> {
        if generation == 0 || timeouts.preparation_ms == 0 || timeouts.cutover_ms == 0 {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            generation,
            phase: Phase::Staged,
            preparation_deadline: now
                .checked_add(timeouts.preparation_ms)
                .ok_or(Error::InvalidData)?,
            cutover_deadline: None,
            timeouts,
            last_time: now,
            final_sequence: None,
        })
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cutover_started_ms(&self) -> Option<u64> {
        self.cutover_deadline.map(|d| d - self.timeouts.cutover_ms)
    }
    pub fn final_sequence(&self) -> Option<u64> {
        self.final_sequence
    }
    pub fn poll(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_time {
            return Err(Error::InvalidData);
        }
        self.last_time = now;
        if matches!(self.phase, Phase::Committed | Phase::Reclaimed) {
            return Ok(());
        }
        if self.phase == Phase::Aborted {
            return Err(Error::BadState);
        }
        if now >= self.preparation_deadline
            || self
                .cutover_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            self.phase = Phase::Aborted;
            return Err(Error::TimedOut);
        }
        Ok(())
    }
    fn advance(&mut self, from: Phase, to: Phase, now: u64) -> Result<(), Error> {
        self.poll(now)?;
        if self.phase != from {
            return Err(Error::BadState);
        }
        self.phase = to;
        Ok(())
    }
    pub fn begin_bulk(&mut self, now: u64) -> Result<(), Error> {
        self.advance(Phase::Staged, Phase::Bulk, now)
    }
    pub fn bulk_complete(&mut self, now: u64) -> Result<(), Error> {
        self.advance(Phase::Bulk, Phase::CatchUp, now)
    }
    pub fn quiesce(&mut self, sequence: u64, now: u64) -> Result<(), Error> {
        let deadline = now
            .checked_add(self.timeouts.cutover_ms)
            .ok_or(Error::InvalidData)?;
        self.advance(Phase::CatchUp, Phase::Quiesced, now)?;
        self.cutover_deadline = Some(deadline);
        self.final_sequence = Some(sequence);
        Ok(())
    }
    pub fn ready(&mut self, applied_sequence: u64, now: u64) -> Result<(), Error> {
        self.poll(now)?;
        if self.final_sequence != Some(applied_sequence) {
            return Err(Error::Sequence);
        }
        self.advance(Phase::Quiesced, Phase::Ready, now)
    }
    pub fn commit(&mut self, now: u64) -> Result<(), Error> {
        self.advance(Phase::Ready, Phase::Committed, now)
    }
    pub fn reclaimed(&mut self, now: u64) -> Result<(), Error> {
        self.advance(Phase::Committed, Phase::Reclaimed, now)
    }
    pub fn abort(&mut self) -> Result<(), Error> {
        if matches!(self.phase, Phase::Committed | Phase::Reclaimed) {
            return Err(Error::BadState);
        }
        self.phase = Phase::Aborted;
        Ok(())
    }
}
