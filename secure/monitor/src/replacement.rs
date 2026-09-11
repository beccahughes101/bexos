//! Monitor-owned replacement coordination. Guest requests cannot manufacture
//! verification, candidate readiness, successful execution, or durable writes.
//! The product owner must deliver each lifecycle event after doing that work.

use bexos_secure_monitor_abi::{ACTIVATE_LIVE_NOW, Request, SHARED_READ, Status};
use bexos_tee_slots::{
    CommitOutcome, OrchestratorError, SlotId, TeeCommit, TeeSlotState, TeeUpdatePhase,
};

use crate::shared::{BufferLease, Caller, DomainId, Registry};

/// Installed by the trusted product owner, never selected by a guest request.
/// A successful verifier must retain an immutable monitor-owned copy of the
/// image, authenticate its signature, architecture and generation, and return
/// its cryptographic digest. Registration alone does not prevent guest writes.
/// The copy must survive unregistering the source until commit or rollback.
pub trait ImageVerifier {
    fn snapshot_and_verify(
        &mut self,
        image: BufferLease<'_>,
        generation: u64,
    ) -> Result<[u8; 32], Status>;
}

/// Default until a product verifier is installed. Never dereferences host
/// addresses merely because their integer ranges passed registry validation.
pub struct UnavailableVerifier;
impl ImageVerifier for UnavailableVerifier {
    fn snapshot_and_verify(
        &mut self,
        _image: BufferLease<'_>,
        _generation: u64,
    ) -> Result<[u8; 32], Status> {
        Err(Status::Unsupported)
    }
}

pub struct SecureRuntime<const WINDOWS: usize, const SLOTS: usize> {
    registry: Registry<WINDOWS, SLOTS>,
    slots: TeeSlotState,
    update_owner: DomainId,
    activation: Option<u64>,
}

impl<const W: usize, const S: usize> SecureRuntime<W, S> {
    pub fn new(
        registry: Registry<W, S>,
        update_owner: DomainId,
        active_slot: SlotId,
        active_generation: u64,
        active_hash: [u8; 32],
    ) -> Self {
        let (a_hash, b_hash) = match active_slot {
            SlotId::A => (active_hash, [0; 32]),
            SlotId::B => ([0; 32], active_hash),
        };
        Self {
            registry,
            slots: TeeSlotState::new(active_slot, active_generation, a_hash, b_hash),
            update_owner,
            activation: None,
        }
    }

    pub fn state(&self) -> &TeeSlotState {
        &self.slots
    }

    /// `now_ns` is read by the monitor from its clock, never from guest registers.
    /// Live activation returns Busy until actual lifecycle events complete it;
    /// the caller may poll the same generation without restarting its deadline.
    pub fn dispatch(
        &mut self,
        caller: Caller,
        registers: [u64; 8],
        now_ns: u64,
        verifier: &mut impl ImageVerifier,
    ) -> [u64; 8] {
        match self.request(caller, registers, now_ns, verifier) {
            Ok(value) => Status::Ok.registers(value),
            Err(status) => status.registers(0),
        }
    }

    fn request(
        &mut self,
        caller: Caller,
        registers: [u64; 8],
        now_ns: u64,
        verifier: &mut impl ImageVerifier,
    ) -> Result<u64, Status> {
        let request = Request::decode(registers)?;
        if matches!(
            request,
            Request::Register { .. } | Request::RegisterPinned { .. } | Request::Unregister { .. }
        ) {
            return self.registry.request(caller, registers);
        }
        if !caller.may_share || caller.domain != self.update_owner {
            return Err(Status::AccessDenied);
        }
        match request {
            Request::StageTrustyCore {
                handle,
                length,
                generation,
            } => {
                if self.slots.pending_slot.is_some() {
                    return Err(Status::Busy);
                }
                if generation <= self.slots.generation {
                    return Err(Status::AccessDenied);
                }
                let lease = self
                    .registry
                    .acquire(caller, handle, 0, length, SHARED_READ)?;
                let digest = verifier.snapshot_and_verify(lease, generation)?;
                let slot = self
                    .slots
                    .stage(generation, digest)
                    .map_err(runtime_status)?;
                self.activation = None;
                Ok(match slot {
                    SlotId::A => 0,
                    SlotId::B => 1,
                })
            }
            Request::ActivateTrustyCore {
                generation,
                activation,
            } => {
                // Scheduling a reboot requires an authenticated persistent
                // boot-selection backend that is not installed here yet.
                if activation != ACTIVATE_LIVE_NOW {
                    return Err(Status::Unsupported);
                }
                if self.slots.phase == TeeUpdatePhase::Completed
                    && self.slots.generation == generation
                    && self.activation == Some(activation)
                {
                    return Ok(generation);
                }
                let pending = self.slots.pending_commit().map_err(runtime_status)?;
                if pending.generation != generation {
                    return Err(Status::InvalidArgs);
                }
                if let Some(previous) = self.activation {
                    if previous != activation {
                        return Err(Status::InvalidArgs);
                    }
                } else {
                    self.slots
                        .prepare(generation, now_ns)
                        .map_err(runtime_status)?;
                    self.activation = Some(activation);
                }
                self.slots.poll_deadline(now_ns).map_err(runtime_status)?;
                Err(Status::Busy)
            }
            _ => Err(Status::Unsupported),
        }
    }

