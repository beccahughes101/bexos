//! Kernel-side mirror of the Trusty orchestrator handover policy.
//!
//! The secure-world orchestrator remains authoritative for Trusty lifecycle
//! state. This mirror validates the non-secure snapshot transition before the
//! kernel commits its local handover state; Trusty IPC itself is owned by the
//! heart-transplantable `teed` transport.
use bexos_kernel_core::transplant::{
    KernelTransplantHandoff, PreservedRegion, SnapshotHeader, SnapshotPhase,
};
use bexos_orchestrator::{
    HeartTransplantOrchestrator, SlotId, SlotTable, TransplantState, Watchdog,
};

fn model(h: &KernelTransplantHandoff) -> HeartTransplantOrchestrator {
    HeartTransplantOrchestrator::new(
        SlotTable::new(SlotId::A, [0; 32], h.artifact_hash),
        Watchdog::new(3),
    )
}
pub fn authorize(h: &KernelTransplantHandoff, bulk: &[u8], delta_bytes: &[u8]) -> Result<(), ()> {
    let mut o = model(h);
    let preserved = PreservedRegion::new(h.snapshot.start, h.snapshot.len);
    let live = SnapshotHeader::new(
        SnapshotPhase::LiveBulk,
        bulk,
        preserved.start,
        bulk.len() as u64,
    )
    .map_err(|_| ())?;
    let delta = SnapshotHeader::new(
        SnapshotPhase::SwitchDelta,
        delta_bytes,
        preserved.start + bulk.len() as u64,
        delta_bytes.len() as u64,
    )
    .map_err(|_| ())?;
    o.begin_live_sync(preserved, live).map_err(|_| ())?;
    o.complete_live_sync().map_err(|_| ())?;
    o.enter_switch_mode(delta).map_err(|_| ())?;
    o.request_replacement().map_err(|_| ())?;
    Ok(())
}
pub fn complete(h: &KernelTransplantHandoff) {
    assert_eq!(h.switch_authorized, 1);
    let mut o = model(h);
    o.state = TransplantState::ReplacementPending;
    o.kernel_replaced().expect("orchestrator replacement");
    o.restore_complete().expect("orchestrator restore");
}
