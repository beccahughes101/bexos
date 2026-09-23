use bexos_tee_slots::{
    CommitOutcome, OrchestratorError, SlotId, TeeUpdatePhase,
    live::{CandidateReport, Error, Live},
    writer::{self, Outcome},
};

fn new() -> Live {
    Live::new(1, 1, SlotId::A, [1; 32], 40, 7, 3).unwrap()
}
fn ready(live: &mut Live, candidate: u64) {
    let watermark = live.storage_watermark();
    live.imported(candidate, watermark).unwrap();
    live.service_ready(candidate, 1, watermark).unwrap();
    live.service_ready(candidate, 2, watermark).unwrap();
}
fn switch(live: &mut Live) {
    live.begin(2, 2, [2; 32], 0).unwrap();
    ready(live, 2);
    live.prepared(1).unwrap();
    live.quiesce(2).unwrap();
    ready(live, 2);
    live.switch(3).unwrap();
    ready(live, 2);
    live.healthy(4).unwrap();
}
#[test]
fn candidate_can_write_only_after_durable_commit_and_old_tickets_cannot_replay() {
    let mut live = new();
    let old_ticket = live.begin_write(1, 7).unwrap();
    live.finish_write(&old_ticket, Outcome::Committed).unwrap();
    assert_eq!(live.storage_watermark(), 41);
    switch(&mut live);
    assert_eq!(
        live.begin_write(2, 7),
        Err(Error::Storage(writer::Error::Denied))
    );
    assert_eq!(
        live.begin_write(1, 7),
        Err(Error::Storage(writer::Error::Busy))
    );
    assert_eq!(live.generation(), 1);
    live.commit(|record| {
        assert_eq!(
            (
                record.image.slot,
                record.image.generation,
                record.image.image_hash
            ),
            (SlotId::B, 2, [2; 32])
        );
        assert_eq!((record.source_owner, record.candidate_owner), (1, 2));
        assert_eq!((record.storage_epoch, record.storage_watermark), (8, 41));
        assert_eq!(record.transport_epoch, 8);
        CommitOutcome::Committed
    })
    .unwrap();
    assert_eq!(live.generation(), 2);
    assert_eq!(live.writer_owner(), 2);
    assert_eq!(live.storage_epoch(), 8);
    assert_eq!(
        live.begin_write(1, 7),
        Err(Error::Storage(writer::Error::Denied))
    );
    assert_eq!(
        live.finish_write(&old_ticket, Outcome::Committed),
        Err(Error::Storage(writer::Error::Stale))
    );
    let ticket = live.begin_write(2, 8).unwrap();
    live.finish_write(&ticket, Outcome::Committed).unwrap();
    assert_eq!(live.storage_watermark(), 42);
    assert_eq!(
        live.commit(|_| panic!("repeated commit called persistence")),
        Err(Error::Lifecycle(OrchestratorError::InvalidTransition))
    );
    assert!(!live.recovery_required());
    live.begin(3, 3, [3; 32], 5).unwrap();
    ready(&mut live, 3);
    live.prepared(6).unwrap();
    live.quiesce(7).unwrap();
    ready(&mut live, 3);
    live.switch(8).unwrap();
    ready(&mut live, 3);
    live.healthy(9).unwrap();
    live.commit(|_| CommitOutcome::Committed).unwrap();
    assert_eq!(live.writer_owner(), 3);
    assert_eq!(live.generation(), 3);
    assert_eq!(live.storage_epoch(), 9);
}
#[test]
fn concurrent_write_invalidates_snapshot_and_drain_precedes_switch() {
    let mut live = new();
    live.begin(2, 2, [2; 32], 0).unwrap();
    ready(&mut live, 2);
    let ticket = live.begin_write(1, 7).unwrap();
    assert_eq!(live.prepared(1), Err(Error::NotReady));
    live.finish_write(&ticket, Outcome::Committed).unwrap();
    assert_eq!(live.prepared(2), Err(Error::NotReady));
    ready(&mut live, 2);
    live.prepared(3).unwrap();
    let ticket = live.begin_write(1, 7).unwrap();
    live.quiesce(4).unwrap();
    assert_eq!(
        live.begin_write(1, 7),
        Err(Error::Storage(writer::Error::Busy))
    );
    assert_eq!(live.switch(5), Err(Error::NotReady));
    live.finish_write(&ticket, Outcome::Committed).unwrap();
    assert_eq!(live.switch(6), Err(Error::NotReady));
    ready(&mut live, 2);
    live.switch(7).unwrap();
    assert_eq!(live.healthy(8), Err(Error::NotReady));
    ready(&mut live, 2);
    live.healthy(9).unwrap();
}

