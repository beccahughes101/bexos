#[test]
fn guest_runtime_type_is_available() {
    let _ = core::mem::size_of::<bexos_memfs::guest::migration::Runtime>();
}
