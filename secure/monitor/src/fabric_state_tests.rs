use super::*;
use crate::{run_state::RunState, state_wire::InvalidState};
use sha2::{Digest, Sha256};

const ID: CheckpointIdentity = CheckpointIdentity {
    domain: 2,
    clock_epoch: 0x1234,
};
const SIZE: usize = Fabric::<4>::STATE_BYTES;

#[test]
fn pending_delivery_and_halt_survive_without_replaying_startup_or_eoi() {
    let mut source = Fabric::<4>::new().unwrap();
    source.write_lapic(0, 0xf0, 0x1ff, 0);
    source.write_lapic(0, 0x300, (1 << 18) | 64, 0);
    let delivered = source.before_entry(0, 0).unwrap();
    source.delivered(0, delivered);
    source.write_lapic(0, 0x300, (1 << 18) | 80, 0);
    let mut armed = [None; 4];
    armed[0] = source.before_entry(0, 0);
    source.write_lapic(0, 0x310, 2 << 24, 0);
    source.write_lapic(0, 0x300, 0x608, 0);
    let mut run = [RunState::default(); 4];
    run[0].halt();
    let mut bytes = [0; SIZE];
    source.snapshot(ID, 100, &run, &armed, &mut bytes).unwrap();
    let mut target = Fabric::<4>::new().unwrap();
    let mut restored = [RunState::default(); 4];
    let mut pending = [None; 4];
    target
        .restore_protected(ID, 10000, &mut restored, &mut pending, &bytes)
        .unwrap();
    assert_eq!(target.state(2), Some(CpuState::Start(8)));
    assert_eq!(target.state(1), Some(CpuState::WaitingForStartup));
    assert!(target.started(2, 8));
    assert!(!target.started(2, 8));
    assert!(!restored[0].ready(true, false));
    assert!(restored[0].ready(true, true));
    assert_eq!(pending[0].unwrap().vector, 80);
    target.delivered(0, pending[0].take().unwrap());
    assert!(target.before_entry(0, 10000).is_none());
    assert_eq!(
        target.read_lapic(0, 0x120, 10000),
        Some((1 << 0) | (1 << 16))
    );
    target.write_lapic(0, 0xb0, 0, 10000);
    assert_eq!(target.read_lapic(0, 0x120, 10000), Some(1));
}

#[test]
fn timers_keep_original_deadlines_and_partial_pit_io() {
    let mut source = Fabric::<4>::new().unwrap();
    source.write_lapic(0, 0xf0, 0x1ff, 0);
    source.ioapic.write(0, 0x30);
    source.ioapic.write(0x10, 80);
    source.hpet.write(0x10, 1, 0);
    source.hpet.write(0x100, (16 << 9) | 4, 0);
    source.hpet.write(0x108, 100, 0);
    source.write_lapic(0, 0x320, 96, 0);
    source.write_lapic(0, 0x380, 100, 0); // 2 us, absolute private clock.
    source.legacy.write(0x43, 0x30, 0);
    source.legacy.write(0x40, 0xa9, 0); // Low half only.
    let mut run = [RunState::default(); 4];
    let mut armed = [None; 4];
    let mut bytes = [0; SIZE];
    source.snapshot(ID, 100, &run, &armed, &mut bytes).unwrap();
    let mut target = Fabric::<4>::new().unwrap();
    target
        .restore_protected(ID, 1500, &mut run, &mut armed, &bytes)
        .unwrap();
    assert_eq!(target.hpet.read(0xf0, 1500), Some(150));
    let delivery = target.before_entry(0, 1500).unwrap();
    assert_eq!(delivery.vector, 80); // Already due, not restarted at restore.
    target.delivered(0, delivery);
    target.write_lapic(0, 0xb0, 0, 1500);
    assert!(target.before_entry(0, 1999).is_none());
    assert_eq!(target.before_entry(0, 2000).unwrap().vector, 96);
    assert!(target.legacy.write(0x40, 4, 1500));
    assert!(target.legacy.write(0x43, 0, 1500));
    assert_eq!(target.legacy.read(0x40, 1500), Some(0xa9));
    assert_eq!(target.legacy.read(0x40, 9999), Some(4));
}

#[test]
fn checkpoint_rejects_wrong_domain_clock_damage_and_noncanonical_state_atomically() {
    let source = Fabric::<4>::new().unwrap();
    let mut run = [RunState::default(); 4];
    let mut armed = [None; 4];
    let mut original = [0; SIZE];
    source
        .snapshot(ID, 100, &run, &armed, &mut original)
        .unwrap();
    let mut target = Fabric::<4>::new().unwrap();
    for size in 0..SIZE {
        assert_eq!(
            target.restore_protected(ID, 100, &mut run, &mut armed, &original[..size]),
            Err(InvalidState)
        );
    }
    for (identity, now) in [
        (CheckpointIdentity { domain: 1, ..ID }, 100),
        (
            CheckpointIdentity {
                clock_epoch: 0x1235,
                ..ID
            },
            100,
        ),
        (ID, 99),
    ] {
        assert_eq!(
            target.restore_protected(identity, now, &mut run, &mut armed, &original),
            Err(InvalidState)
        );
    }
    for offset in [
        0,
        40,
        44,
        64,
        64 + 14 + 28,
        64 + 131,
        64 + 133,
        64 + 134,
        64 + 135,
    ] {
        let mut damaged = original;
        damaged[offset] = 0xff;
        assert_eq!(
            target.restore_protected(ID, 100, &mut run, &mut armed, &damaged),
            Err(InvalidState)
        );
        // Protected provenance is a separate requirement. Even with a valid
        // checksum, malformed controller state must never enter the runtime.
        let end = SIZE - 32;
        let digest = Sha256::digest(&damaged[..end]);
        damaged[end..].copy_from_slice(&digest);
        assert_eq!(
            target.restore_protected(ID, 100, &mut run, &mut armed, &damaged),
            Err(InvalidState)
        );
        let mut after = [0; SIZE];
        target.snapshot(ID, 100, &run, &armed, &mut after).unwrap();
        assert_eq!(after, original);
    }
}
