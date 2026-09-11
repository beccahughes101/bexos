#[test]
fn async_recv_future_can_be_constructed() {
    let _future = bexos_userspace_async::recv_with_spin_limit(bexos_userspace::Channel(0), 0);
}
