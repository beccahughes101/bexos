use crate::{
    budget::Budget,
    context::Context,
    host::Host,
    instance::CoreInstance,
    resources::{Entry, Handle, Origin},
};
use std::{
    future::Future,
    pin::pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, Wake, Waker},
};
struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}
fn run<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = TaskContext::from_waker(&w);
    for _ in 0..100000 {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
    panic!("future exceeded bounded test polls");
}
#[derive(Default)]
struct TestHost(Mutex<Vec<u8>>);
impl Host for TestHost {
    fn monotonic_ns(&self) -> u64 {
        123
    }
    fn log(&self, b: &[u8]) {
        self.0.lock().unwrap().extend_from_slice(b);
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
}
fn context() -> Context {
    let options = bexos_wasm_abi::WasmRunnerOptions {
        path: "/pkg/test.wasm".into(),
        ..Default::default()
    };
    Context::new(
        options,
        Arc::new(TestHost::default()),
        Origin::Signed,
        Budget::new(64 << 20),
    )
}
static TEST_ENGINE: Mutex<()> = Mutex::new(());
fn instance(wat: &str) -> CoreInstance {
    let engine = crate::engine::engine().unwrap();
    run(CoreInstance::instantiate(
        &engine,
        wat::parse_str(wat).unwrap().into(),
        context(),
    ))
    .unwrap()
}
#[test]
fn invalid_pointer_traps_without_calling_host() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let mut i = instance(
        r#"(module (import "bexos:kernel/ipc@1.0.0" "resource-find" (func $find (param i32 i32) (result i32))) (memory (export "memory") 1) (func (export "invoke") (param i32) (result i32) i32.const -1 i32.const 16 call $find))"#,
    );
    assert!(
        run(i.invoke("invoke", 0))
            .unwrap_err()
            .to_string()
            .contains("error while executing")
    );
}
#[test]
fn fuel_exhaustion_yields_and_traps() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let mut i = instance(
        "(module (func (export \"invoke\") (param i32) (result i32) (loop $spin br $spin) i32.const 0))",
    );
    i.store.set_fuel(100).unwrap();
    i.store.fuel_async_yield_interval(Some(10)).unwrap();
    assert!(run(i.invoke("invoke", 0)).is_err());
    assert_eq!(i.store.get_fuel().unwrap(), 0);
}
#[test]
fn memories_are_independent() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let code = "(module (memory (export \"memory\") 1) (func (export \"invoke\") (param i32) (result i32) i32.const 0 local.get 0 i32.store i32.const 0 i32.load))";
    let mut a = instance(code);
    let mut b = instance(code);
    assert_eq!(run(a.invoke("invoke", 17)).unwrap(), 17);
    assert_eq!(run(b.invoke("invoke", 29)).unwrap(), 29);
    let memory = a.instance.get_memory(&mut a.store, "memory").unwrap();
    assert_eq!(memory.data(&a.store)[0], 17);
}
#[test]
fn serialized_artifacts_and_shared_memory_are_rejected() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let e = crate::engine::engine().unwrap();
    assert!(crate::instance::compile_module(&e, b"\x7fELF0000", 1024).is_err());
    let shared = wat::parse_str("(module (memory 1 1 shared))").unwrap();
    assert!(crate::instance::compile_module(&e, &shared, 1024).is_err());
}
#[test]
fn termination_is_irreversible() {
    let c = crate::control::Control::default();
    c.pause();
    assert_eq!(c.status(), 1);
    c.resume();
    assert_eq!(c.status(), 0);
    c.terminate();
    c.resume();
    c.pause();
    assert_eq!(c.status(), 2);
}
#[test]
fn ordinary_command_component_runs() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let bytes = wat::parse_str(
        r#"(component
  (core module $m (func (export "run") (result i32) i32.const 0))
  (core instance $i (instantiate $m))
  (func $run (result (result)) (canon lift (core func $i "run")))
  (instance $cli (export "run" (func $run)))
  (export "wasi:cli/run@0.2.12" (instance $cli)))"#,
    )
    .unwrap();
    let mut command = run(crate::component::CommandInstance::instantiate(
        &engine,
        bytes.into(),
        context(),
    ))
    .unwrap();
    assert_eq!(run(command.run()).unwrap(), 0);
}
#[test]
fn checkpoint_restores_application_state() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let code = r#"(module (memory (export "memory") 1)
 (func (export "bexos-service-version") (result i32) i32.const 1)
 (func (export "bexos-service-dispatch") (param i32) i32.const 0 local.get 0 i32.store)
 (func (export "bexos-checkpoint") (result i64) i64.const 4)
 (func (export "bexos-restore-allocate") (param i32) (result i32) i32.const 0)
 (func (export "bexos-restore") (param i32 i32) (result i32) i32.const 0)
 (func (export "bexos-activate")) (func (export "bexos-abort"))
 (func (export "invoke") (param i32) (result i32) i32.const 0 i32.load))"#;
    let mut original = instance(code);
    run(original.service_version()).unwrap();
    run(original.dispatch(42)).unwrap();
    let checkpoint = run(original.checkpoint()).unwrap();
    let mut candidate = instance(code);
    run(candidate.restore_checkpoint(&checkpoint)).unwrap();
    run(candidate.activate()).unwrap();
    assert_eq!(run(candidate.invoke("invoke", 0)).unwrap(), 42);
}
#[test]
fn aggregate_memory_and_table_limits_cover_siblings() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let mut limits = bexos_wasm_abi::Limits::default();
    limits.max_memory_pages = 2;
    limits.max_table_elements = 2;
    let budget = Budget::for_limits(&limits);
    let code=wat::parse_str("(module (memory 1) (table 1 funcref) (func (export \"invoke\") (param i32) (result i32) local.get 0 memory.grow))").unwrap();
    let make = || {
        let mut c = context();
        c.options.limits = limits.clone();
        Context::new(c.options, c.host, Origin::Signed, budget.clone())
    };
    let mut a = run(CoreInstance::instantiate(
        &engine,
        code.clone().into(),
        make(),
    ))
    .unwrap();
    let b = run(CoreInstance::instantiate(&engine, code.into(), make())).unwrap();
    assert_eq!(run(a.invoke("invoke", 1)).unwrap(), -1);
    drop(b);
    assert_eq!(run(a.invoke("invoke", 1)).unwrap(), 1);
}
#[test]
fn paused_future_keeps_its_live_computation() {
    let control = Arc::new(crate::control::Control::default());
    control.pause();
    let mut future = pin!(crate::control::Controlled::new(control.clone(), async {
        Ok::<_, wasmtime::Error>(42)
    }));
    let w = Waker::from(Arc::new(Noop));
    let mut cx = TaskContext::from_waker(&w);
    assert!(future.as_mut().poll(&mut cx).is_pending());
    control.resume();
    assert_eq!(run(future).unwrap(), 42);
}
#[test]
fn filesystem_paths_reject_escape_and_absolute_names() {
    use crate::wasi::filesystem::safe_path;
    for path in [
        "/secret",
        "../secret",
        "dir/../secret",
        "dir//file",
        "dir/\0file",
    ] {
        assert!(safe_path(path).is_err(), "{path:?}");
    }
    assert!(safe_path("dir/file").is_ok());
}

fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}
#[test]
fn real_child_spawn_invoke_pause_resume_and_terminate() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let child = wat::parse_str(
        r#"(module
        (func (export "double") (param i32) (result i32) local.get 0 i32.const 2 i32.mul))"#,
    )
    .unwrap();
    let mut options = context().options;
    options.limits.fuel = 100_000;
    let config = options.encode().unwrap();
    let mut parent = instance(&format!(
        r#"(module
      (import "bexos:wasm/sandbox@1.0.0" "spawn" (func $spawn (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (import "bexos:wasm/sandbox@1.0.0" "invoke" (func $invoke (param i32 i32 i32 i32) (result i32)))
      (import "bexos:wasm/sandbox@1.0.0" "pause" (func $pause (param i32)))
      (import "bexos:wasm/sandbox@1.0.0" "resume" (func $resume (param i32)))
      (import "bexos:wasm/sandbox@1.0.0" "terminate" (func $terminate (param i32)))
      (import "bexos:wasm/sandbox@1.0.0" "status" (func $status (param i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "{}") (data (i32.const 4096) "{}") (data (i32.const 8192) "double")
      (func (export "spawn") (param i32) (result i32)
        i32.const 0 i32.const {} i32.const 4096 i32.const {} i32.const 0 i32.const 0 i32.const 0 i32.const 0 call $spawn)
      (func (export "call") (param i32) (result i32) local.get 0 i32.const 8192 i32.const 6 i32.const 21 call $invoke)
      (func (export "pause") (param i32) (result i32) local.get 0 call $pause local.get 0 call $status)
      (func (export "resume") (param i32) (result i32) local.get 0 call $resume local.get 0 call $status)
      (func (export "terminate") (param i32) (result i32) local.get 0 call $terminate local.get 0 call $status))"#,
        wat_bytes(&child),
        wat_bytes(&config),
        child.len(),
        config.len()
    ));
    let id = run(parent.invoke("spawn", 0)).unwrap();
    assert_eq!(run(parent.invoke("call", id)).unwrap(), 42);
    assert_eq!(run(parent.invoke("pause", id)).unwrap(), 1);
    assert!(run(parent.invoke("call", id)).is_err());
    assert_eq!(run(parent.invoke("resume", id)).unwrap(), 0);
    assert_eq!(run(parent.invoke("call", id)).unwrap(), 42);
    assert_eq!(run(parent.invoke("terminate", id)).unwrap(), 2);
    assert!(parent.store.data().children.is_empty());
    assert!(run(parent.invoke("call", id)).is_err());
}
#[test]
fn child_admission_checks_all_parent_limits_and_invalid_signatures() {
    let parent = context();
    let bytes = b"\0asm\x01\0\0\0";
    let mut options = parent.options.clone();
    options.limits.fuel = 100_000;
    assert!(
        crate::admission::child_context(&parent, options.clone(), bytes, b"tampered", &[], 100_000)
            .is_err()
    );
    assert!(
        crate::admission::child_context(&parent, options.clone(), bytes, &[], &[1], 100_000)
            .is_err()
    );
    for field in 0..8 {
        let mut bad = options.clone();
        match field {
            0 => bad.limits.max_module_bytes *= 2,
            1 => bad.limits.max_memory_pages *= 2,
            2 => bad.limits.max_table_elements *= 2,
            3 => bad.limits.max_stack_bytes *= 2,
            4 => bad.limits.max_handles *= 2,
            5 => bad.limits.max_children *= 2,
            6 => bad.limits.fuel_slice *= 2,
            _ => bad.limits.fuel += 1,
        }
        assert!(
            crate::admission::child_context(&parent, bad, bytes, &[], &[], 100_000).is_err(),
            "field {field}"
        );
    }
    assert!(crate::admission::child_context(&parent, options, bytes, &[], &[], 100_000).is_ok());
}

#[test]
fn service_component_checkpoint_restores_before_activation() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let bytes =
        wat::parse_str(include_str!("../../../testing/wasm/service_component.wat")).unwrap();
    let mut service = run(crate::service_guest::ServiceGuest::instantiate(
        &engine,
        bytes.into(),
        context(),
    ))
    .unwrap();
    run(service.dispatch(41)).unwrap();
    let snapshot = run(service.snapshot()).unwrap();
    assert_eq!(snapshot.checkpoint, 41u32.to_le_bytes());
    let mut restored = run(snapshot.restore(engine, context(), None)).unwrap();
    assert!(restored.store().data().restoring);
    run(restored.activate()).unwrap();
    run(restored.dispatch(1)).unwrap();
    assert_eq!(run(restored.checkpoint()).unwrap(), 42u32.to_le_bytes());
}
#[test]
fn wasi_checkpoint_adoption_is_typed_single_use_and_preserves_timer() {
    use crate::bindings::bexos::wasm::checkpoint_resources::Host as Hooks;
    use crate::wasi::{state::*, wasi::io::streams::HostInputStream};
    let mut source = context();
    let input = source.push(Input::Empty).unwrap();
    let token = source
        .identify_input(wasmtime::component::Resource::new_borrow(input.rep()))
        .unwrap();
    let timer = source.push(Pollable::Timer(500)).unwrap();
    let timer_token = source
        .identify_pollable(wasmtime::component::Resource::new_borrow(timer.rep()))
        .unwrap();
    let snapshot = source.wasi.snapshot().unwrap();
    let mut candidate = context();
    candidate.restoring = true;
    candidate.wasi.restore(snapshot, 256).unwrap();
    assert!(candidate.adopt_descriptor(token).unwrap().is_err());
    let adopted = candidate.adopt_input(token).unwrap().unwrap();
    assert!(matches!(
        candidate.wasi.table.get(&adopted).unwrap(),
        Input::Empty
    ));
    assert!(candidate.adopt_input(token).unwrap().is_err());
    assert!(
        candidate
            .read(wasmtime::component::Resource::new_borrow(adopted.rep()), 1)
            .is_err()
    );
    let timer = candidate.adopt_pollable(timer_token).unwrap().unwrap();
    assert!(matches!(
        candidate.wasi.table.get(&timer).unwrap(),
        Pollable::Timer(500)
    ));
    assert!(candidate.wasi.restored.is_empty());
    candidate.restoring = false;
    assert!(candidate.adopt_input(token).unwrap().is_err());
}

#[test]
fn unclaimed_ambient_stdio_does_not_block_logical_restore() {
    use crate::wasi::{checkpoint::Saved, output_state::Output, state::*};
    let mut source = context();
    let stderr = source.push(Output::log()).unwrap();
    let stdin = source.push(Input::Empty).unwrap();
    let timer = source.push(Pollable::Timer(500)).unwrap();
    let snapshot = source.wasi.snapshot().unwrap();
    assert!(matches!(
        snapshot.get(&stderr.rep()),
        Some(Saved::Output(_))
    ));
    assert!(matches!(snapshot.get(&stdin.rep()), Some(Saved::Input(_))));

    let mut candidate = context();
    candidate.restoring = true;
    candidate.wasi.restore(snapshot, 256).unwrap();
    candidate.wasi.discard_unclaimed_ambient_io();

    assert_eq!(candidate.wasi.count, 1);
    assert_eq!(candidate.wasi.restored.len(), 1);
    assert!(matches!(
        candidate.wasi.restored.get(&timer.rep()),
        Some(Saved::Pollable(Pollable::Timer(500)))
    ));
}

#[test]
fn core_and_wasi_handles_share_the_child_limit_and_release_reservations() {
    use crate::resources::{Kind, READ};
    use crate::wasi::state::Input;
    struct TestHandle;
    impl Handle for TestHandle {
        fn kind(&self) -> Kind {
            Kind::Channel
        }
        fn rights(&self) -> u32 {
            READ
        }
        fn native(&self) -> u64 {
            1
        }
    }
    let entry = || Entry {
        name: "channel".into(),
        handle: Arc::new(TestHandle),
    };
    let root = Budget::for_limits(&bexos_wasm_abi::Limits::default());
    let mut options = context().options;
    options.limits.max_handles = 2;
    let make = || {
        Context::new(
            options.clone(),
            Arc::new(TestHost::default()),
            Origin::Signed,
            root.clone(),
        )
    };
    let mut child = make();
    let first = child.push(Input::Empty).unwrap();
    let second = child.resources.insert(entry()).unwrap();
    assert!(child.resources.insert(entry()).is_err());
    assert!(child.push(Input::Empty).is_err());
    let called = std::cell::Cell::new(false);
    assert!(
        child
            .resources
            .receive(1, || {
                called.set(true);
                Ok(((), vec![]))
            })
            .is_err()
    );
    assert!(
        !called.get(),
        "full child must not consume the host message"
    );
    child.delete(first).unwrap();
    assert!(
        child
            .resources
            .receive::<()>(1, || wasmtime::bail!("receive failed"))
            .is_err()
    );
    let third = child.resources.insert(entry()).unwrap();
    child.resources.remove(second).unwrap();
    child.resources.remove(third).unwrap();
    assert!(child.push(Input::Empty).is_ok());
    let mut sibling = make();
    assert!(sibling.push(Input::Empty).is_ok());
    assert!(sibling.resources.insert(entry()).is_ok());
}

#[test]
fn embedded_wasi_output_has_no_ambient_log_access() {
    use crate::wasi::wasi::{
        cli::{stderr::Host as Stderr, stdout::Host as Stdout},
        io::streams::{HostOutputStream, StreamError},
    };
    let mut child = context();
    child.child_permit = Some(child.budget.child(1).unwrap());
    let stdout = child.get_stdout().unwrap();
    assert!(matches!(
        child.check_write(stdout).unwrap(),
        Err(StreamError::Closed)
    ));
    let stderr = child.get_stderr().unwrap();
    assert!(matches!(
        child
            .write(stderr, b"must not reach host log".to_vec())
            .unwrap(),
        Err(StreamError::Closed)
    ));
}

#[test]
fn child_stack_limit_is_enforced_with_a_shared_engine() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let bytes: Arc<[u8]> = wat::parse_str(
        r#"(module
        (func $recurse (export "invoke") (param $n i32) (result i32)
            local.get $n i32.eqz
            if (result i32) i32.const 0
            else local.get $n i32.const 1 i32.sub call $recurse i32.const 1 i32.add end))"#,
    )
    .unwrap()
    .into();
    let mut small = context();
    small.options.limits.max_stack_bytes = 65536;
    let mut small = run(CoreInstance::instantiate(&engine, bytes.clone(), small)).unwrap();
    let mut large = run(CoreInstance::instantiate(&engine, bytes, context())).unwrap();
    assert!(run(small.invoke("invoke", 4000)).is_err());
    assert_eq!(run(large.invoke("invoke", 4000)).unwrap(), 4000);
}

