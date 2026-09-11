//! Secure-runtime slot coordination. Runtime owners supply trusted monotonic
//! timestamps and the result of an atomic persistent commit. This state machine
//! does not itself write firmware, start a candidate, or authenticate its image.
use crate::{OrchestratorError, SlotId};
#[path = "slot_wire.rs"]
mod wire;
pub use wire::{STATE_BYTES, StateIdentity};

pub const PREPARATION_DEADLINE_NS: u64 = 30_000_000_000;
pub const CUTOVER_DEADLINE_NS: u64 = 150_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeeUpdatePhase {
    Idle,
    Verifying,
    Staged,
    Prepared,
    LiveSwitch,
    RebootPending,
    HealthWindow,
    CommitPending,
    CommitUncertain,
    Completed,
    RolledBack,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TeeCommit {
    pub slot: SlotId,
    pub generation: u64,
    pub image_hash: [u8; 32],
}

/// A lost acknowledgement is not proof that a durable write failed. Owners
/// must resolve Unknown by reading authenticated persistent state before they
/// can retire either runtime or choose a rollback owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    Committed,
    NotCommitted,
    Unknown,
}

#[derive(Debug, Eq, PartialEq)]
pub struct TeeSlotState {
    pub active_slot: SlotId,
    pub pending_slot: Option<SlotId>,
    /// The durably committed generation, never the merely running candidate.
    pub generation: u64,
    pub rollback_slot: Option<SlotId>,
    pub reboot_required: bool,
    pub phase: TeeUpdatePhase,
    pub slot_hashes: [[u8; 32]; 2],
    pending_generation: Option<u64>,
    preparation_started: Option<u64>,
    cutover_started: Option<u64>,
    last_tick: u64,
}

impl TeeSlotState {
    pub const fn new(
        active_slot: SlotId,
        generation: u64,
        a_hash: [u8; 32],
        b_hash: [u8; 32],
    ) -> Self {
        Self {
            active_slot,
            pending_slot: None,
            generation,
            rollback_slot: None,
            reboot_required: false,
            phase: TeeUpdatePhase::Idle,
            slot_hashes: [a_hash, b_hash],
            pending_generation: None,
            preparation_started: None,
            cutover_started: None,
            last_tick: 0,
        }
    }

    pub fn stage(
        &mut self,
        generation: u64,
        image_hash: [u8; 32],
    ) -> Result<SlotId, OrchestratorError> {
        if self.pending_slot.is_some()
            || !matches!(
                self.phase,
                TeeUpdatePhase::Idle | TeeUpdatePhase::Completed | TeeUpdatePhase::RolledBack
            )
        {
            return Err(OrchestratorError::InvalidTransition);
        }
        if generation <= self.generation {
            self.phase = TeeUpdatePhase::RolledBack;
            return Err(OrchestratorError::RollbackGeneration);
        }
        let inactive = match self.active_slot {
            SlotId::A => SlotId::B,
            SlotId::B => SlotId::A,
        };
        self.slot_hashes[slot_index(inactive)] = image_hash;
        self.pending_slot = Some(inactive);
        self.pending_generation = Some(generation);
        self.rollback_slot = Some(self.active_slot);
        self.phase = TeeUpdatePhase::Staged;
        Ok(inactive)
    }

    pub fn prepare(&mut self, generation: u64, now_ns: u64) -> Result<(), OrchestratorError> {
        self.require_generation(generation)?;
        self.require_phase(TeeUpdatePhase::Staged)?;
        self.preparation_started = Some(now_ns);
        self.last_tick = now_ns;
        self.phase = TeeUpdatePhase::Verifying;
        Ok(())
    }

    /// Candidate readiness includes a consistent storage snapshot and caught-up
    /// durable changes; callers must establish these facts before this event.
    pub fn candidate_ready(&mut self, now_ns: u64) -> Result<(), OrchestratorError> {
        self.require_phase(TeeUpdatePhase::Verifying)?;
        self.check_time(now_ns, false)?;
        self.phase = TeeUpdatePhase::Prepared;
        Ok(())
    }

    /// Start the cutover clock before quiescing the old owner's clients.
    pub fn quiesce(&mut self, now_ns: u64) -> Result<(), OrchestratorError> {
        self.require_phase(TeeUpdatePhase::Prepared)?;
        self.check_time(now_ns, false)?;
        self.cutover_started = Some(now_ns);
        self.phase = TeeUpdatePhase::LiveSwitch;
        Ok(())
    }

    pub fn live_switch(&mut self, generation: u64, now_ns: u64) -> Result<(), OrchestratorError> {
        self.require_generation(generation)?;
        self.require_phase(TeeUpdatePhase::LiveSwitch)?;
        self.check_time(now_ns, true)?;
        self.active_slot = self
            .pending_slot
            .ok_or(OrchestratorError::InvalidTransition)?;
        self.reboot_required = false;
        self.phase = TeeUpdatePhase::HealthWindow;
        Ok(())
    }

    pub fn boot_activate(&mut self, generation: u64, now_ns: u64) -> Result<(), OrchestratorError> {
        self.require_generation(generation)?;
        self.require_phase(TeeUpdatePhase::Prepared)?;
        self.check_time(now_ns, false)?;
        self.reboot_required = true;
        self.phase = TeeUpdatePhase::RebootPending;
        Ok(())
    }

