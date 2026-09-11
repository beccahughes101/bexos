//! Diagnostic protected-journal transitions across three actual Trusty boots.
//! The identities below are fixtures, not installed or executable candidates.
use bexos_secure_firmware::{
    Component,
    selection::{Identity, Operation, Phase, Slot},
};
use bexos_trusty_boot::{ql::Transport, selection};

pub fn verify(transport: &mut impl Transport) {
    let state = selection::query(transport).expect("authenticated boot selection query");
    let image = Identity {
        slot: Slot::B,
        generation: 2,
        digest: [0x5a; 32],
        length: 256,
    };
    if state.revision == 1 {
        assert_eq!(state.committed, [Identity::INITIAL; 2]);
        let stage = state
            .request(Operation::Stage, Component::Hypervisor, image)
            .unwrap();
        let staged = selection::mutate(transport, &stage).unwrap();
        assert_eq!(staged, selection::mutate(transport, &stage).unwrap());
        let attempt = staged
            .request(Operation::Attempt, Component::Hypervisor, image)
            .unwrap();
        let trial = selection::mutate(transport, &attempt).unwrap();
        assert_eq!(trial, selection::mutate(transport, &attempt).unwrap());
        assert_eq!(trial.attempts, 1);
        crate::log(
            "monitor-runtime: protected boot trial persisted without advancing commitment\n",
        );
    } else if state.phase == Phase::Trial {
        assert_eq!(state.attempts, 1);
        assert_eq!(state.pending, Some((Component::Hypervisor, image)));
        assert_eq!(state.committed(Component::Hypervisor), Identity::INITIAL);
        let commit = state
            .request(Operation::Commit, Component::Hypervisor, image)
            .unwrap();
        let committed = selection::mutate(transport, &commit).unwrap();
        assert_eq!(committed, selection::mutate(transport, &commit).unwrap());
        assert_eq!(committed.committed(Component::Hypervisor), image);
        // The v1 compatibility record must change in the same transaction.
        let legacy = bexos_trusty_boot::journal::query(transport, 2, 2).unwrap();
        assert_eq!(legacy.generation, image.generation);
        assert_eq!(legacy.image_hash, image.digest);
        assert_eq!(legacy.previous, 1);
        let stale_abort = state
            .request(Operation::Abort, Component::Hypervisor, image)
            .unwrap();
        assert!(selection::mutate(transport, &stale_abort).is_err());
        assert_eq!(selection::query(transport).unwrap(), committed);
        crate::log(
            "monitor-runtime: protected boot commitment survives retries and rejects stale rollback\n",
        );
    } else {
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.committed(Component::Hypervisor), image);
        assert_eq!(state.committed(Component::Trusty), Identity::INITIAL);
        crate::log("monitor-runtime: protected committed selection recovered after reboot\n");
    }
}
