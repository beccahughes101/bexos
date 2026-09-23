//! Architecture-neutral live Trusty transaction. All events originate in the
//! resident execution owner after actual backend work; this module performs
//! neither candidate execution nor storage copying and advertises no capability.
use crate::writer::{self, Outcome, Ticket, Writer};
use crate::{CommitOutcome, OrchestratorError, SlotId, TeeCommit, TeeSlotState, TeeUpdatePhase};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Lifecycle(OrchestratorError),
    Storage(writer::Error),
    Invalid,
    NotReady,
    RecoveryRequired,
}

/// The resident persistent authority must bind all fields in one authenticated
/// transaction. Image selection alone cannot authorize a different writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Commit {
    pub image: TeeCommit,
    pub source_owner: u64,
    pub candidate_owner: u64,
    pub storage_epoch: u64,
    pub storage_watermark: u64,
    pub transport_epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CandidateReport {
    pub owner: u64,
    pub generation: u64,
    pub migration_abi: u64,
    pub digest: [u8; 32],
}
impl From<OrchestratorError> for Error {
    fn from(value: OrchestratorError) -> Self {
        Self::Lifecycle(value)
    }
}
impl From<writer::Error> for Error {
    fn from(value: writer::Error) -> Self {
        Self::Storage(value)
    }
}

pub struct Live {
    slots: TeeSlotState,
    writer: Writer,
    candidate: u64,
    required_services: u64,
    ready_services: u64,
    copied_through: Option<u64>,
    transport_epoch: u64,
    migration_abi: u64,
    recovery_required: bool,
}
impl Live {
    /// Required services come from the authenticated product policy, including
    /// its platform providers. Neither Trusty instance can reduce this mask.
    pub fn new(
        source: u64,
        generation: u64,
        active_slot: SlotId,
        image_hash: [u8; 32],
        storage_watermark: u64,
        transport_epoch: u64,
        required_services: u64,
    ) -> Result<Self, Error> {
        if generation == 0
            || image_hash == [0; 32]
            || required_services == 0
            || transport_epoch == 0
        {
            return Err(Error::Invalid);
        }
        let (a, b) = match active_slot {
            SlotId::A => (image_hash, [0; 32]),
            SlotId::B => ([0; 32], image_hash),
        };
        Ok(Self {
            slots: TeeSlotState::new(active_slot, generation, a, b),
            writer: Writer::new(source, transport_epoch, storage_watermark)?,
            candidate: 0,
            required_services,
            ready_services: 0,
            copied_through: None,
            transport_epoch,
            migration_abi: 1,
            recovery_required: false,
        })
    }
    pub fn phase(&self) -> TeeUpdatePhase {
        self.slots.phase
    }
    pub fn generation(&self) -> u64 {
        self.slots.generation
    }
    pub fn transport_epoch(&self) -> u64 {
        self.transport_epoch
    }
    pub fn storage_epoch(&self) -> u64 {
        self.writer.epoch()
    }
    pub fn storage_watermark(&self) -> u64 {
        self.writer.watermark()
    }
    /// Recover the unresolved write's identity from resident protected state;
    /// never replay the corresponding mutation to discover its outcome.
    pub fn pending_write_ticket(&self) -> Option<Ticket> {
        self.writer.pending_ticket()
    }
    pub fn writer_owner(&self) -> u64 {
        self.writer.owner()
    }
    pub fn recovery_required(&self) -> bool {
        self.recovery_required
    }

