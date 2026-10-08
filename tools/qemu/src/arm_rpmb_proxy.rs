//! ARM boot-to-runtime RPMB relay.
//!
//! QEMU keeps both guest frontends connected to distinct host sockets. This
//! owner serializes them onto one authenticated RPMB backend: the boot UART
//! runs to protocol EOF first, then the normal-world virtio lane is admitted.
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAME_SIZE: usize = 512;
const MAX_FRAMES: usize = 8;
const TRIAL_MAGIC: &[u8; 16] = b"BEXOSRPMBTRIAL01";
const TRIAL_ACK: u8 = 0xa5;
const TRIAL_BEGIN: u8 = 1;
const TRIAL_COMMIT: u8 = 2;
const TRIAL_ROLLBACK: u8 = 3;

pub struct Proxy {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub boot_socket: PathBuf,
    pub runtime_socket: PathBuf,
}

impl Proxy {
    pub fn start(
        workdir: &Path,
        backend: PathBuf,
        rpmbd: PathBuf,
        state: PathBuf,
    ) -> Result<Self, String> {
        let boot_socket = workdir.join("br.sock");
        let runtime_socket = workdir.join("rr.sock");
        for path in [&boot_socket, &runtime_socket] {
            super::remove_stale_socket(path)?;
        }
        // Darwin's sockaddr_un path is shorter than Bazel's sandboxed
        // TEST_TMPDIR. Bind a relative name while the process is still
        // single-threaded; the socket inode still lives in the owned workdir.
        let boot = bind_in(workdir, "br.sock")
            .map_err(|e| format!("bind boot RPMB relay {}: {e}", boot_socket.display()))?;
        let runtime = bind_in(workdir, "rr.sock")
            .map_err(|e| format!("bind runtime RPMB relay {}: {e}", runtime_socket.display()))?;
        boot.set_nonblocking(true).map_err(|e| e.to_string())?;
        runtime.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        let trial_workdir = workdir.to_path_buf();
        let task = thread::spawn(move || {
            let result = (|| {
                // rpmbd removes its socket when an idle client never arrives.
                // Establish ownership before slow TCG boot waits for both QEMU
                // frontends, then keep all bytes fenced behind their accepts.
                let backend_stream = connect(&backend, &task_stop)?;
                // Accept both frontends before servicing either one. The
                // runtime virtio port is consequently open before its driver
                // enumerates, while all bytes remain fenced behind boot EOF.
                let boot = accept(&boot, &task_stop, "boot")?;
                let runtime = accept(&runtime, &task_stop, "runtime")?;
                bridge(boot, backend_stream, &task_stop)?;
                if task_stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                runtime_proxy(
                    runtime,
                    TrialController::new(backend, rpmbd, state, trial_workdir),
                    &task_stop,
                )
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

struct Backend {
    socket: PathBuf,
    state: PathBuf,
    child: Option<Child>,
}

struct TrialController {
    base_state: PathBuf,
    rpmbd: PathBuf,
    workdir: PathBuf,
    active: Option<Backend>,
    previous: Option<Backend>,
    serial: u64,
}

impl TrialController {
    fn new(backend: PathBuf, rpmbd: PathBuf, state: PathBuf, workdir: PathBuf) -> Self {
        Self {
            base_state: state.clone(),
            rpmbd,
            workdir,
            active: Some(Backend {
                socket: backend,
                state,
                child: None,
            }),
            previous: None,
            serial: 0,
        }
    }

    fn socket(&self) -> Result<&Path, String> {
        self.active
            .as_ref()
            .map(|backend| backend.socket.as_path())
            .ok_or_else(|| "ARM RPMB relay has no active backend".into())
    }

    fn begin(&mut self, stop: &AtomicBool) -> Result<(), String> {
        if self.previous.is_some() {
            return Err("ARM RPMB trial is already active".into());
        }
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| "ARM RPMB relay has no active backend".to_string())?;
        sync_file(&active.state)?;
        self.serial = self.serial.wrapping_add(1).max(1);
        let state = self.workdir.join(format!("rpmb-trial-{}.img", self.serial));
        let socket = self.workdir.join(format!("rt{}.sock", self.serial));
        std::fs::copy(&active.state, &state).map_err(|error| {
            format!(
                "clone RPMB trial {} from {}: {error}",
                state.display(),
                active.state.display()
            )
        })?;
        sync_file(&state)?;
        let child = match spawn_backend(&self.rpmbd, &state, &socket, &self.workdir, stop) {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&state);
                return Err(error);
            }
        };
        self.previous = self.active.take();
        self.active = Some(Backend {
            socket,
            state,
            child: Some(child),
        });
        Ok(())
    }

    fn commit(&mut self) -> Result<(), String> {
        if self.previous.is_none() {
            return Err("ARM RPMB commit without an active trial".into());
        }
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| "ARM RPMB relay has no active backend".to_string())?;
        sync_file(&active.state)?;
        publish_state(&active.state, &self.base_state)?;
        if let Some(previous) = self.previous.take() {
            cleanup_backend(previous, &self.base_state);
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), String> {
        let previous = self
            .previous
            .take()
            .ok_or_else(|| "ARM RPMB rollback without an active trial".to_string())?;
        let trial = self
            .active
            .replace(previous)
            .ok_or_else(|| "ARM RPMB relay has no active backend".to_string())?;
        cleanup_backend(trial, &self.base_state);
        Ok(())
    }
}

impl Drop for TrialController {
    fn drop(&mut self) {
        if let Some(active) = self.active.take() {
            cleanup_backend(active, &self.base_state);
        }
        if let Some(previous) = self.previous.take() {
            cleanup_backend(previous, &self.base_state);
        }
    }
}

fn cleanup_backend(mut backend: Backend, base_state: &Path) {
    if let Some(mut child) = backend.child.take() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&backend.socket);
        if backend.state != base_state {
            let _ = std::fs::remove_file(&backend.state);
        }
    }
}

fn sync_file(path: &Path) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("sync RPMB state {}: {error}", path.display()))
}

