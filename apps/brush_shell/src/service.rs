//! ShellProvider dispatch and transactional lifecycle hooks.
use crate::{engine, session::Session};
use bexos_migration::codec::{Decoder, Encoder};
use bexos_tty::{provider::ProviderSession, transport::Transport};
use bexos_wasm_guest::{exports::bexos::wasm::lifecycle::Guest, tty::WasmTransport};
use shell_fidl::{
    FidlDecode, FidlEncode, HandleRef, ShellProviderCreateSessionRequest,
    ShellProviderCreateSessionResponse,
};
use std::cell::RefCell;

pub struct Service;
#[derive(Default)]
struct State {
    providers: Vec<u64>,
    sessions: Vec<Session>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
impl Guest for Service {
    fn dispatch(resource_id: u32) {
        brush_core::bexos::set_launcher(crate::command::launch);
        STATE.with_borrow_mut(|state| {
            if resource_id != 0 && !state.providers.contains(&(resource_id as u64)) {
                if state.providers.len() >= 64 {
                    let _ = WasmTransport.close(resource_id as u64);
                    return;
                }
                state.providers.push(resource_id as u64);
            }
            let transport = WasmTransport;
            let mut closed = Vec::new();
            for &channel in &state.providers {
                let m = match transport.receive(channel) {
                    Ok(m) => m,
                    Err(kernel_fidl::Status::ErrTimedOut) => continue,
                    Err(_) => {
                        let _ = transport.close(channel);
                        closed.push(channel);
                        continue;
                    }
                };
                let refs: Vec<_> = m.handles.iter().map(|h| HandleRef { raw: *h }).collect();
                let mut status = kernel_fidl::Status::ErrInvalidArgs;
                let mut accepted = false;
                if m.bytes.get(..8) == Some(&1u64.to_le_bytes()) && state.sessions.len() < 16 {
                    if let Ok(q) = ShellProviderCreateSessionRequest::decode(&m.bytes[8..], &refs) {
                        if let Some(environment) = environment(q.environment) {
                            if q.session.raw != 0 && m.handles.len() == 1 {
                                accepted = true;
                                match Session::new(q.session.raw, environment) {
                                    Ok(session) => {
                                        state.sessions.push(session);
                                        status = kernel_fidl::Status::Ok;
                                    }
                                    Err(e) => {
                                        status = e;
                                    }
                                }
                            }
                        }
                    }
                }
                if !accepted {
                    for h in m.handles {
                        let _ = transport.close(h);
                    }
                }
                let response = ShellProviderCreateSessionResponse { status };
                let mut bytes = [0; 64];
                let mut handles = [];
                if let Ok(n) = response.encode(&mut bytes, &mut handles) {
                    let _ = transport.send(channel, &bytes[..n.bytes], &[]);
                }
            }
            state.providers.retain(|h| !closed.contains(h));
            state.sessions.retain_mut(|s| {
                if s.tick().is_ok() {
                    true
                } else {
                    s.tty.close_with(&transport);
                    false
                }
            });
        });
    }
    fn checkpoint() -> Vec<u8> {
        STATE.with_borrow_mut(|state| {
            // An unsupported live evaluator must reject transplant, never silently
            // omit jobs or deserialize open files as /dev/null.
            if state.sessions.iter().any(|s| {
                s.running.is_some()
                    || s.runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.metrics().num_alive_tasks() != 0)
                    || s.shell.as_ref().is_none_or(|shell| {
                        !shell.jobs().jobs.is_empty()
                            || !shell.open_files().iter_fds().all(|(fd, f)| {
                                (0..3).contains(&fd)
                                    && matches!(f, brush_core::openfiles::OpenFile::Stream(stream) if stream.stream_type()==std::any::type_name::<engine::Buffer>())
                            })
                    })
            }) {
                bexos_wasm_guest::bexos::wasm::kernel::checkpoint_defer();
                return Vec::new();
            }
            let mut w = Encoder::new();
            w.word(2);
            w.word(state.providers.len() as u64);
            for h in &state.providers {
                w.word(*h);
            }
            w.word(state.sessions.len() as u64);
            for s in &mut state.sessions {
                s.tty.encode(&mut w);
                w.bytes(&s.line);
                w.bytes(&s.input);
                w.word(s.eof as u64);
                for stream in &s.streams {
                    w.bytes(stream.0.lock().unwrap().make_contiguous());
                    w.word(stream.1.load(std::sync::atomic::Ordering::Relaxed) as u64);
                }
                let shell = s.shell.as_mut().unwrap();

                let files = std::mem::take(shell.open_files_mut());
                let encoded = serde_json::to_vec(shell);
                *shell.open_files_mut() = files;
                match encoded {
                    Ok(bytes) => w.bytes(&bytes),
                    Err(_) => {
                        bexos_wasm_guest::bexos::wasm::kernel::checkpoint_defer();
                        return Vec::new();
                    }
                }
            }
            w.finish()
        })
    }
    fn restore(bytes: Vec<u8>) -> Result<(), ()> {
        fn decode(bytes: &[u8]) -> Result<State, ()> {
            let mut r = Decoder::new(bytes);
            let version = r.word().map_err(|_| ())?;
            if !matches!(version, 1 | 2) {
                return Err(());
            }
            let n = r.count(64).map_err(|_| ())?;
            let mut providers = Vec::new();
            for _ in 0..n {
                let h = r.word().map_err(|_| ())?;
                if h == 0 || providers.contains(&h) {
                    return Err(());
                }
                providers.push(h);
            }
            let n = r.count(16).map_err(|_| ())?;
            let mut sessions = Vec::new();
            for _ in 0..n {
                let tty = ProviderSession::decode(&mut r).map_err(|_| ())?;
                let line = r.bytes(65536).map_err(|_| ())?.to_vec();
                let input = r.bytes(4096).map_err(|_| ())?.to_vec();
                let eof = r.flag().map_err(|_| ())?;
                let streams: [engine::Buffer; 3] = Default::default();
                for stream in &streams {
                    stream
                        .append(r.bytes(engine::BUFFER_LIMIT).map_err(|_| ())?)
                        .map_err(|_| ())?;
                    if version >= 2 {
                        stream.1.store(
                            r.flag().map_err(|_| ())?,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                }
                let mut shell: engine::Brush =
                    serde_json::from_slice(r.bytes(1 << 20).map_err(|_| ())?).map_err(|_| ())?;
                for (name, builtin) in
                    brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode)
                {
                    shell.register_builtin(name, builtin);
                }
                engine::attach(&mut shell, &streams);
                sessions.push(Session {
                    tty,
                    shell: Some(shell),
                    streams,
                    line,
                    input,
                    eof,
                    running: None,
                    interrupted: false,
                    runtime: None,
                });
            }
            r.finish().map_err(|_| ())?;
            use bexos_wasm_guest::bexos::wasm::kernel::{self, ResourceKind};
            let mut owned = std::collections::BTreeSet::new();
            let mut validate = |raw: u64, kind: ResourceKind, rights: u32| -> Result<(), ()> {
                let id = u32::try_from(raw).map_err(|_| ())?;
                if id == 0 || !owned.insert(id) {
                    return Err(());
                }
                let info = kernel::inspect_resource(id).ok_or(())?;
                if info.kind != kind || info.rights & rights != rights {
                    return Err(());
                }
                Ok(())
            };
            for &provider in &providers {
                validate(provider, ResourceKind::Channel, 3)?;
            }
            for session in &sessions {
                validate(session.tty.control, ResourceKind::Channel, 3)?;
                for (i, &stream) in session.tty.streams.iter().enumerate() {
                    validate(stream, ResourceKind::Socket, if i == 0 { 1 } else { 2 })?;
                }
                for &stream in &session.tty.frontend {
                    if stream != 0 {
                        validate(stream, ResourceKind::Socket, 4)?;
                    }
                }
            }
            Ok(State {
                providers,
                sessions,
            })
        }
        let restored = decode(&bytes)?;
        STATE.with_borrow_mut(|s| *s = restored);
        Ok(())
    }
    fn activate() {}
    fn abort() {}
}
fn environment(wire: shell_fidl::WireStringVector<'_>) -> Option<Vec<(String, String)>> {
    let mut result: Vec<(String, String)> = Vec::new();
    for i in 0..wire.len() {
        let value = wire.get(i).ok()?;
        let (name, value) = value.split_once('=')?;
        if name.is_empty()
            || name.contains('\0')
            || value.contains('\0')
            || result.iter().any(|(n, _)| n == name)
        {
            return None;
        }
        result.push((name.into(), value.into()));
    }
    Some(result)
}
