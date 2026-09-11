use bexos_secure_firmware::{
    Component,
    selection::{Identity, Operation, Phase, Slot, State},
};
const RECORDS: &[u8] = include_bytes!(env!("SELECTION_RECORDS"));
#[test]
fn rust_client_matches_actual_c_authority_transitions() {
    assert_eq!(RECORDS.len(), 4 * 512);
    let states: Vec<_> = RECORDS
        .chunks_exact(512)
        .map(|r| State::decode(r).unwrap())
        .collect();
    let image = Identity {
        slot: Slot::B,
        generation: 2,
        digest: [0x5a; 32],
        length: 256,
    };
    for (index, op) in [Operation::Stage, Operation::Attempt, Operation::Commit]
        .into_iter()
        .enumerate()
    {
        let r = states[index]
            .request(op, Component::Hypervisor, image)
            .unwrap();
        assert!(states[index + 1].acknowledges(&r));
        assert!(!states[index].acknowledges(&r));
        let mut stale = r;
        stale[16] += 1;
        assert!(!states[index + 1].acknowledges(&stale));
    }
    assert_eq!(states[0].committed, [Identity::INITIAL; 2]);
    assert_eq!(states[1].phase, Phase::Pending);
    assert_eq!(states[2].phase, Phase::Trial);
    assert_eq!(states[2].attempts, 1);
    assert_eq!(states[3].phase, Phase::Idle);
    assert_eq!(states[3].committed(Component::Hypervisor), image);
    assert_eq!(states[3].committed(Component::Trusty), Identity::INITIAL);
    for record in RECORDS.chunks_exact(512) {
        for length in 0..512 {
            assert!(State::decode(&record[..length]).is_err());
        }
        for at in 204..256 {
            let mut corrupt = record.to_vec();
            corrupt[at] = 1;
            assert!(State::decode(&corrupt).is_err());
        }
    }
}

#[test]
fn final_record_must_match_the_mutation_it_acknowledges() {
    for (record_index, offsets) in [
        (0, &[96usize][..]),
        (1, &[160usize, 296][..]),
        (2, &[160usize, 280][..]),
        (3, &[104usize, 280, 304][..]),
    ] {
        for offset in offsets {
            let mut record = RECORDS[record_index * 512..(record_index + 1) * 512].to_vec();
            record[*offset] ^= 1;
            assert!(
                State::decode(&record).is_err(),
                "record {record_index}, offset {offset}"
            );
        }
    }
}
