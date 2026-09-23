//! ARM boot-to-runtime RPMB relay.
//!
//! QEMU keeps both guest frontends connected to distinct host sockets. This
//! owner serializes them onto one authenticated RPMB backend: the boot UART
//! runs to protocol EOF first, then the normal-world virtio lane is admitted.
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct Proxy {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub boot_socket: PathBuf,
    pub runtime_socket: PathBuf,
}

impl Proxy {
    pub fn start(workdir: &Path, backend: PathBuf) -> Result<Self, String> {
        let boot_socket = workdir.join("bootrpmb-relay.sock");
        let runtime_socket = workdir.join("rpmb0-relay.sock");
        for path in [&boot_socket, &runtime_socket] {
            super::remove_stale_socket(path)?;
        }
        let boot =
            UnixListener::bind(&boot_socket).map_err(|e| format!("bind boot RPMB relay: {e}"))?;
        let runtime = UnixListener::bind(&runtime_socket)
            .map_err(|e| format!("bind runtime RPMB relay: {e}"))?;
        boot.set_nonblocking(true).map_err(|e| e.to_string())?;
        runtime.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        let task = thread::spawn(move || {
            let result = (|| {
                // Accept both frontends before servicing either one. The
                // runtime virtio port is consequently open before its driver
                // enumerates, while all bytes remain fenced behind boot EOF.
                let boot = accept(&boot, &task_stop, "boot")?;
                let runtime = accept(&runtime, &task_stop, "runtime")?;
                let backend_stream = connect(&backend, &task_stop)?;
                bridge(boot, backend_stream, &task_stop)?;
                if task_stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                let backend_stream = connect(&backend, &task_stop)?;
                bridge(runtime, backend_stream, &task_stop)
            })();
            if let Err(error) = result {
                if !task_stop.load(Ordering::Acquire) {
                    eprintln!("ARM RPMB relay failed: {error}");
                }
            }
        });
        Ok(Self {
            stop,
            thread: Some(task),
            boot_socket,
            runtime_socket,
        })
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.thread.take() {
            let _ = task.join();
        }
    }
}

fn accept(listener: &UnixListener, stop: &AtomicBool, name: &str) -> Result<UnixStream, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if stop.load(Ordering::Acquire) {
            return Err(format!("{name} RPMB relay canceled"));
        }
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("accept {name} RPMB frontend: {error}")),
        }
    }
}

fn connect(path: &Path, stop: &AtomicBool) -> Result<UnixStream, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("RPMB backend connection canceled".into());
        }
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("connect RPMB backend: {error}")),
        }
    }
}

fn bridge(frontend: UnixStream, backend: UnixStream, stop: &AtomicBool) -> Result<(), String> {
    frontend
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| e.to_string())?;
    backend
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| e.to_string())?;
    let mut frontend_reader = frontend.try_clone().map_err(|e| e.to_string())?;
    let mut backend_writer = backend.try_clone().map_err(|e| e.to_string())?;
    let direction_done = Arc::new(AtomicBool::new(false));
    let writer_done = direction_done.clone();
    let writer = thread::spawn(move || {
        let _ = pump(&mut frontend_reader, &mut backend_writer, &writer_done);
        writer_done.store(true, Ordering::Release);
        let _ = backend_writer.shutdown(Shutdown::Write);
    });
    let mut backend_reader = backend;
    let mut frontend_writer = frontend;
    pump_until(
        &mut backend_reader,
        &mut frontend_writer,
        stop,
        &direction_done,
    )?;
    direction_done.store(true, Ordering::Release);
    let _ = frontend_writer.shutdown(Shutdown::Both);
    let _ = backend_reader.shutdown(Shutdown::Both);
    let _ = writer.join();
    Ok(())
}

fn pump(reader: &mut UnixStream, writer: &mut UnixStream, done: &AtomicBool) -> Result<(), String> {
    let never_stop = AtomicBool::new(false);
    pump_until(reader, writer, &never_stop, done)
}

fn pump_until(
    reader: &mut UnixStream,
    writer: &mut UnixStream,
    stop: &AtomicBool,
    done: &AtomicBool,
) -> Result<(), String> {
    let mut buffer = [0; 8192];
    while !stop.load(Ordering::Acquire) && !done.load(Ordering::Acquire) {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => writer
                .write_all(&buffer[..count])
                .map_err(|e| format!("write RPMB relay: {e}"))?,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            // Darwin reports EINVAL when a peer disappears while another
            // thread is shutting down the cloned UnixStream.  At that point
            // the direction is at EOF for relay purposes.
            Err(error) if error.kind() == ErrorKind::InvalidInput => break,
            Err(error) => return Err(format!("read RPMB relay: {error}")),
        }
    }
    Ok(())
}
