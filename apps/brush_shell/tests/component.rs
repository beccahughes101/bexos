use bexos_wasm_runtime::{
    budget::Budget,
    context::Context,
    host::Host,
    resources::{Entry, Handle, Kind, Origin, READ, TRANSFER, WRITE},
    service_component::ServiceInstance,
};
use shell_fidl::{FidlEncode, HandleRef};
#[cfg(feature = "bexos_heap")]
mod heap;
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    pin::pin,
    sync::{Arc, Mutex},
    task::{Poll, Wake, Waker},
};
struct Wakeup;
impl Wake for Wakeup {
    fn wake(self: Arc<Self>) {}
}
fn run<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let w = Waker::from(Arc::new(Wakeup));
    for _ in 0..100000 {
        if let Poll::Ready(r) = f.as_mut().poll(&mut std::task::Context::from_waker(&w)) {
            return r;
        }
    }
    panic!("bounded guest polling exceeded");
}
struct H {
    id: u64,
    kind: Kind,
}
impl Handle for H {
    fn kind(&self) -> Kind {
        self.kind
    }
    fn rights(&self) -> u32 {
        READ | WRITE | TRANSFER
    }
    fn native(&self) -> u64 {
        self.id
    }
}
fn h(id: u64, kind: Kind) -> Arc<dyn Handle> {
    Arc::new(H { id, kind })
}
type Message = (Vec<u8>, Vec<Arc<dyn Handle>>);
#[derive(Default)]
struct State {
    next: u64,
    input: BTreeMap<u64, VecDeque<Message>>,
    replies: BTreeMap<u64, VecDeque<Message>>,
    sockets: BTreeMap<u64, VecDeque<u8>>,
    closed: std::collections::BTreeSet<u64>,
    launchers: std::collections::BTreeSet<u64>,
    commands: usize,
    output_blocked: bool,
}
#[derive(Default)]
struct TestHost(Mutex<State>);
impl TestHost {
    fn queue(&self, ch: u64, bytes: Vec<u8>, handles: Vec<Arc<dyn Handle>>) {
        self.0
            .lock()
            .unwrap()
            .input
            .entry(ch)
            .or_default()
            .push_back((bytes, handles));
    }
    fn reply(&self, ch: u64) -> Message {
        self.0
            .lock()
            .unwrap()
            .replies
            .entry(ch)
            .or_default()
            .pop_front()
            .expect("missing reply")
    }
}
fn response<T: app_opener_fidl::FidlEncode>(value: T) -> Vec<u8> {
    let mut bytes = vec![0; 4096];
    let mut handles = [app_opener_fidl::HandleRef { raw: 0 }; 4];
    let n = value.encode(&mut bytes, &mut handles).unwrap();
    bytes.truncate(n.bytes);
    bytes
}
impl Host for TestHost {
    fn channel_pair(&self) -> wasmtime::Result<(Arc<dyn Handle>, Arc<dyn Handle>)> {
        let mut s = self.0.lock().unwrap();
        s.next += 2;
        let n = s.next + 1000;
        Ok((h(n, Kind::Channel), h(n + 1, Kind::Channel)))
    }
    fn duplicate_handle(&self, handle: &dyn Handle) -> wasmtime::Result<Arc<dyn Handle>> {
        Ok(h(handle.native(), handle.kind()))
    }
    fn open_file(
        &self,
        directory: &dyn Handle,
        path: &str,
        _open: bexos_wasm_runtime::wasi::wasi::filesystem::types::OpenFlags,
        _flags: bexos_wasm_runtime::wasi::wasi::filesystem::types::DescriptorFlags,
    ) -> bexos_wasm_runtime::wasi::filesystem::FsResult<Entry> {
        if directory.native() == 40 && path == "." {
            Ok(Entry {
                name: path.into(),
                handle: h(40, Kind::Directory),
            })
        } else {
            Err(bexos_wasm_runtime::wasi::wasi::filesystem::types::ErrorCode::NoEntry)
        }
    }

