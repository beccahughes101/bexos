//! Unix terminal ownership. Every acquired terminal mode has an RAII restore.
use crate::{
    args::ShellArgs,
    commands::{RemoteExit, Result},
};
use bexos_debug_client::UnixDebugClient;
use bexos_debug_wire::{SHELL_CHUNK, ShellRequest};
use std::{
    fs::OpenOptions,
    io::{self, IsTerminal, Read, Write},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    time::Duration,
};
struct Mode {
    fd: RawFd,
    original: libc::termios,
}
impl Mode {
    fn set(fd: RawFd, raw: bool) -> io::Result<Self> {
        unsafe {
            let mut original = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut original) != 0 {
                return Err(io::Error::last_os_error());
            }
            let mut mode = original;
            if raw {
                libc::cfmakeraw(&mut mode);
            } else {
                mode.c_lflag &= !(libc::ECHO as libc::tcflag_t);
            }
            if libc::tcsetattr(fd, libc::TCSANOW, &mode) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { fd, original })
        }
    }
}
impl Drop for Mode {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}
fn size(fd: RawFd) -> (u16, u16) {
    unsafe {
        let mut w: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut w) == 0 && w.ws_row > 0 && w.ws_col > 0 {
            (w.ws_row, w.ws_col)
        } else {
            (24, 80)
        }
    }
}
fn password_line(input: &mut impl Read) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut overflow = false;
    loop {
        let mut b = [0];
        if input.read(&mut b)? == 0 || b[0] == b'\n' {
            break;
        }
        if bytes.len() >= 257 {
            // Consume the complete credential line without retaining it, so
            // the remainder cannot become terminal input after an error.
            overflow = true;
        } else {
            bytes.push(b[0]);
        }
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if overflow || bytes.len() > 256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "password exceeds 256 bytes",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "password must be UTF-8"))
}
fn password(args: &mut ShellArgs) -> Result<String> {
    if args.system {
        return Ok(String::new());
    }
    if let Some(p) = args.password.take() {
        return Ok(p);
    }
    if args.password_stdin {
        // Stdin's global buffer may read ahead into terminal data, which would
        // then be invisible to poll(2). Read the credential from an unbuffered
        // duplicate, leaving subsequent bytes on the original descriptor.
        let fd = unsafe { libc::dup(0) };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut input = unsafe { std::fs::File::from_raw_fd(fd) };
        return Ok(password_line(&mut input)?);
    }
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| format!("open password prompt: {e}; use --password-stdin for automation"))?;
    tty.write_all(b"Password: ")?;
    tty.flush()?;
    let guard = Mode::set(tty.as_raw_fd(), false)?;
    let result = password_line(&mut tty);
    drop(guard);
    tty.write_all(b"\n")?;
    Ok(result?)
}
extern "C" fn interrupted(signal: libc::c_int) {
    INTERRUPTED.store(signal, std::sync::atomic::Ordering::Relaxed);
}
static INTERRUPTED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
struct Signals(Vec<(libc::c_int, libc::sigaction)>);
impl Signals {
    fn install() -> io::Result<Self> {
        INTERRUPTED.store(0, std::sync::atomic::Ordering::Relaxed);
        let mut out = Self(Vec::new());
        for n in [
            libc::SIGTERM,
            libc::SIGHUP,
            libc::SIGINT,
            libc::SIGTSTP,
            libc::SIGQUIT,
        ] {
            unsafe {
                let mut old = std::mem::zeroed();
                let mut new: libc::sigaction = std::mem::zeroed();
                new.sa_sigaction = interrupted as *const () as usize;
                libc::sigemptyset(&mut new.sa_mask);
                if libc::sigaction(n, &new, &mut old) != 0 {
                    return Err(io::Error::last_os_error());
                }
                out.0.push((n, old));
            }
        }
        Ok(out)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for (n, old) in &self.0 {
            unsafe {
                libc::sigaction(*n, old, std::ptr::null_mut());
            }
        }
    }
}
pub fn run(c: &mut UnixDebugClient, mut args: ShellArgs) -> Result {
    // Install before disabling password echo as well as before raw terminal mode.
    let _signals = Signals::install()?;
    let password = password(&mut args)?;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let (rows, cols) = size(0);
    let mut request = ShellRequest {
        system: args.system,
        uid: args.uid.unwrap_or(0),
        user: args.user.take().unwrap_or_default(),
        password,
        rows,
        cols,
        ..Default::default()
    };
    let opened = c.open_shell(&request);
    request.password.clear();
    drop(request);
    let opened = opened?;
    let id = opened.session_id;
    let result = (|| -> Result {
        let _mode = if interactive {
            Some(Mode::set(0, true)?)
        } else {
            None
        };
        if !interactive {
            c.set_shell_mode(id, 0)?;
        }
        let mut dimensions = (rows, cols);
        let mut input = Vec::new();
        let mut eof = false;
        loop {
            let signal = INTERRUPTED.swap(0, std::sync::atomic::Ordering::Relaxed);
            if signal == libc::SIGINT {
                c.signal_shell(id, 1)?;
            } else if signal != 0 {
                let _ = c.signal_shell(id, if signal == libc::SIGTSTP { 3 } else { 2 });
                return Err(Box::new(RemoteExit(128 + signal)));
            }
            if input.is_empty() && !eof {
                let mut p = libc::pollfd {
                    fd: 0,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut p, 1, 20) };
                if ready < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::Interrupted {
                        return Err(e.into());
                    }
                } else if ready > 0 {
                    let mut chunk = [0; SHELL_CHUNK];
                    let n = io::stdin().read(&mut chunk)?;
                    if n == 0 {
                        eof = true;
                    } else {
                        if interactive && chunk[..n].contains(&29) {
                            return Ok(());
                        }
                        input.extend_from_slice(&chunk[..n]);
                    }
                }
            }
            let r = c.exchange_shell(id, &input, eof)?;
            input.drain(..r.consumed as usize);
            io::stdout().write_all(&r.stdout)?;
            io::stdout().flush()?;
            io::stderr().write_all(&r.stderr)?;
            if r.exited {
                return if r.exit_code == 0 {
                    Ok(())
                } else {
                    Err(Box::new(RemoteExit(r.exit_code)))
                };
            }
            if interactive {
                let new = size(0);
                if dimensions != new {
                    c.resize_shell(id, new.0, new.1)?;
                    dimensions = new;
                }
            }
            if eof || !input.is_empty() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    })();
    // Exited sessions are already closed remotely. Cleanup failure must not hide the original error.
    let _ = c.close_shell(id);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_mode_restores_on_error() {
        unsafe {
            let (mut master, mut slave) = (-1, -1);
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                0
            );
            let mut before: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave, &mut before), 0);
            let result = (|| -> io::Result<()> {
                let _guard = Mode::set(slave, true)?;
                let mut raw = std::mem::zeroed();
                assert_eq!(libc::tcgetattr(slave, &mut raw), 0);
                assert_eq!(raw.c_lflag & libc::ICANON, 0);
                Err(io::Error::other("simulated disconnect"))
            })();
            assert!(result.is_err());
            let mut after = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave, &mut after), 0);
            // BSD sets PENDIN when canonical input is restored: it asks the
            // kernel to reprocess queued input and is not a persistent mode.
            assert_eq!(
                before.c_lflag & !libc::PENDIN,
                after.c_lflag & !libc::PENDIN
            );
            assert_eq!(before.c_iflag, after.c_iflag);
            assert_eq!(before.c_oflag, after.c_oflag);
            assert_eq!(before.c_cflag, after.c_cflag);
            assert_eq!(before.c_cc, after.c_cc);
            libc::close(master);
            libc::close(slave);
        }
    }
    #[test]
    fn password_stdin_consumes_only_one_line() {
        let mut input = io::Cursor::new(b"secret\nterminal bytes".to_vec());
        assert_eq!(password_line(&mut input).unwrap(), "secret");
        let mut remaining = Vec::new();
        input.read_to_end(&mut remaining).unwrap();
        assert_eq!(remaining, b"terminal bytes");
    }
    #[test]
    fn oversized_password_is_drained_without_consuming_terminal_input() {
        let mut bytes = vec![b'x'; 1024];
        bytes.extend_from_slice(b"\nterminal bytes");
        let mut input = io::Cursor::new(bytes);
        assert!(password_line(&mut input).is_err());
        let mut remaining = Vec::new();
        input.read_to_end(&mut remaining).unwrap();
        assert_eq!(remaining, b"terminal bytes");
    }
    #[test]
    fn maximum_password_accepts_crlf() {
        let mut bytes = vec![b'x'; 256];
        bytes.extend_from_slice(b"\r\n");
        assert_eq!(
            password_line(&mut io::Cursor::new(bytes)).unwrap().len(),
            256
        );
    }
}
