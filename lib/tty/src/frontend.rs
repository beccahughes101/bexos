use alloc::{vec, vec::Vec};
use bexos_userspace::{Channel, Memory, Rpc, Socket};
use kernel_fidl::Status;
use tty_fidl::*;
/// Live handles are closed explicitly, since migration temporarily shares their identities.
#[derive(Clone, Copy)]
pub struct Frontend {
    pub control: u64,
    pub stdin: u64,
    pub stdout: u64,
    pub stderr: u64,
}
pub fn call<Q: FidlEncode, R: for<'a> FidlDecode<'a>>(
    channel: u64,
    ordinal: u64,
    q: &Q,
) -> Result<R, Status> {
    let mut bytes = vec![0; 4096];
    let mut hs = [HandleRef { raw: 0 }; 4];
    let n = q
        .encode(&mut bytes, &mut hs)
        .map_err(|_| Status::ErrInvalidArgs)?;
    let r = Rpc(Channel(channel))
        .call_raw_with_timeout(
            ordinal,
            &bytes[..n.bytes],
            &hs[..n.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
            true,
            2,
        )
        .map_err(|_| Status::ErrTimedOut)?;
    let handles: Vec<_> = r.handles.iter().map(|h| HandleRef { raw: *h }).collect();
    match R::decode(&r.bytes, &handles) {
        Ok(v) => Ok(v),
        Err(_) => {
            for h in r.handles {
                let _ = Memory::close(h);
            }
            Err(Status::ErrInvalidArgs)
        }
    }
}
fn checked(s: Status) -> Result<(), Status> {
    if s == Status::Ok { Ok(()) } else { Err(s) }
}
impl Frontend {
    pub fn take(control: u64) -> Result<Self, Status> {
        let r: PtySessionTakeIoStreamsResponse =
            call(control, 1, &PtySessionTakeIoStreamsRequest {})?;
        let streams = [r.stdin_stream.raw, r.stdout_stream.raw, r.stderr_stream.raw];
        let invalid = streams.contains(&0)
            || streams
                .iter()
                .enumerate()
                .any(|(i, h)| streams[..i].contains(h));
        if r.status != Status::Ok || invalid {
            for (i, h) in streams.into_iter().enumerate() {
                if h != 0 && !streams[..i].contains(&h) {
                    let _ = Memory::close(h);
                }
            }
            return Err(if r.status == Status::Ok {
                Status::ErrInvalidArgs
            } else {
                r.status
            });
        }
        Ok(Self {
            control,
            stdin: r.stdin_stream.raw,
            stdout: r.stdout_stream.raw,
            stderr: r.stderr_stream.raw,
        })
    }
    pub fn resize(&self, size: WindowSize) -> Result<(), Status> {
        let r: PtySessionSetWindowSizeResponse =
            call(self.control, 2, &PtySessionSetWindowSizeRequest { size })?;
        checked(r.status)
    }
    pub fn mode(&self, flags: TerminalMode) -> Result<(), Status> {
        let r: PtySessionSetTerminalModeResponse =
            call(self.control, 3, &PtySessionSetTerminalModeRequest { flags })?;
        checked(r.status)
    }
    pub fn signal(&self, signal: Signal) -> Result<(), Status> {
        let r: PtySessionSendSignalResponse =
            call(self.control, 4, &PtySessionSendSignalRequest { signal })?;
        checked(r.status)
    }
    pub fn status(&self) -> Result<(bool, i32), Status> {
        let r: PtySessionGetStatusResponse = call(self.control, 5, &PtySessionGetStatusRequest {})?;
        checked(r.status)?;
        Ok((r.exited, r.exit_code))
    }
    pub fn write(&self, input: &[u8]) -> Result<usize, Status> {
        if input.is_empty() {
            return Ok(0);
        }
        match Socket(self.stdin).write(input) {
            Ok(n) => Ok(n as usize),
            Err(Status::ErrTimedOut | Status::ErrNoMemory) => Ok(0),
            Err(e) => Err(e),
        }
    }
    pub fn eof(&self) -> Result<(), Status> {
        Socket(self.stdin).shutdown(false, true)
    }
    pub fn read(handle: u64, max: u32) -> Result<Vec<u8>, Status> {
        match Socket(handle).read(max) {
            Ok(v) => Ok(v),
            Err(Status::ErrTimedOut | Status::ErrPeerClosed) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }
    pub fn drained(&self) -> Result<bool, Status> {
        Ok(Socket(self.stdout).info()?.readable_bytes == 0
            && Socket(self.stderr).info()?.readable_bytes == 0)
    }
    pub fn close(&self) {
        let _: Result<PtySessionCloseResponse, _> =
            call(self.control, 6, &PtySessionCloseRequest {});
        self.close_handles();
    }
    pub fn close_handles(&self) {
        for h in self.handles() {
            if h != 0 {
                let _ = Memory::close(h);
            }
        }
    }
    pub fn handles(&self) -> [u64; 4] {
        [self.control, self.stdin, self.stdout, self.stderr]
    }
}
