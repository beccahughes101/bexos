use bexos_migration::{
    Error, Phase, Session, Timeouts,
    records::{Receiver, Record, Store},
};
use std::sync::Arc;

fn value(bytes: &[u8]) -> Option<Arc<[u8]>> {
    Some(Arc::from(bytes))
}

#[test]
fn ordered_dirty_pass_does_not_starve_records_behind_a_hot_key() {
    let mut dirty = bexos_migration::dirty::DirtySet::new(32);
    for key in 0..20 {
        dirty.mark(key);
    }
    let pass = dirty.snapshot_keys().unwrap();
    for key in &pass {
        dirty.copied(*key);
        dirty.mark(0);
    }
    assert_eq!(pass, (0..20).collect::<Vec<_>>());
    assert_eq!(dirty.snapshot_keys().unwrap(), vec![0]);
    dirty.copied(0);
    assert!(dirty.snapshot_keys().unwrap().is_empty());
}

#[test]
fn live_copy_converges_with_mutations_before_and_after_the_cursor() {
    let mut source = Store::new(4096);
    source.set(1, value(b"first")).unwrap();
    source.set(3, value(b"third")).unwrap();
    source.set(4, value(b"fourth")).unwrap();
    let mut cursor = source.begin().unwrap();
    let mut target = Receiver::new(cursor.sequence, 4096);
    target
        .bulk(source.bulk_next(&mut cursor).unwrap().unwrap())
        .unwrap();
    source.set(1, value(b"changed after copying")).unwrap();
    source.set(2, value(b"new record")).unwrap();
    source.set(3, value(b"changed before copying")).unwrap();
    source.set(4, None).unwrap();
    source.set(5, value(b"above bulk upper bound")).unwrap();
    while let Some(record) = source.bulk_next(&mut cursor).unwrap() {
        target.bulk(record).unwrap();
    }
    target.finish_bulk().unwrap();
    source.set(2, None).unwrap();
    source.set(2, value(b"recreated")).unwrap();
    while let Some(record) = source.delta_next().unwrap() {
        target.delta(record).unwrap();
    }
    assert_eq!(&target.finish(source.sequence()).unwrap(), source.records());
}

#[test]
fn bounded_journal_failure_preserves_live_mutation_and_allows_retry() {
    let mut source = Store::new(65);
    source.set(1, value(b"a")).unwrap();
    let mut cursor = source.begin().unwrap();
    source.set(1, value(b"b")).unwrap();
    assert_eq!(source.set(1, value(b"c")), Err(Error::Capacity));
    assert_eq!(source.records()[&1].data.as_deref(), Some(b"c".as_slice()));
    assert_eq!(source.bulk_next(&mut cursor), Err(Error::Capacity));
    assert_eq!(source.delta_next(), Err(Error::Capacity));
    source.end();
    let mut retry = source.begin().unwrap();
    assert_eq!(
        source
            .bulk_next(&mut retry)
            .unwrap()
            .unwrap()
            .data
            .as_deref(),
        Some(b"c".as_slice())
    );
}

