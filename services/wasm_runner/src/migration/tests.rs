use super::*;
use bexos_wasm_runtime::{instance::CoreInstance, resources::Origin};

#[test]
fn dispatch_keeps_bulk_snapshot_but_defers_cutover_until_refreshed() {
    let options = bexos_wasm_abi::WasmRunnerOptions {
        path: "/pkg/service.wasm".into(),
        ..Default::default()
    };
    let engine = bexos_wasm_runtime::engine::configured_engine(&options.limits).unwrap();
    let host = Arc::new(NativeHost::new());
    let context = Context::new(
        options.clone(),
        host.clone(),
        Origin::Signed,
        Budget::for_limits(&options.limits),
    );
    let bytes = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const 0) "\2a")
            (func (export "bexos-checkpoint") (result i64) i64.const 1)
            (func (export "bexos-service-dispatch") (param i32)
                i32.const 0 i32.const 7 i32.store8))"#,
    )
    .unwrap();
    let core = block_on(CoreInstance::instantiate(&engine, bytes.into(), context)).unwrap();
    let mut runtime = Runtime::source(ServiceGuest::Core(core), Channel(1), None, host);
    runtime.refresh().unwrap();
    assert!(runtime.quiescence_ready());
    assert_eq!(runtime.snapshot.as_ref().unwrap().checkpoint, [42]);
    let bulk = runtime.encode_record(1).unwrap();

    runtime.invalidate();
    let mut guest = runtime.instance.take().unwrap();
    assert!(!runtime.quiescence_ready());
    assert_eq!(runtime.encode_record(1).unwrap(), bulk);
    crate::dispatch::run(&mut guest, 0, || {}).unwrap();
    runtime.instance = Some(guest);
    assert!(!runtime.quiescence_ready());

    runtime.refresh().unwrap();
    assert!(runtime.quiescence_ready());
    assert_eq!(runtime.validate(), Ok(()));
    assert_eq!(runtime.snapshot.as_ref().unwrap().checkpoint, [7]);
    assert_ne!(runtime.encode_record(1).unwrap(), bulk);
    let latest = runtime.encode_record(1).unwrap();
    // Exercise a failed refresh after snapshotting the guest. Bulk transfer
    // must retain its last record, while cutover waits for a successful retry.
    runtime
        .instance
        .as_mut()
        .unwrap()
        .store_mut()
        .data_mut()
        .options
        .path
        .clear();
    assert!(runtime.refresh().is_err());
    assert!(!runtime.quiescence_ready());
    assert_eq!(runtime.encode_record(1).unwrap(), latest);
    runtime
        .instance
        .as_mut()
        .unwrap()
        .store_mut()
        .data_mut()
        .options
        .path = "/pkg/service.wasm".into();
    runtime.refresh().unwrap();
    assert!(runtime.quiescence_ready());
    runtime.discard_checkpoint();
    assert!(!runtime.quiescence_ready());
    assert!(runtime.keys().is_empty());
}