    pub fn boot_entered(&mut self, generation: u64, now_ns: u64) -> Result<(), OrchestratorError> {
        self.require_generation(generation)?;
        self.require_phase(TeeUpdatePhase::RebootPending)?;
        self.active_slot = self
            .pending_slot
            .ok_or(OrchestratorError::InvalidTransition)?;
        self.cutover_started = Some(now_ns);
        self.last_tick = now_ns;
        self.reboot_required = false;
        self.phase = TeeUpdatePhase::HealthWindow;
        Ok(())
    }

    /// Health means all required secure services have reconnected and become
    /// ready. Merely entering candidate code does not satisfy this transition.
    pub fn confirm_health(&mut self, now_ns: u64) -> Result<(), OrchestratorError> {
        self.require_phase(TeeUpdatePhase::HealthWindow)?;
        self.check_time(now_ns, true)?;
        self.phase = TeeUpdatePhase::CommitPending;
        Ok(())
    }

    /// Called by the owner's timer even when a candidate stops reporting
    /// progress. Deadline enforcement must not depend on another candidate call.
    pub fn poll_deadline(&mut self, now_ns: u64) -> Result<(), OrchestratorError> {
        match self.phase {
            TeeUpdatePhase::Verifying | TeeUpdatePhase::Prepared => self.check_time(now_ns, false),
            TeeUpdatePhase::LiveSwitch | TeeUpdatePhase::HealthWindow => {
                self.check_time(now_ns, true)
            }
            _ => Ok(()),
        }
    }

    pub fn commit_durably(
        &mut self,
        persist: impl FnOnce(TeeCommit) -> CommitOutcome,
    ) -> Result<(), OrchestratorError> {
        self.require_phase(TeeUpdatePhase::CommitPending)?;
        let record = self.pending_commit()?;
        // Publishing the write can change persistent state even if the writer
        // never returns (fault, cancellation, or a lost acknowledgement).
        self.phase = TeeUpdatePhase::CommitUncertain;
        let outcome = persist(record);
        self.finish_commit(outcome)
    }

    pub fn pending_commit(&self) -> Result<TeeCommit, OrchestratorError> {
        let slot = self
            .pending_slot
            .ok_or(OrchestratorError::InvalidTransition)?;
        let generation = self
            .pending_generation
            .ok_or(OrchestratorError::InvalidTransition)?;
        Ok(TeeCommit {
            slot,
            generation,
            image_hash: self.slot_hashes[slot_index(slot)],
        })
    }

    pub fn resolve_commit(&mut self, outcome: CommitOutcome) -> Result<(), OrchestratorError> {
        self.require_phase(TeeUpdatePhase::CommitUncertain)?;
        self.finish_commit(outcome)
    }

    fn finish_commit(&mut self, outcome: CommitOutcome) -> Result<(), OrchestratorError> {
        match outcome {
            CommitOutcome::Committed => {
                self.generation = self
                    .pending_generation
                    .ok_or(OrchestratorError::InvalidTransition)?;
                self.clear_pending();
                self.phase = TeeUpdatePhase::Completed;
                Ok(())
            }
            CommitOutcome::NotCommitted => {
                self.restore_previous()?;
                self.phase = TeeUpdatePhase::RolledBack;
                Err(OrchestratorError::CommitFailed)
            }
            CommitOutcome::Unknown => {
                self.phase = TeeUpdatePhase::CommitUncertain;
                Err(OrchestratorError::CommitUncertain)
            }
        }
    }

    pub fn abort(&mut self) -> Result<(), OrchestratorError> {
        self.rollback()?;
        self.phase = TeeUpdatePhase::Idle;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), OrchestratorError> {
        if self.phase == TeeUpdatePhase::CommitUncertain {
            return Err(OrchestratorError::CommitUncertain);
        }
        self.restore_previous()?;
        self.phase = TeeUpdatePhase::RolledBack;
        Ok(())
    }

    fn restore_previous(&mut self) -> Result<(), OrchestratorError> {
        if self.pending_slot.is_none() {
            return Err(OrchestratorError::InvalidTransition);
        }
        self.active_slot = self
            .rollback_slot
            .ok_or(OrchestratorError::InvalidTransition)?;
        self.clear_pending();
        Ok(())
    }

    fn clear_pending(&mut self) {
        self.pending_slot = None;
        self.pending_generation = None;
        self.preparation_started = None;
        self.cutover_started = None;
        self.reboot_required = false;
    }

    fn require_generation(&self, generation: u64) -> Result<(), OrchestratorError> {
        if self.pending_generation != Some(generation) {
            Err(OrchestratorError::InvalidTransition)
        } else {
            Ok(())
        }
    }

    fn require_phase(&self, phase: TeeUpdatePhase) -> Result<(), OrchestratorError> {
        if self.phase != phase {
            Err(OrchestratorError::InvalidTransition)
        } else {
            Ok(())
        }
    }

    fn check_time(&mut self, now: u64, cutover: bool) -> Result<(), OrchestratorError> {
        let (start, limit) = if cutover {
            (self.cutover_started, CUTOVER_DEADLINE_NS)
        } else {
            (self.preparation_started, PREPARATION_DEADLINE_NS)
        };
        let start = start.ok_or(OrchestratorError::InvalidTransition)?;
        if now < self.last_tick || now.checked_sub(start).is_none_or(|elapsed| elapsed > limit) {
            self.restore_previous()?;
            self.phase = TeeUpdatePhase::RolledBack;
            return Err(OrchestratorError::DeadlineExceeded);
        }
        self.last_tick = now;
        Ok(())
    }
}

const fn slot_index(slot: SlotId) -> usize {
    match slot {
        SlotId::A => 0,
        SlotId::B => 1,
    }
}
