//! Pause keeps the suspended future/stack alive; termination drops it safely.
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    task::{Context, Poll, Waker},
};
use wasmtime::{Result, format_err};
#[derive(Default)]
pub struct Control {
    state: AtomicU8,
    waker: Mutex<Option<Waker>>,
}
impl Control {
    fn set(&self, state: u8) {
        let _ = self
            .state
            .try_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                if old == 2 { None } else { Some(state) }
            });
        if let Some(w) = self.waker.lock().unwrap().take() {
            w.wake();
        }
    }
    pub fn pause(&self) {
        self.set(1)
    }
    pub fn resume(&self) {
        self.set(0)
    }
    pub fn terminate(&self) {
        self.set(2)
    }
    pub fn status(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }
}
pub struct Controlled<F> {
    control: Arc<Control>,
    future: Pin<Box<F>>,
}
impl<F> Controlled<F> {
    pub fn new(control: Arc<Control>, future: F) -> Self {
        Self {
            control,
            future: Box::pin(future),
        }
    }
}
impl<F, T> Future for Controlled<F>
where
    F: Future<Output = Result<T>>,
{
    type Output = Result<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        *self.control.waker.lock().unwrap() = Some(cx.waker().clone());
        match self.control.status() {
            2 => Poll::Ready(Err(format_err!("sandbox terminated"))),
            1 => Poll::Pending,
            _ => self.future.as_mut().poll(cx),
        }
    }
}