#[test]
fn stale_candidate_does_not_change_committed_owner_or_protected_state() {
    use bexos_tee_slots::{StateIdentity, live::STATE_BYTES};
    let mut live = new();
    switch(&mut live);
    live.commit(|_| CommitOutcome::Committed).unwrap();
    let mut before = [0; STATE_BYTES];
    live.snapshot(StateIdentity::ArmTrusty, &mut before)
        .unwrap();
    for generation in [0, 1, 2] {
        assert_eq!(
            live.begin(3, generation, [3; 32], 10),
            Err(Error::Lifecycle(OrchestratorError::RollbackGeneration))
        );
        let mut after = [0; STATE_BYTES];
        live.snapshot(StateIdentity::ArmTrusty, &mut after).unwrap();
        assert_eq!(before, after);
        assert_eq!(live.writer_owner(), 2);
    }
}

#[test]
fn candidate_report_must_match_migration_abi_before_preparation_state_changes() {
    use bexos_tee_slots::{StateIdentity, live::STATE_BYTES};
    let mut live = new();
    let mut before = [0; STATE_BYTES];
    live.snapshot(StateIdentity::ArmTrusty, &mut before)
        .unwrap();
    assert_eq!(
        live.begin_reported(
            CandidateReport {
                owner: 2,
                generation: 2,
                migration_abi: 99,
                digest: [2; 32],
            },
            0,
        ),
        Err(Error::Invalid)
    );
    let mut after = [0; STATE_BYTES];
    live.snapshot(StateIdentity::ArmTrusty, &mut after).unwrap();
    assert_eq!(before, after);
    assert_eq!(live.phase(), TeeUpdatePhase::Idle);
    assert_eq!(live.writer_owner(), 1);
    live.begin_reported(
        CandidateReport {
            owner: 2,
            generation: 2,
            migration_abi: 1,
            digest: [2; 32],
        },
        0,
    )
    .unwrap();
    assert_eq!(live.phase(), TeeUpdatePhase::Verifying);
}
#[test]
fn uncertain_commit_fences_both_owners_until_authenticated_resolution() {
    for outcome in [CommitOutcome::Committed, CommitOutcome::NotCommitted] {
        let mut live = new();
        switch(&mut live);
        let expected = live.pending_commit().unwrap();
        assert_eq!(
            live.commit(|_| CommitOutcome::Unknown),
            Err(Error::Lifecycle(OrchestratorError::CommitUncertain))
        );
        assert_eq!(live.phase(), TeeUpdatePhase::CommitUncertain);
        assert_eq!(live.pending_commit().unwrap(), expected);
        assert_eq!(
            live.abort(),
            Err(Error::Lifecycle(OrchestratorError::CommitUncertain))
        );
        assert_eq!(
            live.begin_write(1, 7),
            Err(Error::Storage(writer::Error::Busy))
        );
        assert_eq!(
            live.begin_write(2, 8),
            Err(Error::Storage(writer::Error::Denied))
        );
        let result = live.resolve_commit(outcome);
        if outcome == CommitOutcome::Committed {
            result.unwrap();
            assert_eq!(live.writer_owner(), 2);
        } else {
            assert_eq!(
                result,
                Err(Error::Lifecycle(OrchestratorError::CommitFailed))
            );
            assert_eq!(live.writer_owner(), 1);
            assert_eq!(live.generation(), 1);
            assert_eq!(live.transport_epoch(), 9);
            assert!(live.begin_write(1, 7).is_ok());
        }
    }
}
#[test]
fn lost_storage_reply_must_be_resolved_before_snapshot_or_cutover() {
    let mut live = new();
    live.begin(2, 2, [2; 32], 0).unwrap();
    let ticket = live.begin_write(1, 7).unwrap();
    assert_eq!(
        live.finish_write(&ticket, Outcome::Unknown),
        Err(Error::Storage(writer::Error::Uncertain))
    );
    assert_eq!(
        live.finish_write(&ticket, Outcome::NotCommitted),
        Err(Error::Storage(writer::Error::Uncertain))
    );
    ready(&mut live, 2);
    assert_eq!(live.prepared(1), Err(Error::NotReady));
    live.resolve_write(&ticket, Outcome::Committed).unwrap();
    assert_eq!(live.storage_watermark(), 41);
    ready(&mut live, 2);
    live.prepared(2).unwrap();
    live.quiesce(3).unwrap();
    ready(&mut live, 2);
    live.switch(4).unwrap();
}
#[test]
fn owner_timer_rolls_back_hung_candidate_without_relaxing_deadlines() {
    let mut live = new();
    live.begin(2, 2, [2; 32], 100).unwrap();
    assert_eq!(
        live.poll(30_000_000_101),
        Err(Error::Lifecycle(OrchestratorError::DeadlineExceeded))
    );
    assert_eq!(live.generation(), 1);
    assert!(live.begin_write(1, 7).is_ok());
    let mut live = new();
    live.begin(2, 2, [2; 32], 0).unwrap();
    ready(&mut live, 2);
    live.prepared(1).unwrap();
    live.quiesce(2).unwrap();
    assert_eq!(
        live.poll(150_000_003),
        Err(Error::Lifecycle(OrchestratorError::DeadlineExceeded))
    );
    assert_eq!(live.phase(), TeeUpdatePhase::RolledBack);
    assert_eq!(live.writer_owner(), 1);
    assert!(live.begin_write(1, 7).is_ok());
}

