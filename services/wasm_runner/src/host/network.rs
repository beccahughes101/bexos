use super::*;
use n::FidlEncode;
use net_fidl as n;
struct Reply {
    bytes: Vec<u8>,
    handles: Vec<n::HandleRef>,
}
impl Reply {
    fn take_handle(&mut self, raw: u64) -> NetResult<u64> {
        let index = self
            .handles
            .iter()
            .position(|handle| handle.raw == raw)
            .ok_or(net::ErrorCode::Unknown)?;
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
fn rpc<Q: FidlEncode>(channel: u64, ordinal: u64, q: Q) -> NetResult<Reply> {
    let mut bytes = vec![0; 32768];
    let mut handles = [n::HandleRef { raw: 0 }; 16];
    let encoded = q
        .encode(&mut bytes, &mut handles)
        .map_err(|_| net::ErrorCode::InvalidArgument)?;
    let raw: Vec<_> = handles[..encoded.handles].iter().map(|h| h.raw).collect();
    let reply = bexos_userspace::Rpc(Channel(channel))
        .call_raw_with_timeout(ordinal, &bytes[..encoded.bytes], &raw, true, 5)
        .map_err(|_| net::ErrorCode::Timeout)?;
    Ok(Reply {
        bytes: reply.bytes,
        handles: reply
            .handles
            .into_iter()
            .map(|raw| n::HandleRef { raw })
            .collect(),
    })
}
fn check(status: n::Status) -> NetResult<()> {
    match status {
        n::Status::Ok => Ok(()),
        n::Status::ErrAccessDenied => Err(net::ErrorCode::AccessDenied),
        n::Status::ErrShouldWait | n::Status::ErrResourceExhausted => {
            Err(net::ErrorCode::WouldBlock)
        }
        n::Status::ErrTimedOut => Err(net::ErrorCode::Timeout),
        n::Status::ErrNetworkUnreachable => Err(net::ErrorCode::RemoteUnreachable),
        n::Status::ErrInvalidArgs => Err(net::ErrorCode::InvalidArgument),
        _ => Err(net::ErrorCode::Unknown),
    }
}
fn decode<'a, T: n::FidlDecode<'a>>(r: &'a Reply) -> NetResult<T> {
    T::decode(&r.bytes, &r.handles).map_err(|_| net::ErrorCode::Unknown)
}
fn address(a: &net::IpSocketAddress) -> NetResult<n::SocketAddress> {
    Ok(match a {
        net::IpSocketAddress::Ipv4(a) => n::SocketAddress {
            addr: n::IpAddress::Ipv4(n::Ipv4Address {
                octets: [a.address.0, a.address.1, a.address.2, a.address.3],
            }),
            port: a.port,
        },
        net::IpSocketAddress::Ipv6(a) => {
            if a.scope_id != 0 || a.flow_info != 0 {
                return Err(net::ErrorCode::NotSupported);
            }
            let mut octets = [0; 16];
            for (i, s) in [
                a.address.0,
                a.address.1,
                a.address.2,
                a.address.3,
                a.address.4,
                a.address.5,
                a.address.6,
                a.address.7,
            ]
            .iter()
            .enumerate()
            {
                octets[i * 2..i * 2 + 2].copy_from_slice(&s.to_be_bytes());
            }
            n::SocketAddress {
                addr: n::IpAddress::Ipv6(n::Ipv6Address { octets }),
                port: a.port,
            }
        }
    })
}
fn ip(a: n::IpAddress) -> net::IpAddress {
    match a {
        n::IpAddress::Ipv4(a) => {
            net::IpAddress::Ipv4((a.octets[0], a.octets[1], a.octets[2], a.octets[3]))
        }
        n::IpAddress::Ipv6(a) => {
            let s: Vec<_> = a
                .octets
                .chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect();
            net::IpAddress::Ipv6((s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]))
        }
    }
}
fn socket_address(a: n::SocketAddress) -> net::IpSocketAddress {
    match ip(a.addr) {
        net::IpAddress::Ipv4(address) => net::IpSocketAddress::Ipv4(net::Ipv4SocketAddress {
            address,
            port: a.port,
        }),
        net::IpAddress::Ipv6(address) => net::IpSocketAddress::Ipv6(net::Ipv6SocketAddress {
            address,
            port: a.port,
            flow_info: 0,
            scope_id: 0,
        }),
    }
}
fn pair() -> NetResult<(Channel, Channel)> {
    Channel::pair().map_err(|_| net::ErrorCode::OutOfMemory)
}
fn options() -> n::SocketOptions {
    n::SocketOptions {
        non_blocking: Some(true),
        keep_alive_ms: None,
        rx_buffer_size: Some(65536),
        tx_buffer_size: Some(65536),
    }
}
pub fn resolve(g: &dyn Handle, name: &str) -> NetResult<Vec<net::IpAddress>> {
    let r = rpc(
        g.native(),
        4,
        n::NetstackResolveHostRequest { hostname: name },
    )?;
    let r: n::NetstackResolveHostResponse = decode(&r)?;
    check(r.status)?;
    (0..r.addresses.len())
        .map(|index| {
            r.addresses
                .get(index)
                .map(ip)
                .map_err(|_| net::ErrorCode::Unknown)
        })
        .collect()
}
fn stream(control: u64) -> NetResult<Entry> {
    let mut reply = rpc(control, 1, n::TcpSocketGetStreamRequest {})?;
    let response: n::TcpSocketGetStreamResponse = decode(&reply)?;
    check(response.status)?;
    let raw = response.socket.raw;
    let raw = reply.take_handle(raw)?;
    Ok(Entry {
        name: String::new(),
        handle: Arc::new(NativeHandle {
            raw,
            kind: Kind::Socket,
            rights: READ | WRITE | TRANSFER,
            companions: vec![control],
            allowed_methods: None,
            grant: None,
            ownership: None,
        }),
    })
}
pub fn connect(
    g: &dyn Handle,
    a: &net::IpSocketAddress,
) -> NetResult<(Entry, net::IpSocketAddress)> {
    let address = address(a)?;
    let (client, server) = pair()?;
    let result = (|| {
        let r = rpc(
            g.native(),
            1,
            n::NetstackConnectTcpRequest {
                remote_addr: address,
                options: options(),
                socket: n::HandleRef { raw: server.0 },
            },
        )?;
        let r: n::NetstackConnectTcpResponse = decode(&r)?;
        check(r.status)?;
        let response = rpc(client.0, 3, n::TcpSocketGetLocalAddressRequest {})?;
        let response: n::TcpSocketGetLocalAddressResponse = decode(&response)?;
        check(response.status)?;
        let local = socket_address(response.addr);
        let stream = stream(client.0)?;
        Ok((stream, local))
    })();
    if result.is_err() {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
    }
    result
}
pub fn listen(g: &dyn Handle, a: &net::IpSocketAddress) -> NetResult<Entry> {
    let address = address(a)?;
    if address.port == 0 {
        return Err(net::ErrorCode::NotSupported);
    }
    let (client, server) = pair()?;
    let result = (|| {
        let r = rpc(
            g.native(),
            2,
            n::NetstackListenTcpRequest {
                local_addr: address,
                options: options(),
                listener: n::HandleRef { raw: server.0 },
            },
        )?;
        let r: n::NetstackListenTcpResponse = decode(&r)?;
        check(r.status)?;
        Ok(entry("", client.0, Kind::Channel, READ | WRITE | TRANSFER))
    })();
    if result.is_err() {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
    }
    result
}
pub fn accept(listener: &dyn Handle) -> NetResult<(Entry, net::IpSocketAddress)> {
    let mut reply = rpc(listener.native(), 1, n::TcpListenerAcceptRequest {})?;
    let response: n::TcpListenerAcceptResponse = decode(&reply)?;
    check(response.status)?;
    let peer = socket_address(response.peer_addr);
    let raw = response.client.raw;
    let raw = reply.take_handle(raw)?;
    match stream(raw) {
        Ok(e) => Ok((e, peer)),
        Err(e) => {
            let _ = Memory::close(raw);
            Err(e)
        }
    }
}
pub fn shutdown(h: &dyn Handle, how: ShutdownType) -> NetResult<()> {
    bexos_userspace::Socket(h.native())
        .shutdown(
            matches!(how, ShutdownType::Receive | ShutdownType::Both),
            matches!(how, ShutdownType::Send | ShutdownType::Both),
        )
        .map_err(|_| net::ErrorCode::InvalidState)
}
pub fn bind_udp(g: &dyn Handle, a: &net::IpSocketAddress) -> NetResult<Entry> {
    let address = address(a)?;
    if address.port == 0 {
        return Err(net::ErrorCode::NotSupported);
    }
    let (client, server) = pair()?;
    let result = (|| {
        let r = rpc(
            g.native(),
            3,
            n::NetstackCreateUdpSocketRequest {
                options: options(),
                socket: n::HandleRef { raw: server.0 },
            },
        )?;
        let r: n::NetstackCreateUdpSocketResponse = decode(&r)?;
        check(r.status)?;
        let r = rpc(
            client.0,
            3,
            n::UdpSocketBindRequest {
                local_addr: address,
            },
        )?;
        let r: n::UdpSocketBindResponse = decode(&r)?;
        check(r.status)?;
        Ok(entry("", client.0, Kind::Channel, READ | WRITE | TRANSFER))
    })();
    if result.is_err() {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
    }
    result
}
pub fn receive_udp(
    h: &dyn Handle,
    maximum: usize,
    remote: Option<&net::IpSocketAddress>,
) -> NetResult<Vec<IncomingDatagram>> {
    let mut out = Vec::new();
    for _ in 0..maximum.min(16) {
        let reply = (|| {
            let r = rpc(h.native(), 2, n::UdpSocketRecvFromRequest {})?;
            let decoded: n::UdpSocketRecvFromResponse = decode(&r)?;
            check(decoded.status)?;
            Ok((decoded.source, decoded.data.to_vec()))
        })();
        let (source, data) = match reply {
            Ok(value) => value,
            Err(net::ErrorCode::WouldBlock) => break,
            Err(_) if !out.is_empty() => break,
            Err(error) => return Err(error),
        };
        let source = socket_address(source);
        if remote.is_none_or(|wanted| same_address(wanted, &source)) {
            out.push(IncomingDatagram {
                data,
                remote_address: source,
            });
        }
    }
    Ok(out)
}
fn same_address(a: &net::IpSocketAddress, b: &net::IpSocketAddress) -> bool {
    match (a, b) {
        (net::IpSocketAddress::Ipv4(a), net::IpSocketAddress::Ipv4(b)) => {
            a.address == b.address && a.port == b.port
        }
        (net::IpSocketAddress::Ipv6(a), net::IpSocketAddress::Ipv6(b)) => {
            a.address == b.address
                && a.port == b.port
                && a.scope_id == b.scope_id
                && a.flow_info == b.flow_info
        }
        _ => false,
    }
}
pub fn send_udp(
    h: &dyn Handle,
    datagrams: &[OutgoingDatagram],
    remote: Option<&net::IpSocketAddress>,
) -> NetResult<u64> {
    // Validate the entire batch before sending anything. Once a datagram has
    // been sent, report its count even if a later operation fails.
    let targets = datagrams
        .iter()
        .map(|d| {
            let target = d
                .remote_address
                .as_ref()
                .or(remote)
                .ok_or(net::ErrorCode::InvalidArgument)?;
            if remote.is_some_and(|r| !same_address(r, target)) {
                return Err(net::ErrorCode::InvalidArgument);
            }
            address(target)
        })
        .collect::<NetResult<Vec<_>>>()?;
    let mut count = 0;
    for (d, destination) in datagrams.iter().zip(targets) {
        let result = (|| {
            let r = rpc(
                h.native(),
                1,
                n::UdpSocketSendToRequest {
                    data: &d.data,
                    destination,
                },
            )?;
            let r: n::UdpSocketSendToResponse = decode(&r)?;
            check(r.status)?;
            if r.actual != d.data.len() as u64 {
                return Err(net::ErrorCode::Unknown);
            }
            Ok(())
        })();
        match result {
            Ok(()) => count += 1,
            Err(net::ErrorCode::WouldBlock) => break,
            Err(_) if count > 0 => break,
            Err(error) => return Err(error),
        }
    }
    Ok(count)
}

pub fn ready(handle: &dyn Handle, udp: bool, write: bool) -> NetResult<bool> {
    if udp {
        let reply = rpc(handle.native(), 5, n::UdpSocketGetInfoRequest {})?;
        let info: n::UdpSocketGetInfoResponse = decode(&reply)?;
        check(info.status)?;
        Ok(if write { info.writable } else { info.readable })
    } else {
        let reply = rpc(handle.native(), 3, n::TcpListenerGetInfoRequest {})?;
        let info: n::TcpListenerGetInfoResponse = decode(&reply)?;
        check(info.status)?;
        Ok(info.readable)
    }
}