    pub fn begin_reported(&mut self, report: CandidateReport, now: u64) -> Result<(), Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        if self.writer.frozen() {
            return Err(Error::NotReady);
        }
        if report.owner == 0
            || report.owner == self.writer.owner()
            || report.digest == [0; 32]
            || report.migration_abi != self.migration_abi
        {
            return Err(Error::Invalid);
        }
        // Reserve both cutover and rollback incarnation numbers before starting.
        self.transport_epoch.checked_add(2).ok_or(Error::Invalid)?;
        self.writer.epoch().checked_add(1).ok_or(Error::Invalid)?;
        // Keep the complete committed record when a stale request arrives.
        // Reboot callers may instead report rejected staging as a rollback.
        if report.generation <= self.slots.generation {
            return Err(OrchestratorError::RollbackGeneration.into());
        }
        self.slots.stage(report.generation, report.digest)?;
        self.slots.prepare(report.generation, now)?;
        self.candidate = report.owner;
        self.ready_services = 0;
        self.copied_through = None;
        Ok(())
    }
    pub fn begin(
        &mut self,
        candidate: u64,
        generation: u64,
        digest: [u8; 32],
        now: u64,
    ) -> Result<(), Error> {
        self.begin_reported(
            CandidateReport {
                owner: candidate,
                generation,
                migration_abi: self.migration_abi,
                digest,
            },
            now,
        )
    }
    pub fn begin_write(&mut self, owner: u64, epoch: u64) -> Result<Ticket, Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        Ok(self.writer.begin(owner, epoch)?)
    }
    pub fn finish_write(&mut self, ticket: &Ticket, outcome: Outcome) -> Result<(), Error> {
        Ok(self.writer.finish(ticket, outcome)?)
    }
    pub fn resolve_write(&mut self, ticket: &Ticket, outcome: Outcome) -> Result<(), Error> {
        self.writer.resolve(ticket, outcome)?;
        if self.recovery_required && self.slots.phase == TeeUpdatePhase::RolledBack {
            // No image commitment was attempted. The source's outstanding
            // write was the only obstacle to completing this rollback.
            return self.resume_source();
        }
        if !self.recovery_required
            && !matches!(
                self.slots.phase,
                TeeUpdatePhase::LiveSwitch
                    | TeeUpdatePhase::HealthWindow
                    | TeeUpdatePhase::CommitPending
                    | TeeUpdatePhase::CommitUncertain
            )
        {
            self.writer.resume()?;
        }
        Ok(())
    }
    /// The importer calls this after validating a complete consistent snapshot
    /// and every delta through this source watermark. Readiness for an older
    /// snapshot is invalidated whenever a newer checkpoint is imported.
    pub fn imported(&mut self, candidate: u64, watermark: u64) -> Result<(), Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        if candidate != self.candidate
            || !matches!(
                self.slots.phase,
                TeeUpdatePhase::Verifying
                    | TeeUpdatePhase::LiveSwitch
                    | TeeUpdatePhase::HealthWindow
            )
            || watermark > self.writer.watermark()
            || self.copied_through.is_some_and(|old| watermark < old)
        {
            return Err(Error::Invalid);
        }
        if self.copied_through != Some(watermark) {
            self.ready_services = 0;
        }
        self.copied_through = Some(watermark);
        Ok(())
    }
    /// Owner-observed service probes are bound to the exact imported snapshot.
    pub fn service_ready(
        &mut self,
        candidate: u64,
        service: u64,
        watermark: u64,
    ) -> Result<(), Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        if candidate != self.candidate
            || !service.is_power_of_two()
            || service & self.required_services == 0
            || self.copied_through != Some(watermark)
            || !matches!(
                self.slots.phase,
                TeeUpdatePhase::Verifying
                    | TeeUpdatePhase::LiveSwitch
                    | TeeUpdatePhase::HealthWindow
            )
        {
            return Err(Error::Invalid);
        }
        self.ready_services |= service;
        Ok(())
    }
    fn caught_up(&self) -> bool {
        self.copied_through == Some(self.writer.watermark())
            && self.ready_services == self.required_services
            && self.writer.drained()
    }
    pub fn prepared(&mut self, now: u64) -> Result<(), Error> {
        if !self.caught_up() {
            return Err(Error::NotReady);
        }
        let result = self.slots.candidate_ready(now);
        self.lifecycle(result)
    }
    pub fn quiesce(&mut self, now: u64) -> Result<(), Error> {
        let result = self.slots.quiesce(now);
        self.lifecycle(result)?;
        self.writer.freeze();
        self.ready_services = 0;
        Ok(())
    }
    pub fn switch(&mut self, now: u64) -> Result<(), Error> {
        if !self.writer.frozen() || !self.caught_up() {
            return Err(Error::NotReady);
        }
        let generation = self.slots.pending_commit()?.generation;
        let result = self.slots.live_switch(generation, now);
        self.lifecycle(result)?;
        self.transport_epoch += 1;
        self.ready_services = 0;
        Ok(())
    }
    pub fn healthy(&mut self, now: u64) -> Result<(), Error> {
        if !self.caught_up() {
            return Err(Error::NotReady);
        }
        let result = self.slots.confirm_health(now);
        self.lifecycle(result)
    }
    /// Persistent service writes remain fenced throughout journal publication.
    /// Unknown is resolved from authenticated storage before either owner resumes.
    pub fn pending_commit(&self) -> Result<Commit, Error> {
        if !matches!(
            self.slots.phase,
            TeeUpdatePhase::CommitPending | TeeUpdatePhase::CommitUncertain
        ) {
            return Err(OrchestratorError::InvalidTransition.into());
        }
        Ok(Commit {
            image: self.slots.pending_commit()?,
            source_owner: self.writer.owner(),
            candidate_owner: self.candidate,
            storage_epoch: self.writer.epoch().checked_add(1).ok_or(Error::Invalid)?,
            storage_watermark: self.writer.watermark(),
            transport_epoch: self.transport_epoch,
        })
    }
    pub fn commit(&mut self, persist: impl FnOnce(Commit) -> CommitOutcome) -> Result<(), Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        let record = self.pending_commit()?;
        let result = self.slots.commit_durably(|_| persist(record));
        self.completion(result)
    }
    pub fn resolve_commit(&mut self, outcome: CommitOutcome) -> Result<(), Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        let result = self.slots.resolve_commit(outcome);
        self.completion(result)
    }
    fn completion(&mut self, result: Result<(), OrchestratorError>) -> Result<(), Error> {
        match (result, self.slots.phase) {
            (Ok(()), TeeUpdatePhase::Completed) => {
                if self.writer.transfer(self.candidate).is_err() {
                    self.recovery_required = true;
                    return Err(Error::RecoveryRequired);
                }
            }
            (Err(OrchestratorError::CommitFailed), TeeUpdatePhase::RolledBack) => {
                self.resume_source()?
            }
            _ => (),
        }
        result.map_err(Into::into)
    }
    fn resume_source(&mut self) -> Result<(), Error> {
        // Even rollback fences replies from the abandoned trial incarnation.
        let next_epoch = self.transport_epoch.checked_add(1).ok_or(Error::Invalid)?;
        if self.writer.resume().is_err() {
            self.recovery_required = true;
            return Err(Error::RecoveryRequired);
        }
        self.transport_epoch = next_epoch;
        self.recovery_required = false;
        self.candidate = 0;
        self.copied_through = None;
        self.ready_services = 0;
        Ok(())
    }
    fn lifecycle(&mut self, result: Result<(), OrchestratorError>) -> Result<(), Error> {
        if result == Err(OrchestratorError::DeadlineExceeded) {
            self.resume_source()?;
        }
        result.map_err(Into::into)
    }
    pub fn poll(&mut self, now: u64) -> Result<(), Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        let result = self.slots.poll_deadline(now);
        self.lifecycle(result)
    }
    pub fn abort(&mut self) -> Result<(), Error> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        self.slots.rollback()?;
        self.resume_source()
    }
}

#[path = "live_wire.rs"]
mod wire;
pub use wire::STATE_BYTES;
