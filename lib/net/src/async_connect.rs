//! Deadline-bound connection setup on a private Netstack binding.
use crate::{NetError, secure::BexosSocketIo};
use bexos_userspace::{Channel, Memory, Message, Socket};
use net_fidl::{FidlDecode, FidlEncode, HandleRef, Status};
use std::{future::poll_fn, task::Poll};
struct OwnedChannel(Channel);
impl Drop for OwnedChannel {
    fn drop(&mut self) {
        if self.0.0 != 0 {
            let _ = Memory::close(self.0.0);
        }
    }
}
struct Reply(Message);
impl Drop for Reply {
    fn drop(&mut self) {
        for handle in &self.0.handles {
            let _ = Memory::close(*handle);
        }
    }
}
async fn call(
    channel: Channel,
    ordinal: u64,
    request: &impl FidlEncode,
    timeout_ms: u64,
) -> Result<Reply, NetError> {
    let mut bytes = [0; 1024];
    let mut handles = [HandleRef { raw: 0 }; 2];
    let encoded = request
        .encode(&mut bytes[8..], &mut handles)
        .map_err(|_| NetError::InvalidArgs)?;
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    let raw: Vec<_> = handles[..encoded.handles].iter().map(|h| h.raw).collect();
    if channel.send(&bytes[..8 + encoded.bytes], &raw).is_err() {
        for handle in raw {
            let _ = Memory::close(handle);
        }
        return Err(NetError::Network);
    }
    crate::async_http::with_deadline(
        poll_fn(|cx| match channel.try_recv() {
            Err(kernel_fidl::Status::ErrTimedOut) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Ok(message) => Poll::Ready(Ok(Reply(message))),
            Err(_) => Poll::Ready(Err(NetError::Network)),
        }),
        timeout_ms,
    )
    .await
}
/// Takes ownership; cancellation closes this private channel and TCP control.
pub fn connect(
    channel: Channel,
    host: &str,
    port: u16,
) -> impl core::future::Future<Output = Result<BexosSocketIo, NetError>> + '_ {
    connect_with_timeout(channel, host, port, 10_000)
}

pub fn connect_with_timeout(
    channel: Channel,
    host: &str,
    port: u16,
    timeout_ms: u64,
) -> impl core::future::Future<Output = Result<BexosSocketIo, NetError>> + '_ {
    // Construct the owner before returning the future, including for callers
    // that cancel without ever polling it.
    let channel = OwnedChannel(channel);
    crate::async_http::with_deadline(
        connect_inner(channel, host, port, timeout_ms, false),
        timeout_ms,
    )
}

pub fn connect_scoped_with_timeout(
    channel: Channel,
    host: &str,
    port: u16,
    timeout_ms: u64,
) -> impl core::future::Future<Output = Result<BexosSocketIo, NetError>> + '_ {
    let channel = OwnedChannel(channel);
    crate::async_http::with_deadline(
        connect_inner(channel, host, port, timeout_ms, true),
        timeout_ms,
    )
}

async fn connect_inner(
    channel: OwnedChannel,
    host: &str,
    port: u16,
    timeout_ms: u64,
    scoped: bool,
) -> Result<BexosSocketIo, NetError> {
    let address = if scoped {
        None
    } else if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        Some(net_fidl::IpAddress::Ipv4(net_fidl::Ipv4Address {
            octets: ip.octets(),
        }))
    } else {
        Some(
            crate::async_http::with_deadline(
                async {
                    loop {
                        let reply = call(
                            channel.0,
                            4,
                            &net_fidl::NetstackResolveHostRequest { hostname: host },
                            timeout_ms,
                        )
                        .await?;
                        if !reply.0.handles.is_empty() {
                            return Err(NetError::Network);
                        }
                        let resolved =
                            net_fidl::NetstackResolveHostResponse::decode(&reply.0.bytes, &[])
                                .map_err(|_| NetError::Network)?;
                        if resolved.status == Status::ErrShouldWait {
                            continue;
                        }
                        if resolved.status != Status::Ok {
                            return Err(NetError::Network);
                        }
                        return resolved.addresses.get(0).map_err(|_| NetError::Network);
                    }
                },
                timeout_ms,
            )
            .await?,
        )
    };
    let (control, server) = Channel::pair().map_err(|_| NetError::Network)?;
    let mut control = OwnedChannel(control);
    let options = net_fidl::SocketOptions {
        non_blocking: Some(true),
        keep_alive_ms: Some(0),
        rx_buffer_size: Some(64 * 1024),
        tx_buffer_size: Some(64 * 1024),
    };
    let reply = if scoped {
        call(
            channel.0,
            1,
            &net_fidl::SocketProviderConnectTcpRequest {
                target: net_fidl::Endpoint::Domain(net_fidl::DomainEndpoint { host, port }),
                options,
                socket: HandleRef { raw: server.0 },
            },
            timeout_ms,
        )
        .await?
    } else {
        call(
            channel.0,
            1,
            &net_fidl::NetstackConnectTcpRequest {
                remote_addr: net_fidl::SocketAddress {
                    addr: address.expect("legacy resolution produced address"),
                    port,
                },
                options,
                socket: HandleRef { raw: server.0 },
            },
            timeout_ms,
        )
        .await?
    };
    let status = if scoped {
        net_fidl::SocketProviderConnectTcpResponse::decode(&reply.0.bytes, &[])
            .map_err(|_| NetError::Network)?
            .status
    } else {
        net_fidl::NetstackConnectTcpResponse::decode(&reply.0.bytes, &[])
            .map_err(|_| NetError::Network)?
            .status
    };
    if !reply.0.handles.is_empty() || status != Status::Ok {
        return Err(NetError::Network);
    }
    let mut reply = call(
        control.0,
        1,
        &net_fidl::TcpSocketGetStreamRequest {},
        timeout_ms,
    )
    .await?;
    let refs: Vec<_> = reply
        .0
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect();
    let response = net_fidl::TcpSocketGetStreamResponse::decode(&reply.0.bytes, &refs)
        .map_err(|_| NetError::Network)?;
    if response.status != Status::Ok || refs.len() != 1 || response.socket.raw != refs[0].raw {
        return Err(NetError::Network);
    }
    let socket = reply.0.handles.remove(0);
    Ok(BexosSocketIo::with_control(
        Socket(socket),
        core::mem::replace(&mut control.0, Channel(0)),
    ))
}
