use super::*;
use crate::shared::RamWindow;
use bexos_secure_monitor_abi::ACTIVATE_ON_REBOOT;
use bexos_tee_slots::{CUTOVER_DEADLINE_NS, PREPARATION_DEADLINE_NS};

const OWNER: Caller = Caller {
    domain: DomainId(1),
    may_share: true,
};
const OTHER: Caller = Caller {
    domain: DomainId(2),
    may_share: true,
};

// Explicit test double, not a product verifier or evidence of image execution.
struct TestVerifier;
impl ImageVerifier for TestVerifier {
    fn snapshot_and_verify(
        &mut self,
        image: BufferLease<'_>,
        generation: u64,
    ) -> Result<[u8; 32], Status> {
        assert_eq!(image.host_range(), (0x100000, 4096));
        assert_eq!(generation, 8);
        Ok([8; 32])
    }
}
fn runtime(active: SlotId) -> SecureRuntime<1, 2> {
    SecureRuntime::new(
        Registry::new([RamWindow {
            owner: OWNER.domain,
            guest_start: 0x4000,
            host_start: 0x100000,
            length: 4096,
        }])
        .unwrap(),
        OWNER.domain,
        active,
        7,
        [7; 32],
    )
}
fn call(r: &mut SecureRuntime<1, 2>, caller: Caller, request: Request, now: u64) -> [u64; 8] {
    r.dispatch(caller, request.encode(), now, &mut TestVerifier)
}
fn register(r: &mut SecureRuntime<1, 2>) -> u64 {
    let result = call(
        r,
        OWNER,
        Request::Register {
            address: 0x4000,
            length: 4096,
            access: SHARED_READ,
        },
        0,
    );
    assert_eq!(result[0], 0);
    result[1]
}
fn stage(handle: u64) -> Request {
    Request::StageTrustyCore {
        handle,
        length: 4096,
        generation: 8,
    }
}
fn activate() -> Request {
    Request::ActivateTrustyCore {
        generation: 8,
        activation: ACTIVATE_LIVE_NOW,
    }
}
fn prepared() -> SecureRuntime<1, 2> {
    let mut r = runtime(SlotId::A);
    let handle = register(&mut r);
    assert_eq!(
        call(&mut r, OWNER, stage(handle), 0),
        Status::Ok.registers(1)
    );
    assert_eq!(
        call(&mut r, OWNER, activate(), 10),
        Status::Busy.registers(0)
    );
    r.candidate_ready(20).unwrap();
    r
}
fn health_confirmed() -> SecureRuntime<1, 2> {
    let mut r = prepared();
    r.begin_cutover(30).unwrap();
    r.candidate_switched(40).unwrap();
    r.confirm_service_readiness(50).unwrap();
    r
}

#[test]
fn requests_cannot_invent_readiness_or_durable_commit() {
    let mut r = prepared();
    assert_eq!(r.state().active_slot, SlotId::A);
    assert_eq!(
        call(&mut r, OWNER, activate(), 25),
        Status::Busy.registers(0)
    );
    assert_eq!(
        r.commit(|_| panic!("commit before readiness")),
        Err(Status::InvalidArgs)
    );
    r.begin_cutover(30).unwrap();
    r.candidate_switched(40).unwrap();
    assert_eq!(r.state().generation, 7);
    assert_eq!(r.state().rollback_slot, Some(SlotId::A));
    assert_eq!(
        call(&mut r, OWNER, activate(), 45),
        Status::Busy.registers(0)
    );
    r.confirm_service_readiness(50).unwrap();
    r.commit(|record| {
        assert_eq!(
            record,
            TeeCommit {
                slot: SlotId::B,
                generation: 8,
                image_hash: [8; 32]
            }
        );
        CommitOutcome::Committed
    })
    .unwrap();
    assert_eq!(r.state().generation, 8);
    assert_eq!(r.state().phase, TeeUpdatePhase::Completed);
    assert_eq!(call(&mut r, OWNER, activate(), 60), Status::Ok.registers(8));
}

