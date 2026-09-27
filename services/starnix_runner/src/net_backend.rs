use bexos_userspace::{Channel, Memory, Rpc};
use n::{FidlDecode, FidlEncode};
use net_fidl as n;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub struct TcpConnection {
    pub stream: u64,
    pub control: u64,
    pub local: (IpAddr, u16),
}

pub struct AcceptedConnection {
    pub stream: u64,
    pub control: u64,
    pub peer: (IpAddr, u16),
}

struct Reply {
    bytes: Vec<u8>,
    handles: Vec<n::HandleRef>,
}

impl Reply {
    fn take(&mut self, handle: u64) -> Result<u64, i64> {
        let index = self
            .handles
            .iter()
            .position(|candidate| candidate.raw == handle)
            .ok_or(starnix_kernel::EIO)?;
        Ok(self.handles.remove(index).raw)
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            let _ = Memory::close(handle.raw);
        }
    }
}

fn rpc<Q: FidlEncode>(channel: u64, ordinal: u64, request: &Q) -> Result<Reply, i64> {
    let mut bytes = vec![0; 65_500];
    let mut handles = [n::HandleRef { raw: 0 }; 8];
    let encoded = request
        .encode(&mut bytes, &mut handles)
        .map_err(|_| starnix_kernel::EINVAL)?;
    let raw = handles[..encoded.handles]
        .iter()
        .map(|handle| handle.raw)
        .collect::<Vec<_>>();
    let response = Rpc(Channel(channel))
        .call_raw_with_timeout(ordinal, &bytes[..encoded.bytes], &raw, true, 5)
        .map_err(|status| match status {
            kernel_fidl::Status::ErrTimedOut => starnix_kernel::EAGAIN,
            kernel_fidl::Status::ErrPeerClosed => starnix_kernel::ENETUNREACH,
            _ => starnix_kernel::EIO,
        })?;
    Ok(Reply {
        bytes: response.bytes,
        handles: response
            .handles
            .into_iter()
            .map(|raw| n::HandleRef { raw })
            .collect(),
    })
}

fn decode<'a, T: FidlDecode<'a>>(reply: &'a Reply) -> Result<T, i64> {
    T::decode(&reply.bytes, &reply.handles).map_err(|_| starnix_kernel::EIO)
}

fn check(status: n::Status) -> Result<(), i64> {
    match status {
        n::Status::Ok => Ok(()),
        n::Status::ErrAccessDenied => Err(starnix_kernel::EACCES),
        n::Status::ErrInvalidArgs => Err(starnix_kernel::EINVAL),
        n::Status::ErrResourceExhausted | n::Status::ErrShouldWait => Err(starnix_kernel::EAGAIN),
        n::Status::ErrNetworkUnreachable => Err(starnix_kernel::ENETUNREACH),
        n::Status::ErrTimedOut => Err(starnix_kernel::ETIMEDOUT),
        n::Status::ErrNotFound => Err(starnix_kernel::ENOENT),
        _ => Err(starnix_kernel::EIO),
    }
}

fn wire_address(address: (IpAddr, u16)) -> n::SocketAddress {
    n::SocketAddress {
        addr: match address.0 {
            IpAddr::V4(address) => n::IpAddress::Ipv4(n::Ipv4Address {
                octets: address.octets(),
            }),
            IpAddr::V6(address) => n::IpAddress::Ipv6(n::Ipv6Address {
                octets: address.octets(),
            }),
        },
        port: address.1,
    }
}

fn native_address(address: n::SocketAddress) -> (IpAddr, u16) {
    let ip = match address.addr {
        n::IpAddress::Ipv4(value) => IpAddr::V4(Ipv4Addr::from(value.octets)),
        n::IpAddress::Ipv6(value) => IpAddr::V6(Ipv6Addr::from(value.octets)),
    };
    (ip, address.port)
}

fn options(nonblocking: bool, interface: Option<u64>) -> n::SocketOptions {
    n::SocketOptions {
        non_blocking: Some(nonblocking),
        keep_alive_ms: None,
        rx_buffer_size: None,
        tx_buffer_size: None,
        bound_interface_id: interface,
    }
}

