use bexos_orchestrator::{
    CUTOVER_DEADLINE_NS, CommitOutcome, OrchestratorError, PREPARATION_DEADLINE_NS, SlotId,
    TeeSlotState, TeeUpdatePhase,
};

#[test]
fn protected_handoff_preserves_uncertain_commit_and_original_deadlines() {
    use bexos_orchestrator::{STATE_BYTES, StateIdentity};
    let mut bytes = [0; STATE_BYTES];
    let pending = staged();
    pending
        .snapshot(StateIdentity::X86Trusty, &mut bytes)
        .unwrap();
    let mut resumed =
        TeeSlotState::restore_protected(&bytes, StateIdentity::X86Trusty, 200).unwrap();
    assert_eq!(resumed.generation, 7);
    assert_eq!(
        resumed.candidate_ready(100 + PREPARATION_DEADLINE_NS + 1),
        Err(OrchestratorError::DeadlineExceeded)
    );
    assert_eq!(resumed.active_slot, SlotId::A);
    assert_eq!(
        TeeSlotState::restore_protected(&bytes, StateIdentity::X86Trusty, 99),
        Err(OrchestratorError::DeadlineExceeded)
    );

    let mut pending = running();
    pending
        .snapshot(StateIdentity::X86Hypervisor, &mut bytes)
        .unwrap();
    assert_eq!(
        TeeSlotState::restore_protected(
            &bytes,
            StateIdentity::X86Hypervisor,
            300 + CUTOVER_DEADLINE_NS + 1
        ),
        Err(OrchestratorError::DeadlineExceeded)
    );
    pending.confirm_health(500).unwrap();
    assert_eq!(
        pending.commit_durably(|_| CommitOutcome::Unknown),
        Err(OrchestratorError::CommitUncertain)
    );
    pending
        .snapshot(StateIdentity::X86Hypervisor, &mut bytes)
        .unwrap();
    let mut resumed =
        TeeSlotState::restore_protected(&bytes, StateIdentity::X86Hypervisor, 40_000_000_000)
            .unwrap();
    assert_eq!(resumed.generation, 7);
    assert_eq!(resumed.active_slot, SlotId::B);
    assert_eq!(resumed.rollback_slot, Some(SlotId::A));
    assert_eq!(resumed.rollback(), Err(OrchestratorError::CommitUncertain));
    resumed.resolve_commit(CommitOutcome::Committed).unwrap();
    assert_eq!(resumed.generation, 8);
}

#[test]
fn protected_handoff_rejects_scope_corruption_and_inconsistent_lifecycle() {
    use bexos_orchestrator::{STATE_BYTES, StateIdentity};
    use sha2::{Digest, Sha256};
    let mut bytes = [0; STATE_BYTES];
    staged()
        .snapshot(StateIdentity::X86Trusty, &mut bytes)
        .unwrap();
    for identity in [StateIdentity::ArmTrusty, StateIdentity::X86Hypervisor] {
        assert_eq!(
            TeeSlotState::restore_protected(&bytes, identity, 100),
            Err(OrchestratorError::InvalidState)
        );
    }
    for index in 0..STATE_BYTES {
        let mut corrupted = bytes;
        corrupted[index] ^= 1;
        assert_eq!(
            TeeSlotState::restore_protected(&corrupted, StateIdentity::X86Trusty, 100),
            Err(OrchestratorError::InvalidState)
        );
    }
    // Even a record with a correctly recomputed corruption checksum must obey
    // lifecycle invariants; the checksum is not an authorization mechanism.
    for (index, value) in [(16, 7), (18, 1), (19, 2), (20, 0), (21, 1), (32, 7)] {
        let mut invalid = bytes;
        invalid[index] = value;
        let digest = Sha256::digest(&invalid[..128]);
        invalid[128..].copy_from_slice(&digest);
        assert_eq!(
            TeeSlotState::restore_protected(&invalid, StateIdentity::X86Trusty, 100),
            Err(OrchestratorError::InvalidState)
        );
    }
}

fn staged() -> TeeSlotState {
    let mut state = TeeSlotState::new(SlotId::A, 7, [7; 32], [0; 32]);
    state.stage(8, [8; 32]).unwrap();
    state.prepare(8, 100).unwrap();
    state
}

fn running() -> TeeSlotState {
    let mut state = staged();
    state.candidate_ready(200).unwrap();
    state.quiesce(300).unwrap();
    state.live_switch(8, 400).unwrap();
    state
}

#[test]
fn generation_is_unchanged_until_atomic_storage_commit_succeeds() {
    let mut state = running();
    assert_eq!(state.generation, 7);
    assert_eq!(
        state.commit_durably(|_| panic!("must not commit before health")),
        Err(OrchestratorError::InvalidTransition)
    );
    state.confirm_health(500).unwrap();
    assert_eq!(state.generation, 7);
    state
        .commit_durably(|record| {
            assert_eq!(record.slot, SlotId::B);
            assert_eq!(record.generation, 8);
            assert_eq!(record.image_hash, [8; 32]);
            CommitOutcome::Committed
        })
        .unwrap();
    assert_eq!(state.generation, 8);
    assert_eq!(state.phase, TeeUpdatePhase::Completed);
    assert_eq!(state.rollback(), Err(OrchestratorError::InvalidTransition));
    assert_eq!(
        state.commit_durably(|_| panic!("no repeated commit")),
        Err(OrchestratorError::InvalidTransition)
    );
}