#[test]
fn prepared_restore_requires_every_payload_before_cutover() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let bytes: Arc<[u8]> =
        wat::parse_str(include_str!("../../../testing/wasm/service_component.wat"))
            .unwrap()
            .into();
    let mut source = run(crate::service_guest::ServiceGuest::instantiate(
        &engine,
        bytes.clone(),
        context(),
    ))
    .unwrap();
    run(source.dispatch(19)).unwrap();
    let snapshot = run(source.snapshot()).unwrap();
    assert!(
        run(snapshot.clone().restore_prepared(
            engine.clone(),
            context(),
            None,
            Some(Arc::new(vec![]))
        ))
        .is_err()
    );
    let compiled = crate::prepared::PreparedService::compile(&engine, bytes, 1 << 20).unwrap();
    let mut candidate =
        run(snapshot.restore_prepared(engine, context(), None, Some(Arc::new(vec![compiled]))))
            .unwrap();
    run(candidate.activate()).unwrap();
    assert_eq!(run(candidate.checkpoint()).unwrap(), 19u32.to_le_bytes());
}

#[path = "tests/wasi_io.rs"]
mod wasi_io;

mod wasi_network;

#[test]
fn warm_candidate_installs_latest_parent_and_paused_child_state() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let bytes: Arc<[u8]> =
        wat::parse_str(include_str!("../../../testing/wasm/service_component.wat"))
            .unwrap()
            .into();
    let mut source = run(crate::service_guest::ServiceGuest::instantiate(
        &engine,
        bytes.clone(),
        context(),
    ))
    .unwrap();
    let mut child = run(crate::service_guest::ServiceGuest::instantiate(
        &engine,
        bytes.clone(),
        context(),
    ))
    .unwrap();
    run(child.dispatch(30)).unwrap();
    child.control().pause();
    source
        .store_mut()
        .data_mut()
        .children
        .insert(1, Box::new(crate::child::Child::Service(child)));
    source.store_mut().data_mut().next_child = 2;
    run(source.dispatch(10)).unwrap();
    let bulk = run(source.snapshot()).unwrap();
    let prepared = Arc::new(vec![
        crate::prepared::PreparedService::compile(&engine, bytes, 1 << 20).unwrap(),
    ]);
    let mut candidate = run(bulk.prepare_candidate(engine, context(), None, prepared)).unwrap();
    run(source.dispatch(7)).unwrap();
    let final_state = run(source.snapshot()).unwrap();
    let fuel = final_state.remaining_fuel;
    run(final_state.install_checkpoint(&mut candidate, true)).unwrap();
    assert_eq!(run(candidate.checkpoint()).unwrap(), 17u32.to_le_bytes());
    assert!(candidate.store().get_fuel().unwrap() <= fuel);
    let child = candidate
        .store_mut()
        .data_mut()
        .children
        .get_mut(&1)
        .unwrap();
    assert_eq!(child.control().status(), 1);
    let crate::child::Child::Service(child) = &mut **child else {
        panic!()
    };
    assert_eq!(run(child.checkpoint()).unwrap(), 30u32.to_le_bytes());
    assert!(child.store().data().restoring);
    assert!(candidate.store().data().restoring);
}