#[test]
fn expired_preparation_can_resume_source_after_its_lost_write_is_resolved() {
    use bexos_tee_slots::{StateIdentity, live::STATE_BYTES};
    let mut live = new();
    live.begin(2, 2, [2; 32], 0).unwrap();
    let ticket = live.begin_write(1, 7).unwrap();
    live.finish_write(&ticket, Outcome::Unknown).unwrap_err();
    assert_eq!(live.poll(30_000_000_001), Err(Error::RecoveryRequired));
    assert_eq!(live.phase(), TeeUpdatePhase::RolledBack);
    assert_eq!(live.transport_epoch(), 7);
    assert!(live.begin_write(1, 7).is_err());
    let mut bytes = [0; STATE_BYTES];
    live.snapshot(StateIdentity::X86Trusty, &mut bytes).unwrap();
    drop(ticket);
    drop(live);
    let mut restored =
        Live::restore_protected(&bytes, StateIdentity::X86Trusty, 30_000_000_002).unwrap();
    let ticket = restored.pending_write_ticket().unwrap();
    restored.resolve_write(&ticket, Outcome::Committed).unwrap();
    assert!(restored.pending_write_ticket().is_none());
    assert!(restored.resolve_write(&ticket, Outcome::Committed).is_err());
    assert!(!restored.recovery_required());
    assert_eq!(restored.transport_epoch(), 8);
    assert_eq!(restored.storage_watermark(), 41);
    assert_eq!(restored.generation(), 1);
    assert_eq!(restored.writer_owner(), 1);
    assert!(restored.begin_write(2, 7).is_err());
    assert!(restored.begin_write(1, 7).is_ok());
}

#[test]
fn protected_handoff_retains_fences_uncertainty_and_original_deadlines() {
    use bexos_tee_slots::{StateIdentity, live::STATE_BYTES};
    for identity in [StateIdentity::ArmTrusty, StateIdentity::X86Trusty] {
        let mut live = new();
        switch(&mut live);
        live.commit(|_| CommitOutcome::Unknown).unwrap_err();
        let mut bytes = [0; STATE_BYTES];
        live.snapshot(identity, &mut bytes).unwrap();
        let mut restored = Live::restore_protected(&bytes, identity, 5).unwrap();
        assert_eq!(restored.phase(), TeeUpdatePhase::CommitUncertain);
        assert!(restored.begin_write(1, 7).is_err());
        assert!(restored.begin_write(2, 8).is_err());
        restored.resolve_commit(CommitOutcome::Committed).unwrap();
        assert_eq!(restored.writer_owner(), 2);
        for i in 0..bytes.len() {
            let mut corrupted = bytes;
            corrupted[i] ^= 1;
            assert!(Live::restore_protected(&corrupted, identity, 5).is_err());
        }
        let wrong = if identity == StateIdentity::ArmTrusty {
            StateIdentity::X86Trusty
        } else {
            StateIdentity::ArmTrusty
        };
        assert!(Live::restore_protected(&bytes, wrong, 5).is_err());
        let mut live = new();
        live.begin(2, 2, [2; 32], 100).unwrap();
        live.snapshot(identity, &mut bytes).unwrap();
        assert!(Live::restore_protected(&bytes, identity, 99).is_err());
        assert!(Live::restore_protected(&bytes, identity, 30_000_000_101).is_err());
    }
}