#[test]
fn lost_commit_acknowledgement_blocks_rollback_until_recovery() {
    for recovered in [CommitOutcome::Committed, CommitOutcome::NotCommitted] {
        let mut state = running();
        state.confirm_health(500).unwrap();
        assert_eq!(
            state.commit_durably(|_| CommitOutcome::Unknown),
            Err(OrchestratorError::CommitUncertain)
        );
        assert_eq!(state.generation, 7);
        assert_eq!(state.pending_slot, Some(SlotId::B));
        assert_eq!(state.rollback_slot, Some(SlotId::A));
        assert_eq!(state.abort(), Err(OrchestratorError::CommitUncertain));
        assert_eq!(state.rollback(), Err(OrchestratorError::CommitUncertain));
        assert_eq!(
            state.stage(9, [9; 32]),
            Err(OrchestratorError::InvalidTransition)
        );
        let result = state.resolve_commit(recovered);
        if recovered == CommitOutcome::Committed {
            result.unwrap();
            assert_eq!(state.generation, 8);
            assert_eq!(state.active_slot, SlotId::B);
        } else {
            assert_eq!(result, Err(OrchestratorError::CommitFailed));
            assert_eq!(state.generation, 7);
            assert_eq!(state.active_slot, SlotId::A);
        }
    }
}

#[test]
fn preparation_cannot_extend_or_restart_the_thirty_second_budget() {
    let mut state = staged();
    assert_eq!(
        state.prepare(8, 200),
        Err(OrchestratorError::InvalidTransition)
    );
    assert_eq!(
        state.candidate_ready(100 + PREPARATION_DEADLINE_NS + 1),
        Err(OrchestratorError::DeadlineExceeded)
    );
    assert_eq!(state.active_slot, SlotId::A);
    assert_eq!(state.generation, 7);
    let mut state = staged();
    state
        .candidate_ready(100 + PREPARATION_DEADLINE_NS)
        .unwrap();
    assert_eq!(
        state.quiesce(100 + PREPARATION_DEADLINE_NS + 1),
        Err(OrchestratorError::DeadlineExceeded)
    );
}

#[test]
fn cutover_deadline_includes_service_readiness_after_switch() {
    let mut state = running();
    assert_eq!(
        state.confirm_health(300 + CUTOVER_DEADLINE_NS + 1),
        Err(OrchestratorError::DeadlineExceeded)
    );
    assert_eq!(state.active_slot, SlotId::A);
    assert_eq!(state.generation, 7);
    assert_eq!(state.phase, TeeUpdatePhase::RolledBack);
    let mut boundary = running();
    boundary.confirm_health(300 + CUTOVER_DEADLINE_NS).unwrap();
    assert_eq!(boundary.generation, 7);
}

#[test]
fn stale_generation_duplicate_stage_and_out_of_order_events_cannot_change_owner() {
    let mut state = staged();
    assert_eq!(
        state.live_switch(8, 200),
        Err(OrchestratorError::InvalidTransition)
    );
    assert_eq!(
        state.confirm_health(200),
        Err(OrchestratorError::InvalidTransition)
    );
    assert_eq!(
        state.stage(9, [9; 32]),
        Err(OrchestratorError::InvalidTransition)
    );
    state.candidate_ready(200).unwrap();
    state.quiesce(300).unwrap();
    assert_eq!(
        state.live_switch(9, 400),
        Err(OrchestratorError::InvalidTransition)
    );
    state.live_switch(8, 400).unwrap();
    state.abort().unwrap();
    assert_eq!(state.active_slot, SlotId::A);
    assert_eq!(state.generation, 7);
}

#[test]
fn regressing_clock_restores_the_old_owner() {
    let mut state = running();
    assert_eq!(
        state.confirm_health(399),
        Err(OrchestratorError::DeadlineExceeded)
    );
    assert_eq!(state.active_slot, SlotId::A);
    assert_eq!(state.generation, 7);
}

#[test]
fn owner_timer_can_abort_a_silent_candidate() {
    let mut prepare = staged();
    assert_eq!(
        prepare.poll_deadline(101 + PREPARATION_DEADLINE_NS),
        Err(OrchestratorError::DeadlineExceeded)
    );
    assert_eq!(prepare.active_slot, SlotId::A);
    let mut cutover = running();
    assert_eq!(
        cutover.poll_deadline(301 + CUTOVER_DEADLINE_NS),
        Err(OrchestratorError::DeadlineExceeded)
    );
    assert_eq!(cutover.active_slot, SlotId::A);
}

#[test]
fn a_writer_fault_cannot_make_a_potentially_committed_candidate_rollbackable() {
    let mut state = running();
    state.confirm_health(500).unwrap();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = state.commit_durably(|_| panic!("interrupted storage acknowledgement"));
    }));
    assert!(failure.is_err());
    assert_eq!(state.phase, TeeUpdatePhase::CommitUncertain);
    assert_eq!(state.abort(), Err(OrchestratorError::CommitUncertain));
    state.resolve_commit(CommitOutcome::Committed).unwrap();
    assert_eq!(state.generation, 8);
}
