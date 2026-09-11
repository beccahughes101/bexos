#[test]
fn archivefs_guest_entry_is_tokio_future() {
    let _future = bexos_archivefs::guest::main(0);
}