    fn monotonic_ns(&self) -> u64 {
        1_000_000_000
    }
    fn wall_clock_ns(&self) -> wasmtime::Result<u64> {
        Ok(1_700_000_000_000_000_000)
    }
    fn random(&self, n: usize) -> wasmtime::Result<Vec<u8>> {
        Ok(vec![42; n])
    }
    fn log(&self, b: &[u8]) {
        eprintln!("guest log: {}", String::from_utf8_lossy(b));
    }
    fn channel_read(&self, ch: &dyn Handle, _: usize, _: usize) -> wasmtime::Result<Message> {
        self.0
            .lock()
            .unwrap()
            .input
            .entry(ch.native())
            .or_default()
            .pop_front()
            .ok_or_else(|| {
                wasmtime::Error::from(std::io::Error::from(std::io::ErrorKind::WouldBlock))
            })
    }
    fn channel_write(
        &self,
        ch: &dyn Handle,
        bytes: &[u8],
        handles: &[Entry],
    ) -> wasmtime::Result<()> {
        {
            use app_opener_fidl::{FidlDecode, OpenerStatus};
            let mut state = self.0.lock().unwrap();
            let ordinal = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            let refs: Vec<_> = handles
                .iter()
                .map(|e| app_opener_fidl::HandleRef {
                    raw: e.handle.native(),
                })
                .collect();
            if ch.native() == 30 {
                assert_eq!(ordinal, 6);
                assert_eq!(handles.len(), 1);
                state.launchers.insert(handles[0].handle.native() ^ 1);
                state.input.entry(30).or_default().push_back((
                    response(app_opener_fidl::OpenerBindCommandLauncherResponse {
                        status: OpenerStatus::Ok,
                    }),
                    vec![],
                ));
                return Ok(());
            }
            if state.launchers.contains(&ch.native()) {
                if ordinal == 1 {
                    let q = app_opener_fidl::CommandLauncherResolveCommandRequest::decode(
                        &bytes[8..],
                        &refs,
                    )
                    .unwrap();
                    let found = matches!(
                        q.name,
                        "cat" | "/system/bin/cat" | "bexos.app.brush_shell:cat"
                    );
                    state.input.entry(ch.native()).or_default().push_back((
                        response(app_opener_fidl::CommandLauncherResolveCommandResponse {
                            status: if found {
                                OpenerStatus::Ok
                            } else {
                                OpenerStatus::NotFound
                            },
                            package_name: "bexos.app.brush_shell",
                            process_name: "cat",
                            executable: "/pkg/bin/utilities.wasm",
                        }),
                        vec![],
                    ));
                } else {
                    assert_eq!(ordinal, 2);
                    let q = app_opener_fidl::CommandLauncherLaunchCommandRequest::decode(
                        &bytes[8..],
                        &refs,
                    )
                    .unwrap();
                    assert_eq!(q.command_name, "cat");
                    assert_eq!(q.arguments.get(0).unwrap(), "sample");
                    assert!(
                        (0..q.environment.len())
                            .any(|i| q.environment.get(i).unwrap() == "HOME=/data")
                    );
                    assert_eq!(q.cwd.raw, 40);
                    state.commands += 1;
                    state
                        .sockets
                        .get_mut(&(q.stdout_stream.raw ^ 1))
                        .unwrap()
                        .extend(b"external command output\n");
                    state.closed.insert(q.stdout_stream.raw ^ 1);
                    state.closed.insert(q.stderr_stream.raw ^ 1);
                    state.input.entry(ch.native()).or_default().push_back((
                        response(app_opener_fidl::CommandLauncherLaunchCommandResponse {
                            status: OpenerStatus::Ok,
                            process_control: app_opener_fidl::HandleRef { raw: 50 },
                        }),
                        vec![h(50, Kind::Channel)],
                    ));
                }
                return Ok(());
            }
            if ch.native() == 50 {
                assert_eq!(ordinal, 1);
                state.input.entry(50).or_default().push_back((
                    response(app_opener_fidl::ProcessControlGetStatusResponse {
                        status: OpenerStatus::Ok,
                        exited: true,
                        suspended: false,
                        exit_code: 23,
                    }),
                    vec![],
                ));
                return Ok(());
            }
        }
        self.0
            .lock()
            .unwrap()
            .replies
            .entry(ch.native())
            .or_default()
            .push_back((
                bytes.to_vec(),
                handles.iter().map(|e| e.handle.clone()).collect(),
            ));
        Ok(())
    }
    fn socket_pair(&self) -> wasmtime::Result<(Arc<dyn Handle>, Arc<dyn Handle>)> {
        let mut s = self.0.lock().unwrap();
        s.next += 2;
        let n = s.next + 100;
        s.sockets.insert(n, VecDeque::new());
        s.sockets.insert(n + 1, VecDeque::new());
        Ok((h(n, Kind::Socket), h(n + 1, Kind::Socket)))
    }
    fn socket_read(&self, h: &dyn Handle, n: usize) -> wasmtime::Result<Vec<u8>> {
        let mut s = self.0.lock().unwrap();
        let b = s.sockets.get_mut(&h.native()).unwrap();
        let n = n.min(b.len());
        Ok(b.drain(..n).collect())
    }
    fn socket_write(&self, h: &dyn Handle, b: &[u8]) -> wasmtime::Result<usize> {
        if self.0.lock().unwrap().output_blocked {
            return Ok(0);
        }
        self.0
            .lock()
            .unwrap()
            .sockets
            .get_mut(&(h.native() ^ 1))
            .unwrap()
            .extend(b);
        Ok(b.len())
    }
    fn socket_ready(&self, h: &dyn Handle, write: bool) -> wasmtime::Result<bool> {
        let s = self.0.lock().unwrap();
        Ok(write || s.closed.contains(&h.native()) || !s.sockets[&h.native()].is_empty())
    }
    fn socket_half_close(&self, _: &dyn Handle, _: bool, _: bool) -> wasmtime::Result<()> {
        Ok(())
    }
}
fn context(host: Arc<TestHost>) -> Context {
    let mut options = bexos_wasm_abi::WasmRunnerOptions {
        path: "/pkg/bin/brush.wasm".into(),
        ..Default::default()
    };
    options.limits.max_module_bytes = 32 << 20;
    options.limits.max_memory_pages = 2048;
    options.limits.max_stack_bytes = 1048576;
    Context::new(options, host, Origin::Signed, Budget::new(256 << 20))
}
#[test]
fn real_brush_component_serves_terminal_and_preserves_idle_variables() {
    let host = Arc::new(TestHost::default());
    let engine =
        bexos_wasm_runtime::engine::configured_engine(&context(host.clone()).options.limits)
            .unwrap();
    let bytes: Arc<[u8]> = std::fs::read(env!("BRUSH_WASM")).unwrap().into();
    let mut c = context(host.clone());
    for (name, id, kind) in [
        ("Opener", 30, Kind::Channel),
        ("/data", 40, Kind::Directory),
    ] {
        c.resources
            .insert(Entry {
                name: name.into(),
                handle: h(id, kind),
            })
            .unwrap();
    }
    let provider = c
        .resources
        .insert(Entry {
            name: "bexos.shell.ShellProvider".into(),
            handle: h(10, Kind::Channel),
        })
        .unwrap();
    eprintln!("instantiating Brush");
    let compile_start = std::time::Instant::now();
    let mut guest = run(ServiceInstance::instantiate(&engine, bytes.clone(), c)).unwrap();
    eprintln!("Brush instantiation elapsed: {:?}", compile_start.elapsed());
    #[cfg(feature = "bexos_heap")]
    heap::assert_no_fallback();
    eprintln!("activating Brush");
    run(guest.activate()).unwrap();
    let env = [
        "USER=test",
        "HOME=/data",
        "PATH=/pkg/bin:/system/bin",
        "TERM=xterm-256color",
    ];
    let req = shell_fidl::ShellProviderCreateSessionRequest {
        session: HandleRef { raw: 1 },
        environment: shell_fidl::WireStringVector::from_slice(&env),
    };
    let mut buf = vec![0; 4096];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let n = req.encode(&mut buf, &mut handles).unwrap();
    let mut wire = 1u64.to_le_bytes().to_vec();
    wire.extend_from_slice(&buf[..n.bytes]);
    host.queue(10, wire, vec![h(20, Kind::Channel)]);
    eprintln!("creating terminal");
    run(guest.dispatch(provider)).unwrap();
    use shell_fidl::FidlDecode;
    assert_eq!(
        shell_fidl::ShellProviderCreateSessionResponse::decode(&host.reply(10).0, &[])
            .unwrap()
            .status,
        kernel_fidl::Status::Ok
    );
    host.queue(20, 1u64.to_le_bytes().to_vec(), vec![]);
    run(guest.dispatch(0)).unwrap();
    let (_, streams) = host.reply(20);
    assert_eq!(streams.len(), 3);
    host.socket_write(&*streams[0], b"x=41; echo $((x+1))\n")
        .unwrap();
    run(guest.dispatch(0)).unwrap();
    assert!(
        run(guest.checkpoint()).is_err(),
        "pending evaluator must defer safely"
    );
    for _ in 0..256 {
        run(guest.dispatch(0)).unwrap();
    }
    let out = host.socket_read(&*streams[1], 32768).unwrap();
    assert!(
        String::from_utf8_lossy(&out).contains("42\n"),
        "{}",
        String::from_utf8_lossy(&out)
    );
    eprintln!("testing external launch");
    host.socket_write(&*streams[0], b"cat sample; echo status=$?\n")
        .unwrap();
    for _ in 0..256 {
        run(guest.dispatch(0)).unwrap();
    }
    let out = host.socket_read(&*streams[1], 32768).unwrap();
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("external command output\n"), "{out}");
    assert!(out.contains("status=23\n"), "{out}");
    assert_eq!(host.0.lock().unwrap().commands, 1);
    host.socket_write(
        &*streams[0],
        b"echo $(echo substitution); echo input | echo pipeline; (echo background) & wait\n",
    )
    .unwrap();
    for _ in 0..512 {
        run(guest.dispatch(0)).unwrap();
    }
    let out = host.socket_read(&*streams[1], 32768).unwrap();
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("substitution\n"), "{out}");
    assert!(out.contains("pipeline\n"), "{out}");
    assert!(out.contains("background\n"), "{out}");
    // A large builtin must keep control messages responsive while stdout is full.
    let mode = tty_fidl::PtySessionSetTerminalModeRequest {
        flags: tty_fidl::TerminalMode(0),
    };
    let mut body = [0; 64];
    let n = tty_fidl::FidlEncode::encode(&mode, &mut body, &mut []).unwrap();
    let mut wire = 3u64.to_le_bytes().to_vec();
    wire.extend_from_slice(&body[..n.bytes]);
    host.queue(20, wire, vec![]);
    run(guest.dispatch(0)).unwrap();
    host.reply(20);
    let mut input = b"echo '".to_vec();
    input.extend(std::iter::repeat_n(b'a', 40000));
    input.extend_from_slice(b"'\n");
    host.socket_write(&*streams[0], &input).unwrap();
    host.0.lock().unwrap().output_blocked = true;
    for _ in 0..128 {
        run(guest.dispatch(0)).unwrap();
    }
    assert!(run(guest.checkpoint()).is_err());
    host.queue(20, 5u64.to_le_bytes().to_vec(), vec![]);
    run(guest.dispatch(0)).unwrap();
    let status = host.reply(20);
    assert!(
        !<tty_fidl::PtySessionGetStatusResponse as tty_fidl::FidlDecode>::decode(&status.0, &[])
            .unwrap()
            .exited
    );
    host.0.lock().unwrap().output_blocked = false;
    for _ in 0..256 {
        run(guest.dispatch(0)).unwrap();
    }
    let output = host.socket_read(&*streams[1], 65536).unwrap();
    assert_eq!(output.iter().filter(|b| **b == b'a').count(), 40000);
    eprintln!("testing interruption");
    host.socket_write(&*streams[0], b"while :; do :; done\n")
        .unwrap();
    for _ in 0..32 {
        run(guest.dispatch(0)).unwrap();
    }
    assert!(run(guest.checkpoint()).is_err());
    let signal = tty_fidl::PtySessionSendSignalRequest {
        signal: tty_fidl::Signal::Interrupt,
    };
    use tty_fidl::FidlEncode as _;
    let mut encoded = [0; 64];
    let n = signal.encode(&mut encoded, &mut []).unwrap();
    let mut request = 4u64.to_le_bytes().to_vec();
    request.extend_from_slice(&encoded[..n.bytes]);
    host.queue(20, request, vec![]);
    for _ in 0..256 {
        run(guest.dispatch(0)).unwrap();
    }
    host.reply(20);
    host.socket_write(&*streams[0], b"echo $").unwrap();
    run(guest.dispatch(0)).unwrap();
    let checkpoint = run(guest.checkpoint()).unwrap();
    let resources = guest
        .store
        .data()
        .resources
        .entries()
        .map(|(id, e)| (id, e.clone()))
        .collect();
    let next = guest.store.data().resources.next_id();
    let mut c = context(host.clone());
    c.restoring = true;
    c.insecure_seed = guest.store.data().insecure_seed;
    c.resources.restore(next, resources).unwrap();
    let mut replacement = run(ServiceInstance::instantiate(&engine, bytes, c)).unwrap();
    let mut invalid = checkpoint.clone();
    let provider_id = invalid[16..24].to_vec();
    invalid[32..40].copy_from_slice(&provider_id);
    assert!(
        run(replacement.restore_checkpoint(&invalid)).is_err(),
        "restore must reject a control handle aliased by another owner"
    );
    // Failed validation has not replaced state or performed external I/O.
    run(replacement.restore_checkpoint(&checkpoint)).unwrap();
    run(replacement.activate()).unwrap();
    host.socket_write(&*streams[0], b"x; exit 7\n").unwrap();
    for _ in 0..256 {
        run(replacement.dispatch(0)).unwrap();
    }
    let out = host.socket_read(&*streams[1], 32768).unwrap();
    assert!(
        String::from_utf8_lossy(&out).contains("41\n"),
        "{}",
        String::from_utf8_lossy(&out)
    );
    host.queue(20, 5u64.to_le_bytes().to_vec(), vec![]);
    run(replacement.dispatch(0)).unwrap();
    let (out, _) = host.reply(20);
    use tty_fidl::FidlDecode as _;
    let status = tty_fidl::PtySessionGetStatusResponse::decode(&out, &[]).unwrap();
    assert!(status.exited);
    assert_eq!(status.exit_code, 7);
    // Execute the packaged utility component through actual WASI stdin/stdout.
    for (arguments, expected, exit) in [
        (vec!["cat"], "alpha\nbeta\n", 0),
        (vec!["grep", "-n", "beta"], "2:beta\n", 0),
        (vec!["grep", "absent"], "", 1),
    ] {
        let host = Arc::new(TestHost::default());
        let mut context = context(host.clone());
        context.options.arguments = arguments.into_iter().map(str::to_owned).collect();
        let (input, stdin) = host.socket_pair().unwrap();
        let (output, stdout) = host.socket_pair().unwrap();
        let (_, stderr) = host.socket_pair().unwrap();
        host.socket_write(&*input, b"alpha\nbeta\n").unwrap();
        host.0.lock().unwrap().closed.insert(stdin.native());
        for (name, handle) in [
            ("wasi:stdin", stdin),
            ("wasi:stdout", stdout),
            ("wasi:stderr", stderr),
        ] {
            context
                .resources
                .insert(Entry {
                    name: name.into(),
                    handle,
                })
                .unwrap();
        }
        let bytes: Arc<[u8]> = std::fs::read(env!("UTILITIES_WASM")).unwrap().into();
        let mut utility = run(bexos_wasm_runtime::component::CommandInstance::instantiate(
            &engine, bytes, context,
        ))
        .unwrap();
        assert_eq!(run(utility.run()).unwrap(), exit);
        assert_eq!(
            host.socket_read(&*output, 32768).unwrap(),
            expected.as_bytes()
        );
    }
    #[cfg(feature = "bexos_heap")]
    heap::assert_no_fallback();
}
