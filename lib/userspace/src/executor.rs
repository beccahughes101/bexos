use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

pub fn block_on<F: Future>(mut future: F) -> F::Output {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => crate::yield_now(),
        }
    }
}

fn noop_waker() -> Waker {
    unsafe { Waker::from_raw(raw_waker()) }
}

fn raw_waker() -> RawWaker {
    RawWaker::new(core::ptr::null(), &VTABLE)
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop);

unsafe fn clone(_: *const ()) -> RawWaker {
    raw_waker()
}

unsafe fn wake(_: *const ()) {}

unsafe fn drop(_: *const ()) {}
