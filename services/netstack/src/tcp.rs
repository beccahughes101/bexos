use alloc::vec::Vec;
use bexos_userspace::Socket;
use net_fidl::{IpAddress, Ipv4Address, SocketAddress, Status};
use smoltcp::iface::SocketHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpState {
    Connecting,
    Established,
    Listening,
    Closed,
}

#[derive(Clone)]
pub struct TcpEndpoint {
    pub control: u64,
    pub stream: Option<Socket>,
    pub peer: SocketAddress,
    pub local: SocketAddress,
    pub state: TcpState,
    pub smoltcp_handle: Option<SocketHandle>,
    pub smoltcp_migration: Option<smoltcp::socket::tcp::MigrationState>,
    pub client_control: Option<u64>,
}

#[derive(Clone)]
pub struct TcpListenerState {
    pub control: u64,
    pub local: SocketAddress,
    pub pending: Vec<TcpEndpoint>,
    pub closed: bool,
    pub smoltcp_handle: Option<SocketHandle>,
}

pub fn unspecified() -> SocketAddress {
    SocketAddress {
        addr: IpAddress::Ipv4(Ipv4Address {
            octets: [0, 0, 0, 0],
        }),
        port: 0,
    }
}

impl TcpEndpoint {
    pub fn new(control: u64, peer: SocketAddress, local: SocketAddress) -> Self {
        Self {
            control,
            stream: None,
            peer,
            local,
            state: TcpState::Connecting,
            smoltcp_handle: None,
            smoltcp_migration: None,
            client_control: None,
        }
    }

    pub fn attach_stream(&mut self) -> Result<Socket, Status> {
        if self.state == TcpState::Closed {
            return Err(Status::ErrPeerClosed);
        }
        if let Some(socket) = self.stream {
            return Ok(socket);
        }
        let (service, client) = Socket::pair().map_err(map_kernel_status)?;
        self.stream = Some(service);
        self.state = TcpState::Established;
        Ok(client)
    }

    pub fn close(&mut self) {
        if let Some(socket) = self.stream {
            let _ = socket.shutdown(true, true);
        }
        self.state = TcpState::Closed;
    }
}

impl TcpListenerState {
    pub fn new(control: u64, local: SocketAddress) -> Self {
        Self {
            control,
            local,
            pending: Vec::new(),
            closed: false,
            smoltcp_handle: None,
        }
    }

    pub fn accept(&mut self) -> Result<TcpEndpoint, Status> {
        if self.closed {
            return Err(Status::ErrPeerClosed);
        }
        if self.pending.is_empty() {
            return Err(Status::ErrShouldWait);
        }
        Ok(self.pending.remove(0))
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.pending.clear();
    }
}

pub fn map_kernel_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::Ok => Status::Ok,
        kernel_fidl::Status::ErrInvalidHandle => Status::ErrInvalidHandle,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrBufferTooSmall => Status::ErrBufferTooSmall,
        kernel_fidl::Status::ErrPeerClosed => Status::ErrPeerClosed,
        kernel_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        kernel_fidl::Status::ErrAlreadyExists => Status::ErrAlreadyExists,
        kernel_fidl::Status::ErrResourceExhausted => Status::ErrResourceExhausted,
        _ => Status::ErrInvalidArgs,
    }
}
