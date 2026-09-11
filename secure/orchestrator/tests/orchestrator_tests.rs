use bexos_kernel_core::transplant::{PreservedRegion, SnapshotHeader, SnapshotPhase};
use bexos_orchestrator::{
    BootRollbackState, ComponentKind, ComponentManifest, ComponentRecord,
    HeartTransplantOrchestrator, OrchestratorError, RollbackStage, SlotId, SlotTable, TeeSlotState,
    TeeUpdatePhase, TransplantState, Watchdog, WatchdogDecision,
};

#[test]
fn ab_table_falls_back_to_last_known_good_slot() {
    let mut slots = SlotTable::new(SlotId::B, [1; 32], [2; 32]);
    slots.slots[1].last_known_good = false;

    assert_eq!(slots.mark_active_failed(1), SlotId::A);
    assert!(!slots.slots[1].bootable);
}

#[test]
fn watchdog_expires_after_missed_heartbeat_threshold() {
    let mut watchdog = Watchdog::new(2);
    watchdog.heartbeat(10);

    assert_eq!(watchdog.observe(15, 10), WatchdogDecision::Healthy);
    assert_eq!(watchdog.observe(26, 10), WatchdogDecision::MissedHeartbeat);
    assert_eq!(watchdog.observe(37, 10), WatchdogDecision::Expired);
}

#[test]
fn heart_transplant_tracks_freeze_replace_and_restore() {
    let slots = SlotTable::new(SlotId::A, [1; 32], [2; 32]);
    let watchdog = Watchdog::new(2);
    let preserved = PreservedRegion::new(0x4600_0000, 0x1000);
    let live_header = SnapshotHeader::new(
        SnapshotPhase::LiveBulk,
        b"appd-state",
        preserved.start,
        preserved.len,
    )
    .expect("small live snapshot");
    let switch_header = SnapshotHeader::new(
        SnapshotPhase::SwitchDelta,
        b"appd-delta",
        preserved.start + 0x100,
        0x100,
    )
    .expect("small switch snapshot");
    let mut orchestrator = HeartTransplantOrchestrator::new(slots, watchdog);

    let live_sync = orchestrator
        .begin_live_sync(preserved, live_header)
        .expect("idle can start live sync");
    assert_eq!(orchestrator.state, TransplantState::LiveSyncing);
    assert_eq!(live_sync.arg0, preserved.start);

    orchestrator
        .complete_live_sync()
        .expect("live sync can complete");
    assert_eq!(orchestrator.state, TransplantState::LiveSyncComplete);

    let switch = orchestrator
        .enter_switch_mode(switch_header)
        .expect("completed live sync can enter switch mode");
    assert_eq!(orchestrator.state, TransplantState::SwitchMode);
    assert_eq!(switch.arg1, preserved.start + 0x100);

    orchestrator
        .request_replacement()
        .expect("switch mode can request replacement");
    assert_eq!(orchestrator.state, TransplantState::ReplacementPending);

    orchestrator
        .kernel_replaced()
        .expect("replacement can complete");
    assert_eq!(orchestrator.state, TransplantState::RestorePending);

    orchestrator
        .restore_complete()
        .expect("restore can complete");
    assert_eq!(orchestrator.state, TransplantState::Restored);
}

#[test]
fn heart_transplant_rejects_out_of_order_and_wrong_phase_snapshots() {
    let slots = SlotTable::new(SlotId::A, [1; 32], [2; 32]);
    let watchdog = Watchdog::new(2);
    let preserved = PreservedRegion::new(0x4600_0000, 0x1000);
    let live_header = SnapshotHeader::new(
        SnapshotPhase::LiveBulk,
        b"appd-state",
        preserved.start,
        preserved.len,
    )
    .expect("small live snapshot");
    let switch_header = SnapshotHeader::new(
        SnapshotPhase::SwitchDelta,
        b"appd-delta",
        preserved.start + 0x100,
        0x100,
    )
    .expect("small switch snapshot");
    let mut orchestrator = HeartTransplantOrchestrator::new(slots, watchdog);

    assert_eq!(
        orchestrator.request_replacement(),
        Err(OrchestratorError::InvalidTransition)
    );
    assert_eq!(
        orchestrator.begin_live_sync(preserved, switch_header),
        Err(OrchestratorError::WrongSnapshotPhase)
    );

    orchestrator
        .begin_live_sync(preserved, live_header)
        .expect("idle can start live sync");
    assert_eq!(
        orchestrator.enter_switch_mode(switch_header),
        Err(OrchestratorError::InvalidTransition)
    );
}

#[test]
fn driver_rollback_selects_lkg_without_changing_kernel_slot() {
    let slots = SlotTable::new(SlotId::A, [0x0a; 32], [0x0b; 32]);
    let components = [
        ComponentRecord::new(ComponentKind::D1Driver, [0xd1; 32], [0x11; 32]),
        ComponentRecord::new(ComponentKind::D2Driver, [0xd2; 32], [0x22; 32]),
    ];
    let mut manifest = ComponentManifest::new(slots, components);

    let decision = manifest
        .mark_component_failed(ComponentKind::D1Driver, 2)
        .expect("driver exists");

    assert_eq!(decision.kind, ComponentKind::D1Driver);
    assert_eq!(decision.selected_hash, [0x11; 32]);
    assert!(!decision.kernel_slot_changed);
    assert_eq!(manifest.kernel_slots.active, SlotId::A);
}

