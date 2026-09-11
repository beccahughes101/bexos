use bexos_tee_driver_trusty_ql_proto::{
    Error, Header, QL_HEADER_SIZE, QL_OP_CONNECT, QL_OP_RECV, QL_RESP_BIT, encode_command,
    encode_header, keymint_response_chunk, max_payload_len, response_payload,
};

#[test]
fn command_encoder_writes_header_and_payload() {
    let mut buffer = [0u8; 64];
    encode_command(&mut buffer, QL_OP_CONNECT, 7, b"port\0").unwrap();

    assert_eq!(&buffer[0..2], &QL_OP_CONNECT.to_le_bytes());
    assert_eq!(&buffer[2..4], &0u16.to_le_bytes());
    assert_eq!(&buffer[4..8], &0u32.to_le_bytes());
    assert_eq!(&buffer[8..12], &7u32.to_le_bytes());
    assert_eq!(&buffer[12..16], &5u32.to_le_bytes());
    assert_eq!(&buffer[QL_HEADER_SIZE..QL_HEADER_SIZE + 5], b"port\0");
}

#[test]
fn command_encoder_rejects_oversized_payload() {
    let mut buffer = [0u8; 24];
    let payload = [0u8; 9];

    assert_eq!(
        encode_command(&mut buffer, QL_OP_RECV, 1, &payload),
        Err(Error::PayloadTooLarge)
    );
    assert_eq!(max_payload_len(buffer.len()), 8);
}

#[test]
fn shared_buffer_carries_maximum_rpmb_request_and_response() {
    use bexos_tee_driver_trusty_ql_proto::{QL_BUFFER_SIZE, QL_OP_SEND};
    let mut buffer = vec![0; QL_BUFFER_SIZE];
    let request = vec![0xa5; 24 + 16 + 8 * 512];
    encode_command(&mut buffer, QL_OP_SEND, 7, &request).unwrap();
    let response = vec![0x5a; 24 + 8 * 512];
    encode_header(
        &mut buffer,
        Header {
            opcode: QL_OP_RECV | QL_RESP_BIT,
            flags: 0,
            status: 0,
            handle: 7,
            payload_len: response.len() as u32,
        },
    )
    .unwrap();
    buffer[QL_HEADER_SIZE..QL_HEADER_SIZE + response.len()].copy_from_slice(&response);
    assert_eq!(response_payload(&buffer, QL_OP_RECV).unwrap().1, response);
}

#[test]
fn response_decoder_requires_response_opcode() {
    let mut buffer = [0u8; 64];
    encode_header(
        &mut buffer,
        Header {
            opcode: QL_OP_RECV,
            flags: 0,
            status: 0,
            handle: 9,
            payload_len: 0,
        },
    )
    .unwrap();

    assert_eq!(
        response_payload(&buffer, QL_OP_RECV),
        Err(Error::InvalidResponse)
    );
}

#[test]
fn response_decoder_returns_bounded_payload() {
    let mut buffer = [0u8; 64];
    encode_header(
        &mut buffer,
        Header {
            opcode: QL_OP_RECV | QL_RESP_BIT,
            flags: 0,
            status: 0,
            handle: 9,
            payload_len: 3,
        },
    )
    .unwrap();
    buffer[QL_HEADER_SIZE..QL_HEADER_SIZE + 3].copy_from_slice(b"abc");

    let (header, payload) = response_payload(&buffer, QL_OP_RECV).unwrap();
    assert_eq!(header.handle, 9);
    assert_eq!(payload, b"abc");
}

#[test]
fn response_decoder_rejects_truncated_payload() {
    let mut buffer = [0u8; QL_HEADER_SIZE + 2];
    encode_header(
        &mut buffer,
        Header {
            opcode: QL_OP_RECV | QL_RESP_BIT,
            flags: 0,
            status: 0,
            handle: 9,
            payload_len: 3,
        },
    )
    .unwrap();

    assert_eq!(
        response_payload(&buffer, QL_OP_RECV),
        Err(Error::InvalidResponse)
    );
}

#[test]
fn keymint_chunk_decoder_strips_and_validates_continuation_marker() {
    assert_eq!(
        keymint_response_chunk(b"\x01abc"),
        Ok((true, b"abc".as_slice()))
    );
    assert_eq!(
        keymint_response_chunk(b"\x00def"),
        Ok((false, b"def".as_slice()))
    );
    assert_eq!(keymint_response_chunk(b""), Err(Error::InvalidResponse));
    assert_eq!(
        keymint_response_chunk(b"\x02bad"),
        Err(Error::InvalidResponse)
    );
}
