//! Per-terminal shell execution and bounded transport pumping.
use crate::engine::{self, Brush, Buffer};
use bexos_tty::{Signal, provider::ProviderSession};
use bexos_wasm_guest::{bexos::wasm::kernel, tty::WasmTransport};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

type Execution =
    Pin<Box<dyn Future<Output = (Brush, Result<brush_core::ExecutionResult, String>)>>>;
pub struct Session {
    pub tty: ProviderSession,
    pub shell: Option<Brush>,
    pub streams: [Buffer; 3],
    pub line: Vec<u8>,
    pub input: Vec<u8>,
    pub eof: bool,
    pub running: Option<Execution>,
    pub interrupted: bool,
    pub runtime: Option<tokio::runtime::Runtime>,
}
struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}
fn poll<F: Future + ?Sized>(f: Pin<&mut F>) -> Poll<F::Output> {
    let waker = Waker::from(Arc::new(Noop));
    f.poll(&mut Context::from_waker(&waker))
}
impl Session {
    pub fn new(
        control: u64,
        environment: Vec<(String, String)>,
    ) -> Result<Self, kernel_fidl::Status> {
        let tty = match ProviderSession::new_with(control, &WasmTransport) {
            Ok(tty) => tty,
            Err(e) => {
                let _ = kernel::resource_close(control as u32);
                return Err(e);
            }
        };
        let runtime = runtime().map_err(|_| kernel_fidl::Status::ErrResourceExhausted)?;
        let streams: [Buffer; 3] = Default::default();
        let mut init = Box::pin(engine::create(environment, streams.clone()));
        let shell = match poll(init.as_mut()) {
            Poll::Ready(Ok(shell)) => shell,
            _ => {
                tty.close_with(&WasmTransport);
                return Err(kernel_fidl::Status::ErrInvalidArgs);
            }
        };
        let _ = streams[1].append(b"brush$ ");
        Ok(Self {
            tty,
            shell: Some(shell),
            streams,
            line: Vec::new(),
            input: Vec::new(),
            eof: false,
            running: None,
            interrupted: false,
            runtime: Some(runtime),
        })
    }
    pub fn tick(&mut self) -> Result<(), kernel_fidl::Status> {
        if self.runtime.is_none() {
            self.runtime = Some(runtime().map_err(|_| kernel_fidl::Status::ErrResourceExhausted)?);
        }
        for _ in 0..8 {
            if !self.tty.poll_with(&WasmTransport)? {
                break;
            }
        }
        self.flush();
        for signal in std::mem::take(&mut self.tty.signals) {
            match signal {
                Signal::Kill | Signal::Terminate => {
                    self.running = None;
                    self.tty.exited = true;
                    self.tty.exit_code = if signal == Signal::Kill { 137 } else { 143 };
                }
                Signal::Interrupt if self.running.is_none() => {
                    self.line.clear();
                    if let Some(s) = &mut self.shell {
                        s.set_last_exit_status(130);
                    }
                    let _ = self.streams[1].append(b"\nbrush$ ");
                }
                Signal::Interrupt => self.interrupted = true,
                _ => (),
            }
        }
        if self.tty.exited {
            self.close_output_when_drained();
            return Ok(());
        }
        if self.input.is_empty() && !self.eof {
            match kernel::socket_read(self.tty.streams[0] as u32, 4096) {
                Ok(bytes) if bytes.is_empty() => self.eof = true,
                Ok(bytes) => self.input = bytes,
                Err(kernel::StreamError::WouldBlock) => (),
                Err(_) => self.eof = true,
            }
        }
        if !self.input.is_empty()
            && self.streams[1].0.lock().unwrap().len() < 8192
            && self.streams[0].0.lock().unwrap().len() < 8192
        {
            let input = self.tty.discipline.feed(&self.input);
            self.input.drain(..input.consumed);
            self.streams[1]
                .append(&input.echo)
                .map_err(|_| kernel_fidl::Status::ErrResourceExhausted)?;
            self.tty.signals.extend(input.signals);
            self.eof |= input.eof;
            if self.line.len() + input.bytes.len() > 65536 {
                return Err(kernel_fidl::Status::ErrResourceExhausted);
            }
            if self.running.is_some() {
                self.streams[0]
                    .append(&input.bytes)
                    .map_err(|_| kernel_fidl::Status::ErrResourceExhausted)?;
            } else {
                self.line.extend(input.bytes);
            }
        }
        if self.eof {
            self.streams[0]
                .1
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        brush_core::bexos::set_interrupted(self.interrupted);
        self.runtime
            .as_ref()
            .unwrap()
            .block_on(tokio::task::yield_now());
        let _runtime = self.runtime.as_ref().unwrap().enter();
        if let Some(execution) = &mut self.running {
            if let Poll::Ready((shell, result)) = poll(execution.as_mut()) {
                self.shell = Some(shell);
                self.running = None;
                match result {
                    Ok(r) => {
                        self.tty.exit_code = u8::from(r.exit_code) as i32;
                        self.tty.exited = matches!(
                            r.next_control_flow,
                            brush_core::ExecutionControlFlow::ExitShell
                        );
                    }
                    Err(e) => {
                        let _ = self.streams[2].append(format!("brush: {e}\n").as_bytes());
                        self.tty.exit_code = 1;
                    }
                }
                if self.interrupted {
                    self.tty.exit_code = 130;
                    if let Some(s) = &mut self.shell {
                        s.set_last_exit_status(130);
                    }
                    self.interrupted = false;
                }
                if !self.tty.exited {
                    let _ = self.streams[1].append(b"brush$ ");
                }
            }
        }
        if self.running.is_none()
            && !self.line.is_empty()
            && (self.line.contains(&b'\n') || self.eof)
        {
            if !self.eof {
                let text = std::str::from_utf8(&self.line)
                    .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
                if self
                    .shell
                    .as_ref()
                    .unwrap()
                    .parse_string(text)
                    .as_ref()
                    .is_err_and(|e| brush_core::bexos::incomplete(e))
                {
                    // Keep the complete source until Brush can parse it. No effects
                    // have run, so this input remains checkpointable.
                    return Ok(());
                }
            }
            let command = String::from_utf8(std::mem::take(&mut self.line))
                .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
            let mut shell = self.shell.take().unwrap();
            self.running = Some(Box::pin(async move {
                let params = shell.default_exec_params();
                let result = shell
                    .run_string(command, &"terminal".into(), &params)
                    .await
                    .map_err(|e| e.to_string());
                (shell, result)
            }));
        }
        if self.eof && self.running.is_none() && self.line.is_empty() {
            self.tty.exited = true;
        }
        self.flush();
        self.close_output_when_drained();
        Ok(())
    }
    fn flush(&self) {
        for i in 1..3 {
            let mut bytes = self.streams[i].0.lock().unwrap();
            if bytes.is_empty() {
                continue;
            }
            if let Ok(n) = kernel::socket_write(self.tty.streams[i] as u32, bytes.make_contiguous())
            {
                bytes.drain(..n as usize);
            }
        }
    }
    fn close_output_when_drained(&self) {
        if self.tty.exited
            && self.streams[1].0.lock().unwrap().is_empty()
            && self.streams[2].0.lock().unwrap().is_empty()
        {
            for i in 1..3 {
                let _ = kernel::socket_shutdown(self.tty.streams[i] as u32, false, true);
            }
        }
    }
}

pub fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().build()
}
