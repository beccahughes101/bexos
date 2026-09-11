use std::collections::BTreeMap;

use authmgr_storage_format::{
    decode_context, decode_sequence, encode_context, encode_sequence, increment_sequence,
    FormatError,
};

#[test]
fn context_round_trip_and_corruption_are_bounded() {
    let encoded = encode_context(3, 17, b"policy").unwrap();
    assert_eq!(
        decode_context(&encoded).unwrap(),
        (3, 17, b"policy".to_vec())
    );

    for end in 0..encoded.len() {
        assert!(
            decode_context(&encoded[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
    let mut corrupt_magic = encoded.clone();
    corrupt_magic[0] ^= 0x80;
    assert_eq!(
        decode_context(&corrupt_magic),
        Err(FormatError::ContextHeader)
    );
    let mut corrupt_length = encoded;
    corrupt_length[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        decode_context(&corrupt_length),
        Err(FormatError::ContextLength)
    );
}

#[test]
fn duplicate_context_is_rejected_and_survives_restart() {
    let mut durable = BTreeMap::new();
    let path = "authmgr.i.0123";
    let first = encode_context(1, 9, b"first").unwrap();
    assert!(durable.insert(path, first).is_none());
    let duplicate = encode_context(2, 10, b"duplicate").unwrap();
    assert!(durable.get(path).is_some());
    assert_eq!(
        decode_context(durable.get(path).unwrap()).unwrap().2,
        b"first"
    );

    let reopened = durable.clone();
    assert_eq!(
        decode_context(reopened.get(path).unwrap()).unwrap(),
        (1, 9, b"first".to_vec())
    );
    drop(duplicate);
}

#[test]
fn sequence_persists_and_overflow_fails_closed() {
    let mut durable = BTreeMap::new();
    durable.insert("authmgr.sequence", encode_sequence(41).to_vec());
    let next = increment_sequence(durable.get("authmgr.sequence").unwrap()).unwrap();
    durable.insert("authmgr.sequence", next.to_vec());

    let reopened = durable.clone();
    assert_eq!(
        decode_sequence(reopened.get("authmgr.sequence").unwrap()).unwrap(),
        42
    );
    assert_eq!(
        increment_sequence(&encode_sequence(i32::MAX)),
        Err(FormatError::SequenceOverflow)
    );
    assert_eq!(decode_sequence(&[0; 7]), Err(FormatError::Sequence));
    assert_eq!(decode_sequence(&[0; 9]), Err(FormatError::Sequence));
}