fn publish_state(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("RPMB state has no parent: {}", destination.display()))?;
    let temporary = parent.join(format!(".rpmb-commit-{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    std::fs::copy(source, &temporary).map_err(|error| {
        format!(
            "stage committed RPMB state {}: {error}",
            destination.display()
        )
    })?;
    sync_file(&temporary)?;
    std::fs::rename(&temporary, destination).map_err(|error| {
        format!(
            "publish committed RPMB state {}: {error}",
            destination.display()
        )
    })?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("sync RPMB state directory {}: {error}", parent.display()))
}

fn spawn_backend(
    rpmbd: &Path,
    state: &Path,
    socket: &Path,
    workdir: &Path,
    stop: &AtomicBool,
) -> Result<Child, String> {
    let _ = std::fs::remove_file(socket);
    let socket_name = socket
        .file_name()
        .ok_or_else(|| format!("RPMB trial socket has no file name: {}", socket.display()))?;
    let mut child = Command::new(rpmbd)
        .current_dir(workdir)
        .arg("--dev")
        .arg(state)
        .arg("--sock")
        .arg(socket_name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("spawn trial RPMB backend: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
        if socket.exists() {
            return Ok(child);
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("poll trial RPMB backend: {error}"))?
        {
            return Err(format!("trial RPMB backend exited before ready: {status}"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("trial RPMB backend did not become ready".into())
}

fn runtime_proxy(
    mut frontend: UnixStream,
    mut controller: TrialController,
    stop: &AtomicBool,
) -> Result<(), String> {
    configure_read_polling(&frontend, "runtime frontend")?;
    let mut backend: Option<UnixStream> = None;
    let done = AtomicBool::new(false);
    loop {
        let mut header = [0u8; 4];
        if !read_exact_polling(&mut frontend, &mut header, stop, true)? {
            return Ok(());
        }
        let read = u16::from_le_bytes(header[..2].try_into().unwrap()) as usize;
        let written = u16::from_le_bytes(header[2..].try_into().unwrap()) as usize;
        if !(1..=MAX_FRAMES).contains(&read) || !(1..=MAX_FRAMES).contains(&written) {
            return Err(format!(
                "invalid runtime RPMB exchange read={read} written={written}"
            ));
        }
        let mut frames = vec![0u8; written * FRAME_SIZE];
        read_exact_polling(&mut frontend, &mut frames, stop, false)?;
        if let Some(action) = trial_action(read, written, &frames) {
            backend = None;
            let result = match action {
                TRIAL_BEGIN => controller.begin(stop),
                TRIAL_COMMIT => controller.commit(),
                TRIAL_ROLLBACK => controller.rollback(),
                _ => unreachable!(),
            };
            if let Err(error) = &result {
                eprintln!("ARM RPMB trial control failed: {error}");
            }
            let response = trial_response(action, result.is_ok());
            write_all_polling(&mut frontend, &response, stop, &done)?;
            continue;
        }
        if backend.is_none() {
            let stream = connect(controller.socket()?, stop)?;
            configure_read_polling(&stream, "runtime backend")?;
            backend = Some(stream);
        }
        let stream = backend.as_mut().unwrap();
        write_all_polling(stream, &header, stop, &done)?;
        write_all_polling(stream, &frames, stop, &done)?;
        let mut response = vec![0u8; read * FRAME_SIZE];
        read_exact_polling(stream, &mut response, stop, false)?;
        write_all_polling(&mut frontend, &response, stop, &done)?;
    }
}

fn trial_action(read: usize, written: usize, frames: &[u8]) -> Option<u8> {
    if read != 1
        || written != 1
        || frames.len() != FRAME_SIZE
        || frames[..TRIAL_MAGIC.len()] != *TRIAL_MAGIC
        || frames[TRIAL_MAGIC.len() + 1..]
            .iter()
            .any(|byte| *byte != 0)
    {
        return None;
    }
    matches!(
        frames[TRIAL_MAGIC.len()],
        TRIAL_BEGIN | TRIAL_COMMIT | TRIAL_ROLLBACK
    )
    .then_some(frames[TRIAL_MAGIC.len()])
}

fn trial_response(action: u8, success: bool) -> [u8; FRAME_SIZE] {
    let mut response = [0u8; FRAME_SIZE];
    response[..TRIAL_MAGIC.len()].copy_from_slice(TRIAL_MAGIC);
    response[TRIAL_MAGIC.len()] = action;
    response[TRIAL_MAGIC.len() + 1] = if success { TRIAL_ACK } else { 0 };
    response
}

fn read_exact_polling(
    reader: &mut UnixStream,
    output: &mut [u8],
    stop: &AtomicBool,
    eof_before_data: bool,
) -> Result<bool, String> {
    let mut received = 0;
    while received < output.len() && !stop.load(Ordering::Acquire) {
        match reader.read(&mut output[received..]) {
            Ok(0) if received == 0 && eof_before_data => return Ok(false),
            Ok(0) => return Err("read RPMB relay: truncated exchange".into()),
            Ok(count) => received += count,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) if error.kind() == ErrorKind::InvalidInput => {
                return Err("read RPMB relay: peer disappeared".into());
            }
            Err(error) => return Err(format!("read RPMB relay: {error}")),
        }
    }
    if received == output.len() {
        Ok(true)
    } else {
        Err("read RPMB relay: canceled".into())
    }
}

fn bind_in(workdir: &Path, name: &str) -> std::io::Result<UnixListener> {
    super::with_socket_cwd(workdir, || UnixListener::bind(name))
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

pub(crate) fn connect(path: &Path, stop: &AtomicBool) -> Result<UnixStream, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("RPMB backend connection canceled".into());
        }
        match super::connect_unix(path) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("connect RPMB backend: {error}")),
        }
    }
}

