use super::*;
use crate::{
    bindings::bexos::wasm::kernel::StreamError,
    resources::{Kind, READ, TRANSFER, WRITE},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct SocketHandle(u64);
impl Handle for SocketHandle {
    fn kind(&self) -> Kind {
        Kind::Socket
    }
    fn rights(&self) -> u32 {
        READ | WRITE | TRANSFER
    }
    fn native(&self) -> u64 {
        self.0
    }
}
#[derive(Default)]
struct Streams {
    pairs: AtomicUsize,
    shutdowns: AtomicUsize,
    bytes: Mutex<Vec<u8>>,
}
impl Host for Streams {
    fn monotonic_ns(&self) -> u64 {
        0
    }
    fn log(&self, _: &[u8]) {
        panic!("ambient output");
    }
    fn channel_write(&self, _: &dyn Handle, _: &[u8], _: &[Entry]) -> wasmtime::Result<()> {
        unreachable!()
    }
    fn channel_read(
        &self,
        _: &dyn Handle,
        _: usize,
        _: usize,
    ) -> wasmtime::Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
        unreachable!()
    }
    fn socket_pair(&self) -> wasmtime::Result<(Arc<dyn Handle>, Arc<dyn Handle>)> {
        self.pairs.fetch_add(1, Ordering::Relaxed);
        Ok((Arc::new(SocketHandle(100)), Arc::new(SocketHandle(101))))
    }
    fn socket_ready(&self, _: &dyn Handle, write: bool) -> wasmtime::Result<bool> {
        Ok(write
            || !self.bytes.lock().unwrap().is_empty()
            || self.shutdowns.load(Ordering::Relaxed) > 0)
    }
    fn socket_read(&self, _: &dyn Handle, max: usize) -> wasmtime::Result<Vec<u8>> {
        let mut bytes = self.bytes.lock().unwrap();
        let n = max.min(bytes.len());
        Ok(bytes.drain(..n).collect())
    }
    fn socket_write(&self, _: &dyn Handle, bytes: &[u8]) -> wasmtime::Result<usize> {
        let n = bytes.len().min(3);
        self.bytes.lock().unwrap().extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn socket_half_close(&self, _: &dyn Handle, _: bool, _: bool) -> wasmtime::Result<()> {
        self.shutdowns.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
fn setup() -> (Context, Arc<Streams>) {
    let mut c = context();
    let h = Arc::new(Streams::default());
    c.host = h.clone();
    (c, h)
}
#[test]
fn streams_distinguish_backpressure_from_eof_and_keep_partial_writes() {
    let (mut c, h) = setup();
    let (a, b) = c.terminal_socket_pair().unwrap();
    assert_ne!(a, 100);
    assert!(matches!(
        c.terminal_socket_read(b, 8).unwrap(),
        Err(StreamError::WouldBlock)
    ));
    assert_eq!(c.terminal_socket_write(a, b"abcdef").unwrap().unwrap(), 3);
    assert_eq!(c.terminal_socket_read(b, 8).unwrap().unwrap(), b"abc");
    c.terminal_socket_shutdown(a, false, true).unwrap();
    assert!(c.terminal_socket_read(b, 8).unwrap().unwrap().is_empty());
    assert_eq!(h.shutdowns.load(Ordering::Relaxed), 1);
}
#[test]
fn creation_reserves_both_endpoints_before_host_side_effects() {
    let (mut c, h) = setup();
    c.resources = crate::resources::Resources::new(1, Origin::Signed);
    assert!(c.terminal_socket_pair().is_err());
    c.resources = crate::resources::Resources::new(8, Origin::Unsigned);
    assert!(c.terminal_socket_pair().is_err());
    assert_eq!(h.pairs.load(Ordering::Relaxed), 0);
}
#[test]
fn restore_and_invalid_buffers_cannot_perform_stream_io() {
    let (mut c, h) = setup();
    let (a, _) = c.terminal_socket_pair().unwrap();
    assert!(c.terminal_socket_read(a, 32769).is_err());
    assert!(c.terminal_socket_write(a, &vec![0; 32769]).is_err());
    assert!(c.terminal_socket_shutdown(a, false, false).is_err());
    c.restoring = true;
    assert!(c.terminal_socket_pair().is_err());
    assert!(c.terminal_socket_read(a, 1).is_err());
    assert!(c.terminal_socket_write(a, b"x").is_err());
    assert!(c.terminal_socket_ready(a, false).is_err());
    assert!(c.terminal_socket_shutdown(a, false, true).is_err());
    assert_eq!(h.pairs.load(Ordering::Relaxed), 1);
    assert_eq!(h.shutdowns.load(Ordering::Relaxed), 0);
    assert!(h.bytes.lock().unwrap().is_empty());
}
#[test]
fn wasi_stdin_reads_only_an_explicit_grant() {
    use crate::wasi::{state::Input, wasi::cli::stdin::Host as Stdin};
    let (mut c, _) = setup();
    let closed = c.get_stdin().unwrap();
    assert!(matches!(c.wasi.table.get(&closed).unwrap(), Input::Empty));
    c.resources
        .insert(Entry {
            name: "wasi:stdin".into(),
            handle: Arc::new(SocketHandle(1)),
        })
        .unwrap();
    let input = c.get_stdin().unwrap();
    assert!(matches!(
        c.wasi.table.get(&input).unwrap(),
        Input::Socket(_)
    ));
}

#[test]
fn restore_metadata_validation_does_not_perform_io() {
    use crate::bindings::bexos::wasm::kernel::{Host as _, ResourceKind};
    let (mut c, host) = setup();
    let (id, _) = c.terminal_socket_pair().unwrap();
    c.restoring = true;
    let metadata = c.inspect_resource(id).unwrap().unwrap();
    assert_eq!(metadata.kind, ResourceKind::Socket);
    assert_eq!(metadata.rights, READ | WRITE | TRANSFER);
    assert!(c.inspect_resource(u32::MAX).unwrap().is_none());
    assert_eq!(host.pairs.load(Ordering::Relaxed), 1);
    assert_eq!(host.shutdowns.load(Ordering::Relaxed), 0);
}