#[test]
fn record_envelope_rejects_corruption_wrong_generation_and_trailing_data() {
    let record = Record {
        key: 12,
        sequence: 7,
        data: value(b"state"),
    };
    let bytes = record.encode(42);
    assert_eq!(Record::decode(&bytes, 42, 1024).unwrap(), record);
    assert_eq!(Record::decode(&bytes, 43, 1024), Err(Error::Sequence));
    for len in 0..bytes.len() {
        assert!(Record::decode(&bytes[..len], 42, 1024).is_err());
    }
    for offset in 0..bytes.len() {
        let mut corrupt = bytes.clone();
        corrupt[offset] ^= 1;
        assert!(Record::decode(&corrupt, 42, 1024).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Record::decode(&trailing, 42, 1024).is_err());
    assert_eq!(Record::decode(&bytes, 42, 1), Err(Error::Capacity));
}

#[test]
fn receiver_rejects_gaps_reordered_bulk_and_incomplete_final_state() {
    let mut target = Receiver::new(4, 1024);
    let record = Record {
        key: 1,
        sequence: 4,
        data: value(b"a"),
    };
    target.bulk(record.clone()).unwrap();
    assert_eq!(target.bulk(record.clone()), Err(Error::Sequence));
    assert_eq!(target.delta(record.clone()), Err(Error::Sequence));
    target.finish_bulk().unwrap();
    assert_eq!(
        target.delta(Record {
            sequence: 6,
            ..record.clone()
        }),
        Err(Error::Sequence)
    );
    target
        .delta(Record {
            sequence: 5,
            ..record
        })
        .unwrap();
    assert_eq!(target.finish(6), Err(Error::Sequence));
}

#[test]
fn tracking_receiver_accounts_for_payload_without_retaining_it() {
    let mut target = Receiver::new_tracking(1, 69);
    target
        .bulk(Record {
            key: 1,
            sequence: 1,
            data: value(b"state"),
        })
        .unwrap();
    assert_eq!(
        target.bulk(Record {
            key: 2,
            sequence: 1,
            data: value(b"x"),
        }),
        Err(Error::Capacity)
    );
    target.finish_bulk().unwrap();
    let records = target.finish(1).unwrap();
    assert_eq!(records.keys().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(records[&1].data.as_deref(), Some([].as_slice()));
}

#[test]
fn phase_order_and_exact_final_sequence_guard_commit() {
    let mut s = Session::new(1, 0, Timeouts::default()).unwrap();
    assert_eq!(s.commit(0), Err(Error::BadState));
    s.begin_bulk(1).unwrap();
    s.bulk_complete(2).unwrap();
    s.quiesce(19, 3).unwrap();
    assert_eq!(s.ready(18, 4), Err(Error::Sequence));
    s.ready(19, 5).unwrap();
    s.commit(6).unwrap();
    assert_eq!(s.abort(), Err(Error::BadState));
    s.reclaimed(40_000).unwrap();
    assert_eq!(s.phase(), Phase::Reclaimed);
}

#[test]
fn timeout_aborts_at_deadline_but_does_not_roll_back_a_commit() {
    let mut s = Session::new(2, 100, Timeouts::default()).unwrap();
    s.begin_bulk(101).unwrap();
    assert_eq!(s.poll(30_100), Err(Error::TimedOut));
    assert_eq!(s.phase(), Phase::Aborted);
    let mut s = Session::new(2, 0, Timeouts::default()).unwrap();
    s.begin_bulk(1).unwrap();
    s.bulk_complete(2).unwrap();
    s.quiesce(0, 3).unwrap();
    assert_eq!(s.ready(0, 153), Err(Error::TimedOut));
    assert_eq!(s.commit(154), Err(Error::BadState));
}

#[test]
fn exhaustive_interleavings_match_source() {
    for mask in 0..256 {
        let mut source = Store::new(16_384);
        for key in 0..8 {
            source.set(key, value(&[key as u8])).unwrap();
        }
        let mut cursor = source.begin().unwrap();
        let mut target = Receiver::new(cursor.sequence, 16_384);
        for step in 0..8 {
            if mask & (1 << step) != 0 {
                source.set(step, None).unwrap();
            } else {
                source.set(step, value(&[99])).unwrap();
            }
            if let Some(record) = source.bulk_next(&mut cursor).unwrap() {
                target.bulk(record).unwrap();
            }
        }
        while let Some(record) = source.bulk_next(&mut cursor).unwrap() {
            target.bulk(record).unwrap();
        }
        target.finish_bulk().unwrap();
        while let Some(record) = source.delta_next().unwrap() {
            target.delta(record).unwrap();
        }
        assert_eq!(&target.finish(source.sequence()).unwrap(), source.records());
    }
}
