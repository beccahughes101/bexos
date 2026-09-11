//! Receiver-side and coordinator state machines, independent of transport.
use crate::schema::Error;
use alloc::{collections::BTreeSet, sync::Arc, vec::Vec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Phase {
    Preparing,
    Prepared,
    Resolving,
    Committed,
    Complete,
    Aborted,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transaction {
    pub generation: u64,
    pub deadline_ms: u64,
    pub phase: Phase,
    pub participants: BTreeSet<u64>,
    pub awaiting: BTreeSet<u64>,
}
impl Transaction {
    pub fn new(
        current: u64,
        expected: u64,
        now_ms: u64,
        timeout_ms: u64,
        participants: BTreeSet<u64>,
    ) -> Result<Self, Error> {
        if current != expected {
            return Err(Error::Conflict);
        }
        let generation = current.checked_add(1).ok_or(Error::Overflow)?;
        let deadline_ms = now_ms.checked_add(timeout_ms).ok_or(Error::Overflow)?;
        Ok(Self {
            generation,
            deadline_ms,
            phase: if participants.is_empty() {
                Phase::Prepared
            } else {
                Phase::Preparing
            },
            awaiting: participants.clone(),
            participants,
        })
    }
    pub fn prepare_reply(
        &mut self,
        receiver: u64,
        generation: u64,
        accepted: bool,
    ) -> Result<(), Error> {
        if generation != self.generation
            || self.phase != Phase::Preparing
            || !self.awaiting.remove(&receiver)
        {
            return Err(Error::Conflict);
        }
        if !accepted {
            self.phase = Phase::Aborted;
            return Err(Error::Rejected);
        }
        if self.awaiting.is_empty() {
            self.phase = Phase::Prepared;
        }
        Ok(())
    }
    pub fn expire(&mut self, now_ms: u64) -> Result<(), Error> {
        if self.phase == Phase::Preparing && now_ms >= self.deadline_ms {
            self.phase = Phase::Aborted;
            return Err(Error::Timeout);
        }
        Ok(())
    }
    /// Invoke only after the commit decision has been synchronized to storage.
    pub fn durable_commit(&mut self) -> Result<(), Error> {
        if !matches!(self.phase, Phase::Prepared | Phase::Resolving) {
            return Err(Error::Busy);
        }
        self.awaiting = self.participants.clone();
        self.phase = if self.awaiting.is_empty() {
            Phase::Complete
        } else {
            Phase::Committed
        };
        Ok(())
    }
    pub fn commit_reply(&mut self, receiver: u64, generation: u64) -> Result<(), Error> {
        if generation != self.generation
            || !matches!(self.phase, Phase::Committed | Phase::Complete)
        {
            return Err(Error::Conflict);
        }
        self.awaiting.remove(&receiver);
        if self.awaiting.is_empty() {
            self.phase = Phase::Complete;
        }
        Ok(())
    }
    pub fn disconnect(&mut self, receiver: u64) -> Result<(), Error> {
        if self.participants.contains(&receiver)
            && matches!(self.phase, Phase::Preparing | Phase::Prepared)
        {
            self.phase = Phase::Aborted;
            return Err(Error::Rejected);
        }
        if matches!(
            self.phase,
            Phase::Resolving | Phase::Committed | Phase::Complete
        ) {
            self.participants.remove(&receiver);
            self.awaiting.remove(&receiver);
            if self.phase == Phase::Committed && self.awaiting.is_empty() {
                self.phase = Phase::Complete;
            }
        }
        Ok(())
    }
    pub fn abort(&mut self) -> Result<(), Error> {
        if matches!(
            self.phase,
            Phase::Resolving | Phase::Committed | Phase::Complete
        ) {
            return Err(Error::Conflict);
        }
        self.phase = Phase::Aborted;
        Ok(())
    }
}
/// Snapshots are owned; readers retaining an Arc survive a committed swap.
#[derive(Clone, Debug)]
pub struct Receiver<T> {
    current: Arc<T>,
    generation: u64,
    pending: Option<(u64, Arc<T>)>,
}
impl<T> Receiver<T> {
    pub fn new(current: T, generation: u64) -> Self {
        Self {
            current: Arc::new(current),
            generation,
            pending: None,
        }
    }
    pub fn snapshot(&self) -> Arc<T> {
        self.current.clone()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn prepare(
        &mut self,
        generation: u64,
        value: T,
        validate: impl FnOnce(&T) -> bool,
    ) -> Result<(), Error> {
        if generation <= self.generation {
            return Err(Error::Conflict);
        }
        if self.pending.as_ref().is_some_and(|(g, _)| *g != generation) {
            return Err(Error::Busy);
        }
        if !validate(&value) {
            return Err(Error::Rejected);
        }
        self.pending = Some((generation, Arc::new(value)));
        Ok(())
    }
    pub fn commit(&mut self, generation: u64, changed: impl FnOnce(&T)) -> Result<(), Error> {
        if generation == self.generation {
            return Ok(());
        }
        if self.pending.as_ref().map(|(g, _)| *g) != Some(generation) {
            return Err(Error::Conflict);
        }
        let (_, value) = self.pending.take().unwrap();
        self.current = value;
        self.generation = generation;
        changed(&self.current);
        Ok(())
    }
    pub fn abort(&mut self, generation: u64) {
        if self.pending.as_ref().is_some_and(|(g, _)| *g == generation) {
            self.pending = None;
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSnapshot {
    pub receiver: u64,
    pub bytes: Vec<u8>,
}