#[test]
fn ordinary_wasi_command_calls_streams_polling_clock_random_and_environment() {
    struct FixtureHandle(crate::resources::Kind);
    impl Handle for FixtureHandle {
        fn kind(&self) -> crate::resources::Kind {
            self.0
        }
        fn rights(&self) -> u32 {
            crate::resources::READ
        }
        fn native(&self) -> u64 {
            1
        }
    }
    struct FixtureHost(Mutex<Vec<u8>>);
    impl Host for FixtureHost {
        fn open_file(
            &self,
            _: &dyn Handle,
            path: &str,
            _: crate::wasi::wasi::filesystem::types::OpenFlags,
            _: crate::wasi::wasi::filesystem::types::DescriptorFlags,
        ) -> crate::wasi::filesystem::FsResult<Entry> {
            assert_eq!(path, "fixture.txt");
            Ok(Entry {
                name: path.into(),
                handle: Arc::new(FixtureHandle(crate::resources::Kind::File)),
            })
        }
        fn read_file(&self, _: &dyn Handle, offset: u64, len: usize) -> wasmtime::Result<Vec<u8>> {
            assert_eq!((offset, len), (0, 4));
            Ok(b"wasm".to_vec())
        }
        fn write_file(&self, _: &dyn Handle, _: u64, _: &[u8]) -> wasmtime::Result<usize> {
            panic!("read-only descriptor reached host write");
        }

        fn monotonic_ns(&self) -> u64 {
            123
        }
        fn random(&self, len: usize) -> wasmtime::Result<Vec<u8>> {
            Ok(vec![42; len])
        }
        fn log(&self, bytes: &[u8]) {
            self.0.lock().unwrap().extend_from_slice(bytes);
        }
        fn channel_write(&self, _: &dyn Handle, _: &[u8], _: &[Entry]) -> wasmtime::Result<()> {
            panic!("unexpected channel operation");
        }
        fn channel_read(
            &self,
            _: &dyn Handle,
            _: usize,
            _: usize,
        ) -> wasmtime::Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
            panic!("unexpected channel operation");
        }
    }
    let _lock = TEST_ENGINE.lock().unwrap();
    let host = Arc::new(FixtureHost(Mutex::new(Vec::new())));
    let mut ctx = context();
    ctx.host = host.clone();
    ctx.resources
        .insert(Entry {
            name: "/pkg".into(),
            handle: Arc::new(FixtureHandle(crate::resources::Kind::Directory)),
        })
        .unwrap();
    let engine = crate::engine::engine().unwrap();
    let bytes = wat::parse_str(include_str!("../../../testing/wasm/command.wat")).unwrap();
    let mut instance = run(crate::component::CommandInstance::instantiate(
        &engine,
        bytes.into(),
        ctx,
    ))
    .unwrap();
    assert_eq!(run(instance.run()).unwrap(), 0);
    assert_eq!(
        &*host.0.lock().unwrap(),
        b"wasi-fixture: streams clocks random environment ok\n"
    );
}

#[path = "tests/file_service.rs"]
mod file_service;

#[test]
fn embedded_signed_components_have_no_ambient_wasi_clocks_or_randomness() {
    use crate::wasi::wasi::{
        clocks::{monotonic_clock, wall_clock},
        random::random,
    };
    let mut child = context();
    child.child_permit = Some(child.budget.child(1).unwrap());
    assert!(monotonic_clock::Host::now(&mut child).is_err());
    assert!(monotonic_clock::Host::subscribe_instant(&mut child, 0).is_err());
    assert!(wall_clock::Host::now(&mut child).is_err());
    assert!(random::Host::get_random_bytes(&mut child, 8).is_err());
    assert_eq!(child.wasi.count, 0);
}

#[path = "tests/transfers.rs"]
mod transfers;

mod terminal;