    /// Product owner calls this only after candidate execution has prestarted
    /// with a consistent storage snapshot and caught up with durable writes.
    pub fn candidate_ready(&mut self, now_ns: u64) -> Result<(), Status> {
        self.slots.candidate_ready(now_ns).map_err(runtime_status)
    }

    /// Call before quiescing clients. This starts the 150 ms readiness budget.
    pub fn begin_cutover(&mut self, now_ns: u64) -> Result<(), Status> {
        if self.activation != Some(ACTIVATE_LIVE_NOW) {
            return Err(Status::InvalidArgs);
        }
        self.slots.quiesce(now_ns).map_err(runtime_status)
    }

    /// Candidate execution must actually own the services before this event.
    /// The previous owner and its memory remain retained through durable commit.
    pub fn candidate_switched(&mut self, now_ns: u64) -> Result<(), Status> {
        let generation = self
            .slots
            .pending_commit()
            .map_err(runtime_status)?
            .generation;
        self.slots
            .live_switch(generation, now_ns)
            .map_err(runtime_status)
    }

    pub fn confirm_service_readiness(&mut self, now_ns: u64) -> Result<(), Status> {
        self.slots.confirm_health(now_ns).map_err(runtime_status)
    }

    /// The callback writes authenticated persistent state atomically. It must
    /// return Unknown on lost acknowledgement, never infer success from equality
    /// of the proposed record. No callback runs before readiness is confirmed.
    pub fn commit(
        &mut self,
        persist: impl FnOnce(TeeCommit) -> CommitOutcome,
    ) -> Result<(), Status> {
        self.slots.commit_durably(persist).map_err(runtime_status)
    }

    /// Supply only the outcome established by authenticated storage recovery.
    pub fn resolve_commit(&mut self, outcome: CommitOutcome) -> Result<(), Status> {
        self.slots.resolve_commit(outcome).map_err(runtime_status)
    }

    /// The owner must fence candidate writes and restore the previous execution
    /// owner when rollback occurs. Uncertain durable commits prohibit rollback.
    pub fn rollback(&mut self) -> Result<(), Status> {
        self.slots.rollback().map_err(runtime_status)
    }

    /// Timer entry for silent candidates, independent of guest polling.
    pub fn poll_deadline(&mut self, now_ns: u64) -> Result<(), Status> {
        self.slots.poll_deadline(now_ns).map_err(runtime_status)
    }
}

fn runtime_status(error: OrchestratorError) -> Status {
    match error {
        OrchestratorError::InvalidTransition | OrchestratorError::InvalidState => {
            Status::InvalidArgs
        }
        OrchestratorError::RollbackGeneration | OrchestratorError::Rollback => Status::AccessDenied,
        OrchestratorError::DeadlineExceeded
        | OrchestratorError::CommitFailed
        | OrchestratorError::CommitUncertain => Status::Busy,
        OrchestratorError::ComponentNotFound | OrchestratorError::WrongSnapshotPhase => {
            Status::Unsupported
        }
    }
}

#[cfg(test)]
#[path = "replacement_tests.rs"]
mod tests;
