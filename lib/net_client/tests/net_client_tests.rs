extern "C" fn abi_version() -> u32 {
    1
}

unsafe extern "C" fn encode_get(
    _authority_ptr: *const u8,
    authority_len: usize,
    _path_ptr: *const u8,
    path_len: usize,
    _out_ptr: *mut u8,
    out_len: usize,
    written: *mut usize,
) -> i32 {
    unsafe {
        *written = authority_len + path_len;
    }
    if out_len == 0 { -3 } else { 0 }
}

#[test]
fn link_map_binds_required_net_symbols() {
    let bytes = bexos_net_client::test_link_map(&[
        (
            &b"bexos_net_abi_version"[..],
            abi_version as *const () as usize as u64,
        ),
        (
            &b"bexos_net_http1_encode_get"[..],
            encode_get as *const () as usize as u64,
        ),
    ]);
    bexos_net_client::init_from_link_map(&bytes).unwrap();
}
