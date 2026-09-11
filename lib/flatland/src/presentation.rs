//! Bounded frame-boundary transaction scheduling. Time uses monotonic ticks;
//! the caller supplies the same clock domain as the presentation protocol.
use crate::{Error, scene::Graph};
use alloc::collections::VecDeque;

pub const MAX_QUEUED_PRESENTS: usize = 3;
#[derive(Clone, Debug)]
pub struct Pending {
    pub sequence: u64,
    pub time: u64,
    pub graph: Graph,
}
#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub frames: VecDeque<Pending>,
    pub sequence: u64,
    pub last_time: u64,
    pub latched_sequence: u64,
    pub presented_sequence: u64,
    pub rejected_sequence: u64,
    pub presented_at: u64,
}
impl Queue {
    pub fn submit(&mut self, graph: &Graph, time: u64) -> Result<u64, Error> {
        if time < self.last_time {
            return Err(Error::Stale);
        }
        if self.frames.len() == MAX_QUEUED_PRESENTS {
            return Err(Error::Busy);
        }
        graph.validate()?;
        let sequence = self.sequence.checked_add(1).ok_or(Error::Bounds)?;
        self.frames.push_back(Pending {
            sequence,
            time,
            graph: graph.clone(),
        });
        self.sequence = sequence;
        self.last_time = time;
        Ok(sequence)
    }
    /// Latch at most one transaction per session per frame; acquire readiness
    /// must be established by the owner before calling this method.
    pub fn latch(&mut self, now: u64) -> Option<Pending> {
        if self.frames.front().is_some_and(|p| p.time <= now) {
            let frame = self.frames.pop_front()?;
            self.latched_sequence = frame.sequence;
            Some(frame)
        } else {
            None
        }
    }
    pub fn reject(&mut self) -> Option<Pending> {
        let frame = self.frames.pop_front()?;
        self.rejected_sequence = frame.sequence;
        Some(frame)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.frames.len() > MAX_QUEUED_PRESENTS {
            return Err(Error::Bounds);
        }
        if self.latched_sequence > self.sequence
            || self.presented_sequence > self.latched_sequence
            || self.rejected_sequence > self.sequence
        {
            return Err(Error::Invalid);
        }
        let mut last = None;
        for p in &self.frames {
            if p.sequence == 0
                || p.sequence <= self.latched_sequence
                || p.sequence <= self.rejected_sequence
                || p.sequence > self.sequence
                || p.time > self.last_time
                || last.is_some_and(|(seq, time)| p.sequence <= seq || p.time < time)
            {
                return Err(Error::Invalid);
            }
            p.graph.validate()?;
            last = Some((p.sequence, p.time));
        }
        Ok(())
    }
}
