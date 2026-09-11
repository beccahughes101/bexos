use std::{
    future::Future,
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};
struct Yield;
impl Wake for Yield {
    fn wake(self: Arc<Self>) {}
}
/// Poll once per scheduler turn, including Wasmtime fuel and WASI poll yields.
pub fn block_on<F: Future>(future: F) -> F::Output {
    block_on_with(future, || {})
}
pub fn block_on_with<F: Future>(future: F, mut pending: impl FnMut()) -> F::Output {
    let waker = Waker::from(Arc::new(Yield));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                pending();
                bexos_userspace::syscall::yield_now();
            }
        }
    }
}