#[test]
fn kernel_failure_uses_slot_table_policy() {
    let mut slots = SlotTable::new(SlotId::B, [0x0a; 32], [0x0b; 32]);
    slots.slots[1].last_known_good = false;
    let components = [ComponentRecord::new(
        ComponentKind::D1Driver,
        [0xd1; 32],
        [0x11; 32],
    )];
    let mut manifest = ComponentManifest::new(slots, components);

    let decision = manifest
        .mark_component_failed(ComponentKind::Kernel, 1)
        .expect("kernel rollback is slot-backed");

    assert!(decision.kernel_slot_changed);
    assert_eq!(manifest.kernel_slots.active, SlotId::A);
    assert_eq!(decision.selected_hash, [0x0a; 32]);
}

#[test]
fn boot_rollback_state_stages_commits_and_aborts_generations() {
    let mut state = BootRollbackState::new(7, [1; 32], [2; 32]);
    assert_eq!(
        state.verify_boot(6, [1; 32]),
        Err(OrchestratorError::Rollback)
    );
    assert_eq!(
        state.verify_boot(7, [9; 32]),
        Err(OrchestratorError::InvalidState)
    );
    state.stage_update(8, [3; 32]).unwrap();
    assert_eq!(
        state.stage,
        RollbackStage::Pending {
            generation: 8,
            artifact_hash: [3; 32]
        }
    );
    state.abort_update();
    assert_eq!(state.stage, RollbackStage::Idle);
    state.stage_update(9, [4; 32]).unwrap();
    state.commit_update().unwrap();
    assert_eq!(state.min_generation, 9);
    assert_eq!(state.active_hash, [4; 32]);
    assert_eq!(state.last_known_good_hash, [1; 32]);
}

#[test]
fn tee_live_switch_preserves_rollback_slot_until_health_confirmation() {
    let mut tee = TeeSlotState::new(SlotId::A, 4, [0xaa; 32], [0xbb; 32]);

    assert_eq!(tee.stage(5, [0xcc; 32]), Ok(SlotId::B));
    assert_eq!(tee.prepare(5, 0), Ok(()));
    tee.candidate_ready(1).unwrap();
    tee.quiesce(2).unwrap();
    assert_eq!(tee.live_switch(5, 3), Ok(()));
    assert_eq!(tee.active_slot, SlotId::B);
    assert_eq!(tee.rollback_slot, Some(SlotId::A));
    assert_eq!(tee.phase, TeeUpdatePhase::HealthWindow);

    assert_eq!(tee.confirm_health(4), Ok(()));
    assert_eq!(tee.generation, 4);
    assert_eq!(tee.phase, TeeUpdatePhase::CommitPending);
    tee.commit_durably(|_| bexos_orchestrator::CommitOutcome::Committed)
        .unwrap();
    assert_eq!(tee.generation, 5);
    assert_eq!(tee.phase, TeeUpdatePhase::Completed);
    assert_eq!(tee.pending_slot, None);
    assert_eq!(tee.rollback_slot, Some(SlotId::A));
}

#[test]
fn tee_reboot_activation_marks_pending_slot_until_health_confirmation() {
    let mut tee = TeeSlotState::new(SlotId::A, 10, [0xaa; 32], [0xbb; 32]);

    assert_eq!(tee.stage(11, [0xcc; 32]), Ok(SlotId::B));
    assert_eq!(tee.prepare(11, 0), Ok(()));
    tee.candidate_ready(1).unwrap();
    assert_eq!(tee.boot_activate(11, 2), Ok(()));
    assert_eq!(tee.active_slot, SlotId::A);
    assert_eq!(tee.pending_slot, Some(SlotId::B));
    assert!(tee.reboot_required);
    assert_eq!(tee.phase, TeeUpdatePhase::RebootPending);

    tee.boot_entered(11, 0).unwrap();
    assert_eq!(tee.confirm_health(1), Ok(()));
    assert_eq!(tee.generation, 10);
    tee.commit_durably(|_| bexos_orchestrator::CommitOutcome::Committed)
        .unwrap();
    assert_eq!(tee.active_slot, SlotId::B);
    assert!(!tee.reboot_required);
}

#[test]
fn tee_update_rejects_rollback_generation_and_can_abort() {
    let mut tee = TeeSlotState::new(SlotId::B, 12, [0xaa; 32], [0xbb; 32]);

    assert_eq!(
        tee.stage(12, [0xcc; 32]),
        Err(OrchestratorError::RollbackGeneration)
    );
    assert_eq!(tee.phase, TeeUpdatePhase::RolledBack);

    assert_eq!(tee.stage(13, [0xdd; 32]), Ok(SlotId::A));
    tee.abort().unwrap();
    assert_eq!(tee.pending_slot, None);
    assert_eq!(tee.phase, TeeUpdatePhase::Idle);
}

#[test]
fn tee_health_failure_rolls_back_to_previous_slot() {
    let mut tee = TeeSlotState::new(SlotId::A, 20, [0xaa; 32], [0xbb; 32]);

    tee.stage(21, [0xcc; 32]).unwrap();
    tee.prepare(21, 0).unwrap();
    tee.candidate_ready(1).unwrap();
    tee.quiesce(2).unwrap();
    tee.live_switch(21, 3).unwrap();
    assert_eq!(tee.rollback(), Ok(()));
    assert_eq!(tee.active_slot, SlotId::A);
    assert_eq!(tee.phase, TeeUpdatePhase::RolledBack);
}