pub fn connect_tcp(
    provider: u64,
    remote: (IpAddr, u16),
    nonblocking: bool,
    interface: Option<u64>,
) -> Result<TcpConnection, i64> {
    let (client, server) = Channel::pair().map_err(|_| starnix_kernel::ENOMEM)?;
    let result = (|| {
        let reply = rpc(
            provider,
            1,
            &n::SocketProviderConnectTcpRequest {
                target: n::Endpoint::Ip(wire_address(remote)),
                options: options(nonblocking, interface),
                socket: n::HandleRef { raw: server.0 },
            },
        )?;
        let response: n::SocketProviderConnectTcpResponse = decode(&reply)?;
        check(response.status)?;
        let local_reply = rpc(client.0, 3, &n::TcpSocketGetLocalAddressRequest {})?;
        let local: n::TcpSocketGetLocalAddressResponse = decode(&local_reply)?;
        check(local.status)?;
        let mut stream_reply = rpc(client.0, 1, &n::TcpSocketGetStreamRequest {})?;
        let stream: n::TcpSocketGetStreamResponse = decode(&stream_reply)?;
        check(stream.status)?;
        Ok(TcpConnection {
            stream: stream_reply.take(stream.socket.raw)?,
            control: client.0,
            local: native_address(local.addr),
        })
    })();
    if result.is_err() {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
    }
    result
}

pub fn listen_tcp(
    provider: u64,
    local: (IpAddr, u16),
    nonblocking: bool,
    interface: Option<u64>,
) -> Result<u64, i64> {
    let (client, server) = Channel::pair().map_err(|_| starnix_kernel::ENOMEM)?;
    let result = (|| {
        let reply = rpc(
            provider,
            2,
            &n::SocketProviderListenTcpRequest {
                local_addr: wire_address(local),
                options: options(nonblocking, interface),
                listener: n::HandleRef { raw: server.0 },
            },
        )?;
        let response: n::SocketProviderListenTcpResponse = decode(&reply)?;
        check(response.status)?;
        Ok(client.0)
    })();
    if result.is_err() {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
    }
    result
}

pub fn accept_tcp(listener: u64) -> Result<AcceptedConnection, i64> {
    let mut reply = rpc(listener, 1, &n::TcpListenerAcceptRequest {})?;
    let response: n::TcpListenerAcceptResponse = decode(&reply)?;
    check(response.status)?;
    let control = reply.take(response.client.raw)?;
    let mut stream_reply = rpc(control, 1, &n::TcpSocketGetStreamRequest {})?;
    let stream: n::TcpSocketGetStreamResponse = decode(&stream_reply)?;
    check(stream.status)?;
    Ok(AcceptedConnection {
        stream: stream_reply.take(stream.socket.raw)?,
        control,
        peer: native_address(response.peer_addr),
    })
}

pub fn create_udp(
    provider: u64,
    local: (IpAddr, u16),
    nonblocking: bool,
    interface: Option<u64>,
) -> Result<u64, i64> {
    let (client, server) = Channel::pair().map_err(|_| starnix_kernel::ENOMEM)?;
    let result = (|| {
        let reply = rpc(
            provider,
            3,
            &n::SocketProviderCreateUdpSocketRequest {
                options: options(nonblocking, interface),
                socket: n::HandleRef { raw: server.0 },
            },
        )?;
        let response: n::SocketProviderCreateUdpSocketResponse = decode(&reply)?;
        check(response.status)?;
        let reply = rpc(
            client.0,
            3,
            &n::UdpSocketBindRequest {
                local_addr: wire_address(local),
            },
        )?;
        let response: n::UdpSocketBindResponse = decode(&reply)?;
        check(response.status)?;
        Ok(client.0)
    })();
    if result.is_err() {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
    }
    result
}

pub fn send_udp(control: u64, data: &[u8], destination: (IpAddr, u16)) -> Result<usize, i64> {
    let reply = rpc(
        control,
        1,
        &n::UdpSocketSendToRequest {
            data,
            destination: wire_address(destination),
        },
    )?;
    let response: n::UdpSocketSendToResponse = decode(&reply)?;
    check(response.status)?;
    usize::try_from(response.actual).map_err(|_| starnix_kernel::EIO)
}

pub fn recv_udp(control: u64) -> Result<(Vec<u8>, (IpAddr, u16)), i64> {
    let reply = rpc(control, 2, &n::UdpSocketRecvFromRequest {})?;
    let response: n::UdpSocketRecvFromResponse = decode(&reply)?;
    check(response.status)?;
    Ok((response.data.to_vec(), native_address(response.source)))
}
