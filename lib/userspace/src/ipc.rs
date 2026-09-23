use alloc::vec;
use alloc::vec::Vec;
use kernel_fidl::{FidlDecode, FidlEncode, FidlWireError, HandleRef, Status};
pub const MAX_MESSAGE: usize = 65536;
pub const DEADLINE_SECONDS: u64 = 60;
pub struct Message {
    pub bytes: Vec<u8>,
    pub handles: Vec<u64>,
}
#[derive(Clone, Copy)]
pub struct Channel(pub u64);
#[derive(Clone, Copy)]
pub struct Socket(pub u64);
#[derive(Clone, Copy)]
pub struct KernelTransport(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketInfo {
    pub readable_bytes: u64,
    pub local_read_closed: bool,
    pub local_write_closed: bool,
    pub peer_read_closed: bool,
    pub peer_write_closed: bool,
}
impl kernel_fidl::FidlTransport for KernelTransport {
    fn call(
        &mut self,
        ordinal: u64,
        request: &[u8],
        handles: &[HandleRef],
        response: &mut [u8],
        out: &mut [HandleRef],
    ) -> Result<kernel_fidl::EncodeResult, FidlWireError> {
        let hs: Vec<_> = handles.iter().map(|h| h.raw).collect();
        let mut oh = vec![0; out.len()];
        let (bytes, n) = crate::syscall::fidl(self.0, ordinal, request, &hs, response, &mut oh)
            .map_err(|_| FidlWireError::Transport)?;
        for i in 0..n {
            out[i].raw = oh[i];
        }
        Ok(kernel_fidl::EncodeResult { bytes, handles: n })
    }
}
pub fn kernel_call<Q: FidlEncode, R: for<'a> FidlDecode<'a>>(
    protocol: u64,
    name: &str,
    methods: &[kernel_fidl::MethodBinding],
    req: &Q,
) -> Result<R, Status> {
    let ordinal = methods
        .iter()
        .find(|m| m.name == name)
        .ok_or(Status::ErrInvalidArgs)?
        .ordinal;
    let mut bytes = vec![0; MAX_MESSAGE];
    let mut handles = [HandleRef { raw: 0 }; 64];
    let encoded = req
        .encode(&mut bytes, &mut handles)
        .map_err(|_| Status::ErrInvalidArgs)?;
    let hs: Vec<_> = handles[..encoded.handles].iter().map(|h| h.raw).collect();
    let mut out = vec![0; MAX_MESSAGE];
    let mut oh = [0; 64];
    let (n, k) = crate::syscall::fidl(
        protocol,
        ordinal,
        &bytes[..encoded.bytes],
        &hs,
        &mut out,
        &mut oh,
    )
    .map_err(status)?;
    let refs: Vec<_> = oh[..k].iter().map(|h| HandleRef { raw: *h }).collect();
    R::decode(&out[..n], &refs).map_err(|_| Status::ErrInvalidArgs)
}
/// Call a known bounded protocol without allocating temporary IPC storage.
/// Callers must size the response for the entire response: a mutating syscall
/// must never be retried merely because its result buffer was too small.
pub fn kernel_call_buffered<Q: FidlEncode, R: for<'a> FidlDecode<'a>>(
    protocol: u64,
    name: &str,
    methods: &[kernel_fidl::MethodBinding],
    request: &Q,
    bytes: &mut [u8],
    out: &mut [u8],
) -> Result<R, Status> {
    let ordinal = methods
        .iter()
        .find(|method| method.name == name)
        .ok_or(Status::ErrInvalidArgs)?
        .ordinal;
    let mut refs = [HandleRef { raw: 0 }; 64];
    let encoded = request
        .encode(bytes, &mut refs)
        .map_err(|error| match error {
            FidlWireError::BufferTooSmall => Status::ErrBufferTooSmall,
            _ => Status::ErrInvalidArgs,
        })?;
    let mut handles = [0; 64];
    for (handle, reference) in handles.iter_mut().zip(&refs).take(encoded.handles) {
        *handle = reference.raw;
    }
    let mut received = [0; 64];
    let (n, count) = crate::syscall::fidl(
        protocol,
        ordinal,
        &bytes[..encoded.bytes],
        &handles[..encoded.handles],
        out,
        &mut received,
    )
    .map_err(status)?;
    for (reference, handle) in refs.iter_mut().zip(received).take(count) {
        reference.raw = handle;
    }
    R::decode(&out[..n], &refs[..count]).map_err(|_| Status::ErrInvalidArgs)
}
pub fn status(code: i32) -> Status {
    match code {
        -1 => Status::ErrInvalidHandle,
        -2 => Status::ErrAccessDenied,
        -3 => Status::ErrNoMemory,
        -4 => Status::ErrBufferTooSmall,
        -5 => Status::ErrPeerClosed,
        -6 => Status::ErrTimedOut,
        -7 => Status::ErrAlreadyExists,
        -8 => Status::ErrInvalidArgs,
        -9 => Status::ErrResourceExhausted,
        _ => Status::ErrInvalidArgs,
    }
}
pub fn check(status: Status) -> Result<(), Status> {
    if status == Status::Ok {
        Ok(())
    } else {
        Err(status)
    }
}
impl Channel {
    pub fn pair() -> Result<(Self, Self), Status> {
        let r: kernel_fidl::ChannelControlCreateChannelResponse = kernel_call(
            1,
            "CreateChannel",
            kernel_fidl::CHANNEL_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::ChannelControlCreateChannelRequest {},
        )?;
        check(r.status)?;
        Ok((Self(r.local_endpoint.raw), Self(r.remote_endpoint.raw)))
    }
    pub fn send(&self, bytes: &[u8], handles: &[u64]) -> Result<(), Status> {
        if handles.len() > 64 || bytes.len() > MAX_MESSAGE - 128 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut refs = [HandleRef { raw: 0 }; 64];
        for (reference, handle) in refs.iter_mut().zip(handles) {
            reference.raw = *handle;
        }
        let request = kernel_fidl::ChannelControlWriteMessageRequest {
            channel: HandleRef { raw: self.0 },
            data: bytes,
            handles: &refs[..handles.len()],
        };
        // Request size is bounded before dispatch, so large messages fall back
        // without ever executing the write twice.
        let mut small = [0; 4096];
        let mut large;
        let storage: &mut [u8] = if bytes.len() + handles.len() * 8 + 64 <= small.len() {
            &mut small
        } else {
            large = vec![0; MAX_MESSAGE];
            &mut large
        };
        let r: kernel_fidl::ChannelControlWriteMessageResponse = kernel_call_buffered(
            1,
            "WriteMessage",
            kernel_fidl::CHANNEL_CONTROL_PUBLIC_METHODS,
            &request,
            storage,
            &mut [0; 32],
        )?;
        check(r.status)
    }
    pub fn try_recv(&self) -> Result<Message, Status> {
        // Most service-loop polls find no message or a small control request.
        // A too-small read leaves the queued message and its handles untouched.
        let mut small = [0; 1024];
        match self.try_recv_into(&mut small, 512, 32) {
            Err(Status::ErrBufferTooSmall) => {
                let mut out = vec![0; MAX_MESSAGE];
                self.try_recv_into(&mut out, MAX_MESSAGE - 128, 32)
            }
            result => result,
        }
    }

    /// Capacity failures leave both the message and transferred handles queued.
    pub fn try_recv_bounded(
        &self,
        max_bytes: usize,
        max_handles: usize,
    ) -> Result<Message, Status> {
        if max_bytes > MAX_MESSAGE - 128 || max_handles > 32 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut out = vec![0; MAX_MESSAGE];
        self.try_recv_into(&mut out, max_bytes, max_handles)
    }

    fn try_recv_into(
        &self,
        out: &mut [u8],
        max_bytes: usize,
        max_handles: usize,
    ) -> Result<Message, Status> {
        let mut req = [0; 32];
        let mut hs = [HandleRef { raw: 0 }; 1];
        let encoded = kernel_fidl::ChannelControlReadMessageRequest {
            channel: HandleRef { raw: self.0 },
            max_bytes: max_bytes as u32,
            max_handles: max_handles as u32,
        }
        .encode(&mut req, &mut hs)
        .map_err(|_| Status::ErrInvalidArgs)?;
        let ordinal = kernel_fidl::CHANNEL_CONTROL_PUBLIC_METHODS
            .iter()
            .find(|m| m.name == "ReadMessage")
            .unwrap()
            .ordinal;
        let mut handles = [0; 64];
        let (n, k) = crate::syscall::fidl(
            1,
            ordinal,
            &req[..encoded.bytes],
            &[self.0],
            out,
            &mut handles,
        )
        .map_err(status)?;
        let refs: Vec<_> = handles[..k].iter().map(|h| HandleRef { raw: *h }).collect();
        let r = kernel_fidl::ChannelControlReadMessageResponse::decode(&out[..n], &refs)
            .map_err(|_| Status::ErrInvalidArgs)?;
        check(r.status)?;
        Ok(Message {
            bytes: r.data.to_vec(),
            handles: r.handles.iter().map(|h| h.raw).collect(),
        })
    }
    pub fn recv(&self) -> Result<Message, Status> {
        self.recv_with_timeout(DEADLINE_SECONDS)
    }

    pub fn recv_with_timeout(&self, timeout_seconds: u64) -> Result<Message, Status> {
        let start = crate::syscall::ticks();
        loop {
            match self.try_recv() {
                Err(Status::ErrTimedOut) => {}
                r => return r,
            }
            if crate::syscall::ticks().wrapping_sub(start)
                > crate::syscall::frequency().saturating_mul(timeout_seconds)
            {
                return Err(Status::ErrTimedOut);
            }
            crate::yield_now();
        }
    }

    /// Waits until a message arrives or the channel reports a terminal error.
    ///
    /// Use this only for protocol messages whose absence leaves the process
    /// with no useful work to do, such as its initial startup envelope.
    pub fn recv_blocking(&self) -> Result<Message, Status> {
        loop {
            match self.try_recv() {
                Err(Status::ErrTimedOut) => self.wait_readable()?,
                result => return result,
            }
        }
    }

    fn wait_readable(&self) -> Result<(), Status> {
        let items = [kernel_fidl::InlineVectorStruct1 {
            h: HandleRef { raw: self.0 },
            signals: kernel_fidl::Signals(
                kernel_fidl::Signals::READABLE.0 | kernel_fidl::Signals::PEER_CLOSED.0,
            ),
        }];
        let response: kernel_fidl::TaskControlWaitManyResponse = kernel_call(
            3,
            "WaitMany",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlWaitManyRequest {
                items: kernel_fidl::WireVector::from_slice(&items),
                deadline_nanos: -1,
            },
        )?;
        match response.status {
            Status::Ok | Status::ErrTimedOut => Ok(()),
            status => Err(status),
        }
    }
}
impl Socket {
    /// Poll or wait for byte-stream readiness using the kernel's queue state.
    pub fn wait_io(&self, write: bool, deadline_nanos: i64) -> Result<bool, Status> {
        let items = [kernel_fidl::InlineVectorStruct1 {
            h: HandleRef { raw: self.0 },
            signals: kernel_fidl::Signals(
                (if write {
                    kernel_fidl::Signals::WRITABLE.0
                } else {
                    kernel_fidl::Signals::READABLE.0
                }) | kernel_fidl::Signals::PEER_CLOSED.0,
            ),
        }];
        let response: kernel_fidl::TaskControlWaitManyResponse = kernel_call(
            3,
            "WaitMany",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlWaitManyRequest {
                items: kernel_fidl::WireVector::from_slice(&items),
                deadline_nanos,
            },
        )?;
        match response.status {
            Status::Ok => Ok(true),
            Status::ErrTimedOut => Ok(false),
            status => Err(status),
        }
    }

    pub fn pair() -> Result<(Self, Self), Status> {
        let r: kernel_fidl::SocketControlCreateSocketPairResponse = kernel_call(
            10,
            "CreateSocketPair",
            kernel_fidl::SOCKET_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::SocketControlCreateSocketPairRequest {},
        )?;
        check(r.status)?;
        Ok((Self(r.local_socket.raw), Self(r.remote_socket.raw)))
    }

    pub fn read(&self, max_bytes: u32) -> Result<Vec<u8>, Status> {
        let mut req = [0; 32];
        let mut hs = [HandleRef { raw: 0 }; 1];
        let encoded = kernel_fidl::SocketControlReadRequest {
            socket: HandleRef { raw: self.0 },
            max_bytes,
        }
        .encode(&mut req, &mut hs)
        .map_err(|_| Status::ErrInvalidArgs)?;
        let ordinal = kernel_fidl::SOCKET_CONTROL_PUBLIC_METHODS
            .iter()
            .find(|m| m.name == "Read")
            .unwrap()
            .ordinal;
        let mut out = vec![0; MAX_MESSAGE];
        let mut handles = [0; 1];
        let (n, k) = crate::syscall::fidl(
            10,
            ordinal,
            &req[..encoded.bytes],
            &[self.0],
            &mut out,
            &mut handles,
        )
        .map_err(status)?;
        let refs: Vec<_> = handles[..k].iter().map(|h| HandleRef { raw: *h }).collect();
        let r = kernel_fidl::SocketControlReadResponse::decode(&out[..n], &refs)
            .map_err(|_| Status::ErrInvalidArgs)?;
        check(r.status)?;
        Ok(r.data.to_vec())
    }

    pub fn write(&self, data: &[u8]) -> Result<u64, Status> {
        let r: kernel_fidl::SocketControlWriteResponse = kernel_call(
            10,
            "Write",
            kernel_fidl::SOCKET_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::SocketControlWriteRequest {
                socket: HandleRef { raw: self.0 },
                data,
            },
        )?;
        check(r.status)?;
        Ok(r.actual)
    }

    pub fn shutdown(&self, read: bool, write: bool) -> Result<(), Status> {
        let r: kernel_fidl::SocketControlShutdownResponse = kernel_call(
            10,
            "Shutdown",
            kernel_fidl::SOCKET_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::SocketControlShutdownRequest {
                socket: HandleRef { raw: self.0 },
                read,
                write,
            },
        )?;
        check(r.status)
    }

    pub fn info(&self) -> Result<SocketInfo, Status> {
        let r: kernel_fidl::SocketControlGetInfoResponse = kernel_call(
            10,
            "GetInfo",
            kernel_fidl::SOCKET_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::SocketControlGetInfoRequest {
                socket: HandleRef { raw: self.0 },
            },
        )?;
        check(r.status)?;
        Ok(SocketInfo {
            readable_bytes: r.readable_bytes,
            local_read_closed: r.local_read_closed,
            local_write_closed: r.local_write_closed,
            peer_read_closed: r.peer_read_closed,
            peer_write_closed: r.peer_write_closed,
        })
    }
}
/// One request in flight per channel. The envelope is an ordinal followed by
/// the generated FIDL payload; replies carry only the generated response.
#[derive(Clone, Copy)]
pub struct Rpc(pub Channel);
impl Rpc {
    /// Drain a previous untagged reply before allocating a new request's owned
    /// resources. Failure leaves the queue fenced and sends no new request.
    pub fn drain_pending_with_timeout(
        &self,
        timeout_seconds: u64,
        awaiting_response: &mut bool,
    ) -> Result<(), Status> {
        if *awaiting_response {
            let previous = self.0.recv_with_timeout(timeout_seconds)?;
            for handle in previous.handles {
                let _ = crate::Memory::close(handle);
            }
            *awaiting_response = false;
        }
        Ok(())
    }

    /// Serialize untagged replies across timeouts. The caller checkpoints
    /// `awaiting_response` with the channel when transferring service state.
    pub fn call_ordered_with_timeout(
        &self,
        ordinal: u64,
        request: &[u8],
        handles: &[u64],
        timeout_seconds: u64,
        awaiting_response: &mut bool,
    ) -> Result<Message, Status> {
        self.drain_pending_with_timeout(timeout_seconds, awaiting_response)?;
        self.call_raw_with_timeout(ordinal, request, handles, false, timeout_seconds)?;
        *awaiting_response = true;
        let response = self.0.recv_with_timeout(timeout_seconds)?;
        *awaiting_response = false;
        Ok(response)
    }
    pub fn call_raw(
        &self,
        ordinal: u64,
        request: &[u8],
        handles: &[u64],
        reply: bool,
    ) -> Result<Message, Status> {
        self.call_raw_with_timeout(ordinal, request, handles, reply, DEADLINE_SECONDS)
    }

    pub fn call_raw_with_timeout(
        &self,
        ordinal: u64,
        request: &[u8],
        handles: &[u64],
        reply: bool,
        timeout_seconds: u64,
    ) -> Result<Message, Status> {
        let mut bytes = Vec::with_capacity(8 + request.len());
        bytes.extend_from_slice(&ordinal.to_le_bytes());
        bytes.extend_from_slice(request);
        let sent = self.0.send(&bytes, handles);
        // This envelope may contain an authentication request. Do not retain
        // its plaintext while waiting for the response or on send failure.
        for byte in &mut bytes {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        sent?;
        if reply {
            self.0.recv_with_timeout(timeout_seconds)
        } else {
            Ok(Message {
                bytes: Vec::new(),
                handles: Vec::new(),
            })
        }
    }
}
macro_rules! transport {
    ($f:ident) => {
        impl $f::FidlTransport for Rpc {
            fn call(
                &mut self,
                ordinal: u64,
                req: &[u8],
                hs: &[$f::HandleRef],
                out: &mut [u8],
                oh: &mut [$f::HandleRef],
            ) -> Result<$f::EncodeResult, $f::FidlWireError> {
                let handles: Vec<_> = hs.iter().map(|h| h.raw).collect();
                let m = self
                    .call_raw(ordinal, req, &handles, true)
                    .map_err(|_| $f::FidlWireError::Transport)?;
                if m.bytes.len() > out.len() || m.handles.len() > oh.len() {
                    return Err($f::FidlWireError::BufferTooSmall);
                }
                out[..m.bytes.len()].copy_from_slice(&m.bytes);
                for (i, h) in m.handles.iter().enumerate() {
                    oh[i].raw = *h;
                }
                Ok($f::EncodeResult {
                    bytes: m.bytes.len(),
                    handles: m.handles.len(),
                })
            }
            fn send(
                &mut self,
                ordinal: u64,
                req: &[u8],
                hs: &[$f::HandleRef],
            ) -> Result<(), $f::FidlWireError> {
                self.call_raw(
                    ordinal,
                    req,
                    &hs.iter().map(|h| h.raw).collect::<Vec<_>>(),
                    false,
                )
                .map(|_| ())
                .map_err(|_| $f::FidlWireError::Transport)
            }
        }
        impl $f::FidlTransport for &mut Rpc {
            fn call(
                &mut self,
                ordinal: u64,
                req: &[u8],
                hs: &[$f::HandleRef],
                out: &mut [u8],
                oh: &mut [$f::HandleRef],
            ) -> Result<$f::EncodeResult, $f::FidlWireError> {
                let handles: Vec<_> = hs.iter().map(|h| h.raw).collect();
                let m = self
                    .call_raw(ordinal, req, &handles, true)
                    .map_err(|_| $f::FidlWireError::Transport)?;
                if m.bytes.len() > out.len() || m.handles.len() > oh.len() {
                    return Err($f::FidlWireError::BufferTooSmall);
                }
                out[..m.bytes.len()].copy_from_slice(&m.bytes);
                for (i, h) in m.handles.iter().enumerate() {
                    oh[i].raw = *h;
                }
                Ok($f::EncodeResult {
                    bytes: m.bytes.len(),
                    handles: m.handles.len(),
                })
            }
            fn send(
                &mut self,
                ordinal: u64,
                req: &[u8],
                hs: &[$f::HandleRef],
            ) -> Result<(), $f::FidlWireError> {
                self.call_raw(
                    ordinal,
                    req,
                    &hs.iter().map(|h| h.raw).collect::<Vec<_>>(),
                    false,
                )
                .map(|_| ())
                .map_err(|_| $f::FidlWireError::Transport)
            }
        }
    };
}
transport!(block_fidl);
transport!(fs_fidl);
transport!(power_fidl);
transport!(tee_manager_fidl);
transport!(ethernet_fidl);
transport!(net_fidl);
transport!(trust_fidl);
transport!(job_fidl);
transport!(app_worker_fidl);
transport!(app_service_directory_fidl);
transport!(time_fidl);
transport!(user_manager_fidl);
transport!(hardware_manager_fidl);
