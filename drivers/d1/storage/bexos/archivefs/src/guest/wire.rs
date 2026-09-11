pub(crate) fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}
