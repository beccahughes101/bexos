#![no_std]

//! Allocation-free secure-runtime coordination for firmware and normal-world
//! orchestrators. This crate deliberately has no kernel or allocator dependency.

mod slot_types;
mod tee_slots;
pub use slot_types::{OrchestratorError, SlotId};
pub use tee_slots::{
    CUTOVER_DEADLINE_NS, CommitOutcome, PREPARATION_DEADLINE_NS, STATE_BYTES, StateIdentity,
    TeeCommit, TeeSlotState, TeeUpdatePhase,
};
