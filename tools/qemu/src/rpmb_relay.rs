//! Serial ownership transfer for authenticated RPMB across a firmware handoff.
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ChildStdout;
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy)]
pub struct Transfer {
    pub boot_device: &'static str,
    pub runtime_device: &'static str,
    pub attach_marker: &'static [u8],
    pub release_marker: &'static [u8],
    pub initially_attached: bool,
}

/// Tee QEMU output while changing which serial frontend owns the one RPMB
/// connection. Only permanent-owner markers can trigger either transition.
pub struct Relay {
    shutdown: UnixStream,
    thread: Option<JoinHandle<()>>,
}

impl Relay {
    pub fn start(
        mut input: impl Read + Send + 'static,
        control: PathBuf,
        rpmb: PathBuf,
        transfer: Transfer,
    ) -> Result<(ChildStdout, Self), String> {
        let (output, mut writer) = UnixStream::pair().map_err(|e| e.to_string())?;
        let shutdown = writer.try_clone().map_err(|e| e.to_string())?;
        let task = thread::spawn(move || {
            let mut attached = transfer.initially_attached;
            let mut transferred = false;
            let mut recent = Vec::new();
            let mut buffer = [0; 4096];
            while let Ok(count) = input.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                recent.extend_from_slice(&buffer[..count]);
                let result = (|| {
                    if !attached && super::contains(&recent, transfer.attach_marker) {
                        super::qmp::change_serial(&control, transfer.boot_device, Some(&rpmb))?;
                        attached = true;
                    }
                    if attached && !transferred && super::contains(&recent, transfer.release_marker)
                    {
                        super::qmp::transfer_serial(
                            &control,
                            transfer.boot_device,
                            transfer.runtime_device,
                            &rpmb,
                        )?;
                        transferred = true;
                    }
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    let _ = writeln!(writer, "panic: secure QEMU transport setup failed: {error}");
                    break;
                }
                if writer.write_all(&buffer[..count]).is_err() {
                    break;
                }
                if recent.len() > 512 {
                    recent.drain(..recent.len() - 512);
                }
            }
            let _ = writer.shutdown(Shutdown::Write);
        });
        Ok((
            ChildStdout::from(OwnedFd::from(output)),
            Self {
                shutdown,
                thread: Some(task),
            },
        ))
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.shutdown.shutdown(Shutdown::Both);
        if let Some(task) = self.thread.take() {
            let _ = task.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const UNUSED: Transfer = Transfer {
        boot_device: "boot",
        runtime_device: "runtime",
        attach_marker: b"never attach",
        release_marker: b"never release",
        initially_attached: false,
    };

    #[test]
    fn eof_does_not_wait_for_owner_drop() {
        let expected = b"guest completed\n";
        let (output, relay) = Relay::start(
            std::io::Cursor::new(expected),
            PathBuf::new(),
            PathBuf::new(),
            UNUSED,
        )
        .unwrap();
        let mut socket = UnixStream::from(OwnedFd::from(output));
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
        drop(relay);
    }

    #[test]
    fn cancellation_releases_a_backpressured_relay() {
        let (_output, relay) = Relay::start(
            std::io::Cursor::new(vec![b'x'; 1024 * 1024]),
            PathBuf::new(),
            PathBuf::new(),
            UNUSED,
        )
        .unwrap();
        drop(relay);
    }
}
