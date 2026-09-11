use super::*;
use crate::resources::{Kind, READ, TRANSFER, WRITE};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct TransferHandle {
    rights: u32,
    closed: Arc<AtomicUsize>,
}
impl Handle for TransferHandle {
    fn kind(&self) -> Kind {
        Kind::Channel
    }
    fn rights(&self) -> u32 {
        self.rights
    }
    fn native(&self) -> u64 {
        99
    }
}
impl Drop for TransferHandle {
    fn drop(&mut self) {
        self.closed.fetch_add(1, Ordering::Relaxed);
    }
}
struct TransferHost {
    fail: AtomicBool,
    calls: AtomicUsize,
}
impl Host for TransferHost {
    fn monotonic_ns(&self) -> u64 {
        0
    }
    fn log(&self, _: &[u8]) {}
    fn channel_write(
        &self,
        _: &dyn Handle,
        bytes: &[u8],
        handles: &[Entry],
    ) -> wasmtime::Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(bytes, b"x");
        assert_eq!(handles.len(), 1);
        if self.fail.load(Ordering::Relaxed) {
            wasmtime::bail!("injected send failure");
        }
        Ok(())
    }
    fn channel_read(
        &self,
        _: &dyn Handle,
        _: usize,
        _: usize,
    ) -> wasmtime::Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
        panic!("invalid receive pointer reached host");
    }
}
#[test]
fn real_core_transfer_failure_preserves_handle_and_success_consumes_it_once() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let host = Arc::new(TransferHost {
        fail: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
    });
    let mut ctx = context();
    ctx.host = host.clone();
    let closed = Arc::new(AtomicUsize::new(0));
    let target = ctx
        .resources
        .insert(Entry {
            name: "target".into(),
            handle: Arc::new(TransferHandle {
                rights: READ | WRITE,
                closed: Arc::new(AtomicUsize::new(0)),
            }),
        })
        .unwrap();
    assert_eq!(
        ctx.resources
            .insert(Entry {
                name: "transfer".into(),
                handle: Arc::new(TransferHandle {
                    rights: READ | TRANSFER,
                    closed: closed.clone()
                })
            })
            .unwrap(),
        2
    );
    let bytes = wat::parse_str(r#"(module
      (import "bexos:kernel/ipc@1.0.0" "channel-write" (func $send (param i32 i32 i32 i32 i32) (result i32)))
      (import "bexos:kernel/ipc@1.0.0" "channel-read" (func $read (param i32 i32 i32 i32 i32) (result i64)))
      (memory (export "memory") 1)
      (data (i32.const 0) "\02\00\00\00") (data (i32.const 16) "x")
      (func (export "send") (param i32) (result i32) local.get 0 i32.const 16 i32.const 1 i32.const 0 i32.const 1 call $send)
      (func (export "bad-receive") (param i32) (result i32) local.get 0 i32.const -1 i32.const 8 i32.const 0 i32.const 1 call $read i32.wrap_i64)
    )"#).unwrap();
    let mut instance = run(CoreInstance::instantiate(&engine, bytes.into(), ctx)).unwrap();
    assert!(run(instance.invoke("bad-receive", target as i32)).is_err());
    assert_eq!(run(instance.invoke("send", target as i32)).unwrap(), -1);
    assert!(instance.store.data().resources.transferable(2).is_ok());
    assert_eq!(closed.load(Ordering::Relaxed), 0);
    host.fail.store(false, Ordering::Relaxed);
    assert_eq!(run(instance.invoke("send", target as i32)).unwrap(), 0);
    assert!(instance.store.data().resources.transferable(2).is_err());
    assert_eq!(closed.load(Ordering::Relaxed), 1);
    assert!(run(instance.invoke("send", target as i32)).is_err());
    assert_eq!(host.calls.load(Ordering::Relaxed), 2);
}
