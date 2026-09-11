use super::*;
use crate::bindings::bexos::wasm::checkpoint_resources::Host as Hooks;
use crate::resources::{Kind, READ, WRITE};
use crate::wasi::{
    checkpoint::Saved,
    state::{Input, Output},
    wasi::io::{
        poll::HostPollable,
        streams::{HostOutputStream, StreamError},
    },
};
use std::sync::atomic::{AtomicUsize, Ordering};
use wasmtime::component::Resource;
struct Socket;
impl Handle for Socket {
    fn kind(&self) -> Kind {
        Kind::Socket
    }
    fn rights(&self) -> u32 {
        READ | WRITE
    }
    fn native(&self) -> u64 {
        7
    }
}
#[derive(Default)]
struct Backpressure {
    quota: AtomicUsize,
    written: Mutex<Vec<u8>>,
    reads: AtomicUsize,
}
impl Host for Backpressure {
    fn monotonic_ns(&self) -> u64 {
        0
    }
    fn log(&self, _: &[u8]) {
        panic!("unexpected ambient log");
    }
    fn channel_write(&self, _: &dyn Handle, _: &[u8], _: &[Entry]) -> wasmtime::Result<()> {
        wasmtime::bail!("unavailable")
    }
    fn channel_read(
        &self,
        _: &dyn Handle,
        _: usize,
        _: usize,
    ) -> wasmtime::Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
        wasmtime::bail!("unavailable")
    }
    fn socket_ready(&self, _: &dyn Handle, _: bool) -> wasmtime::Result<bool> {
        Ok(true)
    }
    fn socket_write(&self, _: &dyn Handle, bytes: &[u8]) -> wasmtime::Result<usize> {
        let n = bytes.len().min(self.quota.load(Ordering::Relaxed));
        self.quota.fetch_sub(n, Ordering::Relaxed);
        self.written.lock().unwrap().extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn socket_read(&self, _: &dyn Handle, length: usize) -> wasmtime::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(vec![1; length])
    }
}
fn setup() -> (Context, Arc<Backpressure>) {
    let host = Arc::new(Backpressure::default());
    let mut context = context();
    context.host = host.clone();
    (context, host)
}
#[test]
fn partial_output_and_pollable_survive_checkpoint_without_replaying_bytes() {
    let (mut source, host) = setup();
    let output = source.push(Output::socket(Arc::new(Socket))).unwrap();
    let id = output.rep();
    assert!(
        source
            .write(Resource::new_borrow(id), b"no permit".to_vec())
            .is_err()
    );
    assert_eq!(
        source
            .check_write(Resource::new_borrow(id))
            .unwrap()
            .unwrap(),
        32768
    );
    host.quota.store(2, Ordering::Relaxed);
    source
        .write(Resource::new_borrow(id), b"abcdef".to_vec())
        .unwrap()
        .unwrap();
    source.flush(Resource::new_borrow(id)).unwrap().unwrap();
    let poll = HostOutputStream::subscribe(&mut source, Resource::new_borrow(id)).unwrap();
    let poll_id = poll.rep();
    assert!(!source.ready(Resource::new_borrow(poll_id)).unwrap());
    let captured = source.wasi.snapshot().unwrap();
    host.quota.store(2, Ordering::Relaxed);
    assert!(!source.ready(Resource::new_borrow(poll_id)).unwrap());
    let Saved::Output(earlier) = captured.get(&id).unwrap() else {
        panic!()
    };
    assert_eq!(
        earlier.snapshot().pending,
        b"cdef",
        "snapshot must not follow later live writes"
    );
    let final_state = source.wasi.snapshot().unwrap();
    let (mut candidate, _) = setup();
    candidate.host = host.clone();
    candidate.restoring = true;
    candidate.wasi.restore(final_state, 256).unwrap();
    let adopted_poll = candidate.adopt_pollable(poll_id).unwrap().unwrap();
    let new_poll = adopted_poll.rep();
    let adopted_output = candidate.adopt_output(id).unwrap().unwrap();
    let new_output = adopted_output.rep();
    assert!(candidate.ready(Resource::new_borrow(new_poll)).is_err());
    candidate.restoring = false;
    host.quota.store(2, Ordering::Relaxed);
    assert!(candidate.ready(Resource::new_borrow(new_poll)).unwrap());
    assert_eq!(
        candidate
            .check_write(Resource::new_borrow(new_output))
            .unwrap()
            .unwrap(),
        32768
    );
    candidate
        .flush(Resource::new_borrow(new_output))
        .unwrap()
        .unwrap();
    assert_eq!(&*host.written.lock().unwrap(), b"abcdef");
}
#[test]
fn splice_does_not_consume_input_without_output_credit() {
    let (mut context, host) = setup();
    let output = context.push(Output::socket(Arc::new(Socket))).unwrap();
    let id = output.rep();
    context
        .check_write(Resource::new_borrow(id))
        .unwrap()
        .unwrap();
    context
        .write(Resource::new_borrow(id), vec![0; 32768])
        .unwrap()
        .unwrap();
    let input = context.push(Input::Socket(Arc::new(Socket))).unwrap();
    assert_eq!(
        context
            .splice(Resource::new_borrow(id), input, 10)
            .unwrap()
            .unwrap(),
        0
    );
    assert_eq!(host.reads.load(Ordering::Relaxed), 0);
}
#[test]
fn blocking_output_yields_until_capacity_is_available() {
    let (mut context, host) = setup();
    let output = context.push(Output::socket(Arc::new(Socket))).unwrap();
    let mut future = pin!(context.blocking_write_and_flush(output, b"test".to_vec()));
    let waker = Waker::from(Arc::new(Noop));
    let mut cx = TaskContext::from_waker(&waker);
    assert!(future.as_mut().poll(&mut cx).is_pending());
    host.quota.store(4, Ordering::Relaxed);
    assert!(matches!(
        future.as_mut().poll(&mut cx),
        Poll::Ready(Ok(Ok(())))
    ));
    assert_eq!(&*host.written.lock().unwrap(), b"test");
}
#[test]
fn closed_output_reports_wasi_closed() {
    let (mut context, _) = setup();
    let output = context.push(Output::closed()).unwrap();
    assert!(matches!(
        context.check_write(output).unwrap(),
        Err(StreamError::Closed)
    ));
}
