//! Shared allocation-free secure-runtime identifiers and errors.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotId {
    A,
    B,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrchestratorError {
    InvalidTransition,
    WrongSnapshotPhase,
    ComponentNotFound,
    Rollback,
    InvalidState,
    RollbackGeneration,
    DeadlineExceeded,
    CommitFailed,
    CommitUncertain,
}
