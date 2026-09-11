#![no_main]
extern crate alloc;
mod migration;
use alloc::{vec, vec::Vec};
use bexos_tty::provider::ProviderSession;
use bexos_userspace::{
    Channel, Memory, Socket, Startup,
    live_migration::{Source, State},
};
use kernel_fidl::Status;
use shell_fidl::{FidlDecode, FidlEncode, HandleRef};
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).unwrap();
    let mut state = if startup.migration_target {
        bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            startup.migration_generation,
        )
        .unwrap()
    } else {
        let mut s = migration::Runtime::empty();
        s.control = control;
        s.migration = startup.migration;
        Startup::ready(control).unwrap();
        s
    };
    let mut source = Source::new(state.migration);
    loop {
        let _ = source.poll(&state);
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if let Ok(m) = state.control.try_recv() {
            if m.bytes
                .starts_with(b"bexos.opener.interface.v1\nbexos.shell.ShellProvider")
            {
                state.providers.extend(m.handles);
                mark_changed(&mut source);
            } else {
                for h in m.handles {
                    let _ = Memory::close(h);
                }
            }
        }
        if let Some(s) = &mut state.session {
            let mut peer_closed = false;
            let changed = match s.poll() {
                Ok(v) => v,
                Err(Status::ErrTimedOut) => false,
                Err(e) => {
                    peer_closed = e == Status::ErrPeerClosed;
                    s.exited = true;
                    true
                }
            };
            if changed {
                mark_changed(&mut source);
            }
            if !s.exited {
                if state.output.is_empty() && s.signals.is_empty() {
                    let next = if state.input.is_empty() {
                        Socket(s.streams[0]).read(4096)
                    } else {
                        Ok(core::mem::take(&mut state.input))
                    };
                    match next {
                        Ok(bytes) => {
                            let input = s.discipline.feed(&bytes);
                            state.input.extend_from_slice(&bytes[input.consumed..]);
                            state.output.extend(input.echo);
                            state.output.extend(input.bytes);
                            s.signals.extend(input.signals);
                            if input.eof {
                                state.eof = true;
                            }
                            mark_changed(&mut source);
                        }
                        Err(Status::ErrPeerClosed) => {
                            state.output.append(&mut s.discipline.pending);
                            state.eof = true;
                            mark_changed(&mut source);
                        }
                        Err(_) => {}
                    }
                }
                if !state.output.is_empty() {
                    if let Ok(n) = Socket(s.streams[1]).write(&state.output) {
                        state.output.drain(..n as usize);
                        mark_changed(&mut source);
                    }
                }
                if state.errors.is_empty() && !s.signals.is_empty() {
                    let signal = s.signals.remove(0);
                    let bytes = alloc::format!(
                        "signal={} rows={} cols={}\n",
                        signal as u32,
                        s.size.rows,
                        s.size.cols
                    );
                    state.errors.extend_from_slice(bytes.as_bytes());
                    if matches!(
                        signal,
                        bexos_tty::Signal::Terminate | bexos_tty::Signal::Kill
                    ) {
                        state.eof = true;
                    }
                    mark_changed(&mut source);
                }
                if !state.errors.is_empty() {
                    if let Ok(n) = Socket(s.streams[2]).write(&state.errors) {
                        state.errors.drain(..n as usize);
                        mark_changed(&mut source);
                    }
                }
                if state.eof
                    && state.output.is_empty()
                    && state.errors.is_empty()
                    && s.signals.is_empty()
                {
                    s.exited = true;
                    s.exit_code = 0;
                    let _ = Socket(s.streams[1]).shutdown(false, true);
                    let _ = Socket(s.streams[2]).shutdown(false, true);
                    mark_changed(&mut source);
                }
            }
            // Keep exited session until the frontend explicitly closes its control channel.
            if peer_closed {
                s.close();
                state.session = None;
                state.eof = false;
                state.input.clear();
                state.output.clear();
                state.errors.clear();
                mark_changed(&mut source);
            }
        }
        for c in state.providers.clone() {
            let message = Channel(c).try_recv();
            if matches!(message, Err(Status::ErrPeerClosed)) {
                state.providers.retain(|h| *h != c);
                let _ = Memory::close(c);
                mark_changed(&mut source);
            }
            if let Ok(m) = message {
                let hs: Vec<_> = m.handles.iter().map(|h| HandleRef { raw: *h }).collect();
                let result = if m.bytes.len() >= 8
                    && u64::from_le_bytes(m.bytes[..8].try_into().unwrap()) == 1
                {
                    shell_fidl::ShellProviderCreateSessionRequest::decode(&m.bytes[8..], &hs).ok()
                } else {
                    None
                };
                let status = match result {
                    Some(q) if state.session.is_none() => match ProviderSession::new(q.session.raw)
                    {
                        Ok(s) => {
                            state.session = Some(s);
                            Status::Ok
                        }
                        Err(e) => {
                            let _ = Memory::close(q.session.raw);
                            e
                        }
                    },
                    _ => {
                        for h in m.handles {
                            let _ = Memory::close(h);
                        }
                        Status::ErrAlreadyExists
                    }
                };
                let q = shell_fidl::ShellProviderCreateSessionResponse { status };
                let mut b = [0; 32];
                let mut h = [HandleRef { raw: 0 }; 1];
                if let Ok(n) = q.encode(&mut b, &mut h) {
                    let _ = Channel(c).send(&b[..n.bytes], &[]);
                }
                mark_changed(&mut source);
            }
        }
        bexos_userspace::yield_now();
    }
}

fn mark_changed(source: &mut Source) {
    for key in [0, 1 << 32, (1 << 32) | 1, 2 << 32, 3 << 32] {
        source.changed(key);
    }
}
