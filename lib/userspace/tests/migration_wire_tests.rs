use bexos_userspace::live_migration::{MAX_RECORD_DATA, wire::encode_request};
use migration_fidl::{FidlDecode, StateReceiverAdoptDeltaRequest};

#[test]
fn direct_cutover_sender_encodes_a_maximum_size_file_delta() {
    let data = vec![0xa5; MAX_RECORD_DATA];
    let record = bexos_migration::records::Record {
        key: 17,
        sequence: 9,
        data: Some(data.clone().into()),
    }
    .encode(3);
    let bytes = encode_request(4, &StateReceiverAdoptDeltaRequest { record: &record }).unwrap();
    assert_eq!(u64::from_le_bytes(bytes[..8].try_into().unwrap()), 4);
    let request = StateReceiverAdoptDeltaRequest::decode(&bytes[8..], &[]).unwrap();
    let decoded =
        bexos_migration::records::Record::decode(request.record, 3, MAX_RECORD_DATA).unwrap();
    assert_eq!(decoded.key, 17);
    assert_eq!(decoded.sequence, 9);
    assert_eq!(decoded.data.as_deref(), Some(data.as_slice()));
}

#[test]
fn direct_delta_ack_requires_matching_peer_response_and_success() {
    use bexos_migration::Error;
    use bexos_userspace::live_migration::wire::decode_delta_ack;
    use migration_fidl::{FidlEncode, StateReceiverAdoptDeltaResponse};

    let mut ack = vec![0; 24];
    ack[..8].copy_from_slice(&7u64.to_le_bytes());
    ack[8..16].copy_from_slice(&4u64.to_le_bytes());
    StateReceiverAdoptDeltaResponse { status: 0 }
        .encode(&mut ack[16..], &mut [])
        .unwrap();
    assert_eq!(decode_delta_ack(&ack, false, 7), Ok(()));
    for length in 0..ack.len() {
        assert_eq!(
            decode_delta_ack(&ack[..length], false, 7),
            Err(Error::InvalidData)
        );
    }
    assert_eq!(decode_delta_ack(&ack, true, 7), Err(Error::InvalidData));
    assert_eq!(decode_delta_ack(&ack, false, 8), Err(Error::InvalidData));
    ack[8] = 5;
    assert_eq!(decode_delta_ack(&ack, false, 7), Err(Error::InvalidData));
    ack[8] = 4;
    ack.push(0);
    assert_eq!(decode_delta_ack(&ack, false, 7), Err(Error::InvalidData));
    ack.pop();
    StateReceiverAdoptDeltaResponse { status: -8 }
        .encode(&mut ack[16..], &mut [])
        .unwrap();
    assert_eq!(decode_delta_ack(&ack, false, 7), Err(Error::BadState));
}

#[test]
fn direct_validate_ack_requires_matching_peer_response_and_success() {
    use bexos_migration::Error;
    use bexos_userspace::live_migration::wire::decode_validate_ack;
    use migration_fidl::{FidlEncode, StateReceiverValidateResponse};

    let mut ack = vec![0; 24];
    ack[..8].copy_from_slice(&11u64.to_le_bytes());
    ack[8..16].copy_from_slice(&5u64.to_le_bytes());
    StateReceiverValidateResponse { status: 0 }
        .encode(&mut ack[16..], &mut [])
        .unwrap();
    assert_eq!(decode_validate_ack(&ack, false, 11), Ok(()));
    for length in 0..ack.len() {
        assert_eq!(
            decode_validate_ack(&ack[..length], false, 11),
            Err(Error::InvalidData)
        );
    }
    assert_eq!(decode_validate_ack(&ack, true, 11), Err(Error::InvalidData));
    assert_eq!(
        decode_validate_ack(&ack, false, 12),
        Err(Error::InvalidData)
    );
    ack[8] = 4;
    assert_eq!(
        decode_validate_ack(&ack, false, 11),
        Err(Error::InvalidData)
    );
    ack[8] = 5;
    ack.push(0);
    assert_eq!(
        decode_validate_ack(&ack, false, 11),
        Err(Error::InvalidData)
    );
    ack.pop();
    StateReceiverValidateResponse { status: -8 }
        .encode(&mut ack[16..], &mut [])
        .unwrap();
    assert_eq!(decode_validate_ack(&ack, false, 11), Err(Error::BadState));
}