pub(crate) fn bridge(
    frontend: UnixStream,
    backend: UnixStream,
    stop: &AtomicBool,
) -> Result<(), String> {
    configure_read_polling(&frontend, "frontend")?;
    configure_read_polling(&backend, "backend")?;
    let mut frontend_reader = frontend
        .try_clone()
        .map_err(|e| format!("clone frontend RPMB stream: {e}"))?;
    let mut backend_writer = backend
        .try_clone()
        .map_err(|e| format!("clone backend RPMB stream: {e}"))?;
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

fn configure_read_polling(stream: &UnixStream, name: &str) -> Result<(), String> {
    match stream.set_read_timeout(Some(Duration::from_millis(100))) {
        Ok(()) => Ok(()),
        // Darwin rejects SO_RCVTIMEO for the QEMU chardev socket. Nonblocking
        // mode provides the same bounded cancellation point; writes below
        // explicitly retry WouldBlock because cloned streams share this flag.
        Err(error) if error.kind() == ErrorKind::InvalidInput => stream
            .set_nonblocking(true)
            .map_err(|e| format!("set {name} RPMB stream nonblocking: {e}")),
        Err(error) => Err(format!("set {name} RPMB read timeout: {error}")),
    }
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
            Ok(count) => write_all_polling(writer, &buffer[..count], stop, done)?,
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

fn write_all_polling(
    writer: &mut UnixStream,
    bytes: &[u8],
    stop: &AtomicBool,
    done: &AtomicBool,
) -> Result<(), String> {
    let mut written = 0;
    while written < bytes.len() && !stop.load(Ordering::Acquire) && !done.load(Ordering::Acquire) {
        match writer.write(&bytes[written..]) {
            Ok(0) => return Err("write RPMB relay: peer closed".into()),
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(format!("write RPMB relay: {error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_bind_survives_long_test_tmpdir() {
        let directory = std::env::temp_dir().join(format!(
            "bexos-arm-rpmb-{}-{}",
            std::process::id(),
            "x".repeat(100)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let listener = bind_in(&directory, "r.sock").unwrap();
        assert!(directory.join("r.sock").exists());
        let stream = super::super::connect_unix(&directory.join("r.sock")).unwrap();
        drop(stream);
        drop(listener);
        std::fs::remove_file(directory.join("r.sock")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn trial_control_frames_are_exact_and_acknowledged() {
        let mut request = [0u8; FRAME_SIZE];
        request[..TRIAL_MAGIC.len()].copy_from_slice(TRIAL_MAGIC);
        request[TRIAL_MAGIC.len()] = TRIAL_BEGIN;
        assert_eq!(trial_action(1, 1, &request), Some(TRIAL_BEGIN));
        assert_eq!(trial_action(2, 1, &request), None);
        request[200] = 1;
        assert_eq!(trial_action(1, 1, &request), None);
        let response = trial_response(TRIAL_COMMIT, true);
        assert_eq!(&response[..TRIAL_MAGIC.len()], TRIAL_MAGIC);
        assert_eq!(response[TRIAL_MAGIC.len()], TRIAL_COMMIT);
        assert_eq!(response[TRIAL_MAGIC.len() + 1], TRIAL_ACK);
    }

    #[test]
    fn committed_state_is_atomically_published() {
        let directory =
            std::env::temp_dir().join(format!("bexos-rpmb-publish-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source");
        let destination = directory.join("destination");
        std::fs::write(&source, b"candidate").unwrap();
        std::fs::write(&destination, b"source").unwrap();
        publish_state(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"candidate");
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(destination).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