#[test]
fn missing_verifier_and_unauthorized_callers_fail_without_state_changes() {
    let mut r = runtime(SlotId::B);
    assert_eq!(r.state().slot_hashes, [[0; 32], [7; 32]]);
    let handle = register(&mut r);
    assert_eq!(
        r.dispatch(OWNER, stage(handle).encode(), 0, &mut UnavailableVerifier),
        Status::Unsupported.registers(0)
    );
    assert_eq!(r.state().phase, TeeUpdatePhase::Idle);
    for caller in [
        OTHER,
        Caller {
            may_share: false,
            ..OWNER
        },
    ] {
        assert_eq!(
            call(&mut r, caller, stage(handle), 0),
            Status::AccessDenied.registers(0)
        );
        assert_eq!(
            call(&mut r, caller, activate(), 0),
            Status::AccessDenied.registers(0)
        );
    }
    assert_eq!(r.state().phase, TeeUpdatePhase::Idle);
    assert_eq!(
        call(&mut r, OWNER, stage(handle), 0),
        Status::Ok.registers(0)
    );
    assert_eq!(
        call(&mut r, OTHER, activate(), 1),
        Status::AccessDenied.registers(0)
    );
    assert_eq!(r.state().phase, TeeUpdatePhase::Staged);
    assert_eq!(
        call(
            &mut r,
            OWNER,
            Request::ActivateTrustyCore {
                generation: 8,
                activation: ACTIVATE_ON_REBOOT
            },
            1
        ),
        Status::Unsupported.registers(0)
    );
    assert_eq!(r.state().phase, TeeUpdatePhase::Staged);
}

#[test]
fn unknown_commit_requires_authenticated_recovery() {
    let mut r = health_confirmed();
    assert_eq!(r.commit(|_| CommitOutcome::Unknown), Err(Status::Busy));
    assert_eq!(r.rollback(), Err(Status::Busy));
    assert_eq!(r.state().generation, 7);
    assert_eq!(r.state().phase, TeeUpdatePhase::CommitUncertain);
    assert_eq!(
        call(&mut r, OWNER, activate(), 60),
        Status::Busy.registers(0)
    );
    assert_eq!(call(&mut r, OWNER, stage(1), 60), Status::Busy.registers(0));
    r.resolve_commit(CommitOutcome::Committed).unwrap();
    assert_eq!(call(&mut r, OWNER, activate(), 70), Status::Ok.registers(8));
}

#[test]
fn failed_commit_retains_old_generation_and_restores_owner() {
    let mut r = health_confirmed();
    assert_eq!(r.commit(|_| CommitOutcome::NotCommitted), Err(Status::Busy));
    assert_eq!(r.state().active_slot, SlotId::A);
    assert_eq!(r.state().generation, 7);
    assert_eq!(r.state().phase, TeeUpdatePhase::RolledBack);
    assert_eq!(
        call(&mut r, OWNER, activate(), 60),
        Status::InvalidArgs.registers(0)
    );
}

#[test]
fn polling_does_not_reset_preparation_and_silent_candidates_expire() {
    let mut r = prepared();
    assert_eq!(
        call(&mut r, OWNER, activate(), PREPARATION_DEADLINE_NS + 10),
        Status::Busy.registers(0)
    );
    assert_eq!(
        r.poll_deadline(PREPARATION_DEADLINE_NS + 11),
        Err(Status::Busy)
    );
    assert_eq!(r.state().phase, TeeUpdatePhase::RolledBack);
    let mut r = prepared();
    r.begin_cutover(30).unwrap();
    r.candidate_switched(40).unwrap();
    assert_eq!(
        r.confirm_service_readiness(31 + CUTOVER_DEADLINE_NS),
        Err(Status::Busy)
    );
    assert_eq!(r.state().active_slot, SlotId::A);
    assert_eq!(r.state().generation, 7);
    assert_eq!(
        r.commit(|_| panic!("expired candidate")),
        Err(Status::InvalidArgs)
    );
}

#[test]
fn invalid_handle_and_generation_never_reach_verifier() {
    let mut r = runtime(SlotId::A);
    assert_eq!(
        call(&mut r, OWNER, stage(99), 0),
        Status::InvalidHandle.registers(0)
    );
    let handle = register(&mut r);
    assert_eq!(
        call(
            &mut r,
            OWNER,
            Request::StageTrustyCore {
                handle,
                length: 4096,
                generation: 7
            },
            0
        ),
        Status::AccessDenied.registers(0)
    );
    call(&mut r, OWNER, Request::Unregister { handle }, 0);
    assert_eq!(
        call(&mut r, OWNER, stage(handle), 0),
        Status::InvalidHandle.registers(0)
    );
}
