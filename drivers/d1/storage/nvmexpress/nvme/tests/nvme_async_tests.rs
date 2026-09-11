#[test]
fn nvme_guest_entry_is_tokio_future() {
    let _future = bexos_d1_nvme::guest::main(0);
}

#[test]
fn nvme_hardware_entrypoints_are_async() {
    let _connect = bexos_d1_nvme::hardware::Hardware::connect(0, 1);
}
