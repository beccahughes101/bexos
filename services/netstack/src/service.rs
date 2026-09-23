use alloc::format;
use alloc::vec::Vec;
use bexos_userspace::live_migration::Source;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Startup, log};
use net_fidl::{
    FidlDecode, FidlEncode, HandleRef, IpAddress, Ipv4Address, Ipv6Address, LinkStatus,
    LinkWatcherOnLinkStatusRequest, LinkWatcherPublicClient, NetstackConnectTcpRequest,
    NetstackConnectTcpResponse, NetstackCreateUdpSocketRequest, NetstackCreateUdpSocketResponse,
    NetstackGetLinkStatusRequest, NetstackGetLinkStatusResponse, NetstackListenTcpRequest,
    NetstackListenTcpResponse, NetstackResolveHostRequest, NetstackResolveHostResponse,
    NetstackWatchLinkStatusRequest, NetstackWatchLinkStatusResponse, Status,
    TcpListenerAcceptRequest, TcpListenerAcceptResponse, TcpListenerCloseRequest,
    TcpListenerGetInfoRequest, TcpListenerGetInfoResponse, TcpSocketCloseRequest,
    TcpSocketGetLocalAddressRequest, TcpSocketGetLocalAddressResponse,
    TcpSocketGetPeerAddressRequest, TcpSocketGetPeerAddressResponse, TcpSocketGetStreamRequest,
    TcpSocketGetStreamResponse, TcpSocketShutdownRequest, TcpSocketShutdownResponse,
    UdpSocketBindRequest, UdpSocketBindResponse, UdpSocketCloseRequest, UdpSocketGetInfoRequest,
    UdpSocketGetInfoResponse, UdpSocketRecvFromRequest, UdpSocketRecvFromResponse,
    UdpSocketSendToRequest, UdpSocketSendToResponse,
};

use crate::config::{ConfigSource, NetConfig};
use crate::dns::DnsRecord;
use crate::ethernet;
use crate::link::PacketLink;
use crate::migration::Runtime;
use crate::stack::{Netstack, empty_addr};

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("netstackd startup");
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state).await,
            Err(_) => bexos_userspace::exit(),
        }
    }
    let config = NetConfig::from_startup(&startup);
    let ethernet_endpoint = startup
        .service_grants
        .iter()
        .find(|grant| grant.service == "bexos.hardware.ethernet.Device")
        .map(|grant| grant.endpoint);
    let tls_trust = startup
        .service_grants
        .iter()
        .find(|grant| grant.service == "bexos.security.trust.TlsTrustManager")
        .map(|grant| Channel(grant.endpoint));
    let link = ethernet_endpoint.and_then(|endpoint| {
        let link = ethernet::connect(endpoint)
            .map_err(|error| {
                log(&format!(
                    "netstackd: Ethernet connection failed: {error:?}\n"
                ));
            })
            .ok()?;
        PacketLink::new(link)
            .map_err(|error| {
                log(&format!(
                    "netstackd: Ethernet packet mapping failed: {error:?}\n"
                ));
            })
            .ok()
    });
    let stack = Netstack::new(config, None);
    log_ready(&stack, link.as_ref().map_or(0, |link| link.resources.mtu));
    Startup::ready(control).unwrap();
    serve(Runtime {
        control,
        migration: startup.migration,
        tls_trust,
        clients: Vec::new(),
        link_watchers: Vec::new(),
        stack,
        link,
        generation: 0,
    })
    .await
}

async fn serve(mut runtime: Runtime) -> ! {
    let control = runtime.control;
    let mut source = Source::new(runtime.migration);
    let mut packet_changes = bexos_userspace::live_migration::RecordChanges::default();
    loop {
        packet_changes.poll(&runtime, &mut source);
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if runtime
            .link
            .as_mut()
            .is_some_and(|link| !link.drain_for_quiesce())
        {
            source.changed(0);
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        let had_link = runtime.link.is_some();
        bexos_trace::trace_scope!(
            bexos_trace::CATEGORY_NETWORK_STACK,
            "netstack:poll_packet_plane"
        );
        runtime.stack.poll_packet_plane(runtime.link.as_mut());
        if had_link != runtime.link.is_some() {
            notify_link_watchers(&mut runtime.link_watchers, runtime.link.as_ref());
            source.changed(0);
        }
        if let Ok(message) = control.try_recv() {
            bexos_trace::trace_instant!(
                bexos_trace::CATEGORY_NETWORK_STACK,
                "netstack:control_message"
            );
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("Netstack") {
                        log(&format!(
                            "netstackd: bound endpoint={endpoint} caller={:?} methods={:?}\n",
                            binding.caller_package, binding.method_ordinals
                        ));
                        runtime.clients.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                        source.changed(0);
                    }
                } else if metadata_protocol(metadata) == Some("Netstack") {
                    let _ = bexos_userspace::Memory::close(endpoint);
                }
            }
        }
        // Every class must be polled even when another one made progress.
        // Short-circuiting here lets repeated directory/DNS activity starve
        // TCP setup, stream control and cancellation indefinitely.
        let mut changed = poll_netstack_clients(
            &mut runtime.clients,
            &mut runtime.link_watchers,
            &mut runtime.stack,
            runtime.link.as_mut(),
        );
        changed |= poll_tcp_clients(&mut runtime.stack);
        changed |= poll_listener_clients(&mut runtime.stack);
        changed |= poll_udp_clients(&mut runtime.stack);
        if changed {
            source.changed(0);
        }
        bexos_userspace::yield_now();
    }
}

fn poll_netstack_clients(
    clients: &mut Vec<BoundServiceEndpoint>,
    watchers: &mut Vec<u64>,
    stack: &mut Netstack,
    mut link: Option<&mut PacketLink>,
) -> bool {
    let mut changed = false;
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            bexos_trace::trace_counter!(
                bexos_trace::CATEGORY_NETWORK_STACK,
                "netstack:client_ordinal",
                ordinal as i64,
            );
            let handles = handle_refs(&message.handles);
            if !client.allows(ordinal) {
                log(&format!(
                    "netstackd: denied ordinal={ordinal} allowed={:?}\n",
                    client.allowed_methods
                ));
                return true;
            }
            match ordinal {
                1 => {
                    let status = NetstackConnectTcpRequest::decode(req, &handles)
                        .map(|request| {
                            let status = stack.connect_tcp(request.socket.raw, request.remote_addr);
                            if status == Status::Ok {
                                let status = stack
                                    .attach_tcp_to_link(request.socket.raw, link.as_deref_mut());
                                if status != Status::Ok {
                                    stack.remove_tcp(request.socket.raw);
                                }
                                status
                            } else {
                                let _ = bexos_userspace::Memory::close(request.socket.raw);
                                status
                            }
                        })
                        .unwrap_or(Status::ErrInvalidArgs);
                    reply(client.channel, &NetstackConnectTcpResponse { status });
                }
                2 => {
                    let status = NetstackListenTcpRequest::decode(req, &handles)
                        .map(|request| {
                            let status = stack.listen_tcp(request.listener.raw, request.local_addr);
                            if status == Status::Ok {
                                stack.attach_listener_to_link(
                                    request.listener.raw,
                                    link.as_deref_mut(),
                                )
                            } else {
                                status
                            }
                        })
                        .unwrap_or(Status::ErrInvalidArgs);
                    reply(client.channel, &NetstackListenTcpResponse { status });
                }
                3 => {
                    let status = NetstackCreateUdpSocketRequest::decode(req, &handles)
                        .map(|request| stack.create_udp(request.socket.raw))
                        .unwrap_or(Status::ErrInvalidArgs);
                    reply(client.channel, &NetstackCreateUdpSocketResponse { status });
                }
                4 => {
                    reply_resolve_host(client.channel, stack, link.as_deref_mut(), req, &handles);
                }
                5 => {
                    let response = if NetstackGetLinkStatusRequest::decode(req, &handles).is_ok() {
                        NetstackGetLinkStatusResponse {
                            status: Status::Ok,
                            link: LinkStatus {
                                available: link.is_some(),
                                metered: false,
                            },
                        }
                    } else {
                        NetstackGetLinkStatusResponse {
                            status: Status::ErrInvalidArgs,
                            link: LinkStatus {
                                available: false,
                                metered: true,
                            },
                        }
                    };
                    reply(client.channel, &response);
                }
                6 => {
                    let response = match NetstackWatchLinkStatusRequest::decode(req, &handles) {
                        Ok(request) if request.watcher.raw != 0 => {
                            watchers.push(request.watcher.raw);
                            notify_link_watchers(watchers, link.as_deref());
                            NetstackWatchLinkStatusResponse { status: Status::Ok }
                        }
                        _ => NetstackWatchLinkStatusResponse {
                            status: Status::ErrInvalidArgs,
                        },
                    };
                    reply(client.channel, &response);
                }
                _ => {}
            }
            true
        }
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            changed = true;
            let _ = bexos_userspace::Memory::close(client.channel.0);
            false
        }
        Err(_) => true,
    });
    changed
}

fn notify_link_watchers(watchers: &mut Vec<u64>, link: Option<&PacketLink>) {
    let status = LinkStatus {
        available: link.is_some(),
        metered: false,
    };
    watchers.retain(|watcher| {
        let mut client = LinkWatcherPublicClient::new(bexos_userspace::Rpc(Channel(*watcher)));
        let mut request_bytes = [0; 32];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        client
            .on_link_status(
                &LinkWatcherOnLinkStatusRequest { link: status },
                &mut request_bytes,
                &mut request_handles,
            )
            .is_ok()
    });
}

fn poll_tcp_clients(stack: &mut Netstack) -> bool {
    let mut changed = false;
    let controls = stack
        .tcp
        .iter()
        .map(|endpoint| endpoint.control)
        .collect::<Vec<_>>();
    for control in controls {
        let channel = Channel(control);
        let message = match channel.try_recv() {
            Ok(message) => message,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                stack.remove_tcp(control);
                changed = true;
                continue;
            }
            Err(_) => continue,
        };
        {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            match ordinal {
                1 => {
                    let response = if TcpSocketGetStreamRequest::decode(req, &handles).is_ok() {
                        match stack
                            .tcp_mut(control)
                            .and_then(|tcp| tcp.attach_stream().ok())
                        {
                            Some(socket) => TcpSocketGetStreamResponse {
                                status: Status::Ok,
                                socket: HandleRef { raw: socket.0 },
                            },
                            None => TcpSocketGetStreamResponse {
                                status: Status::ErrPeerClosed,
                                socket: HandleRef { raw: 0 },
                            },
                        }
                    } else {
                        TcpSocketGetStreamResponse {
                            status: Status::ErrInvalidArgs,
                            socket: HandleRef { raw: 0 },
                        }
                    };
                    reply(channel, &response);
                }
                2 => {
                    let response = if TcpSocketGetPeerAddressRequest::decode(req, &handles).is_ok()
                    {
                        stack
                            .tcp_mut(control)
                            .map(|tcp| TcpSocketGetPeerAddressResponse {
                                status: Status::Ok,
                                addr: tcp.peer,
                            })
                            .unwrap_or(TcpSocketGetPeerAddressResponse {
                                status: Status::ErrNotFound,
                                addr: empty_addr(),
                            })
                    } else {
                        TcpSocketGetPeerAddressResponse {
                            status: Status::ErrInvalidArgs,
                            addr: empty_addr(),
                        }
                    };
                    reply(channel, &response);
                }
                3 => {
                    let response = if TcpSocketGetLocalAddressRequest::decode(req, &handles).is_ok()
                    {
                        stack
                            .tcp_mut(control)
                            .map(|tcp| TcpSocketGetLocalAddressResponse {
                                status: Status::Ok,
                                addr: tcp.local,
                            })
                            .unwrap_or(TcpSocketGetLocalAddressResponse {
                                status: Status::ErrNotFound,
                                addr: empty_addr(),
                            })
                    } else {
                        TcpSocketGetLocalAddressResponse {
                            status: Status::ErrInvalidArgs,
                            addr: empty_addr(),
                        }
                    };
                    reply(channel, &response);
                }
                4 => {
                    let response = match TcpSocketShutdownRequest::decode(req, &handles) {
                        Ok(request) => {
                            let status = stack
                                .tcp_mut(control)
                                .and_then(|tcp| tcp.stream)
                                .map(|socket| {
                                    socket
                                        .shutdown(request.read, request.write)
                                        .map(|_| Status::Ok)
                                        .unwrap_or(Status::ErrInvalidHandle)
                                })
                                .unwrap_or(Status::ErrNotFound);
                            TcpSocketShutdownResponse { status }
                        }
                        Err(_) => TcpSocketShutdownResponse {
                            status: Status::ErrInvalidArgs,
                        },
                    };
                    reply(channel, &response);
                }
                5 => {
                    if TcpSocketCloseRequest::decode(req, &handles).is_ok() {
                        stack.remove_tcp(control);
                    }
                }
                _ => {}
            }
        }
    }
    changed
}

fn poll_listener_clients(stack: &mut Netstack) -> bool {
    let mut changed = false;
    let controls = stack
        .listeners
        .iter()
        .map(|listener| listener.control)
        .collect::<Vec<_>>();
    for control in controls {
        let channel = Channel(control);
        if let Ok(message) = channel.try_recv() {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            match ordinal {
                1 => {
                    let response = if TcpListenerAcceptRequest::decode(req, &handles).is_ok() {
                        match stack
                            .listener_mut(control)
                            .and_then(|listener| listener.accept().ok())
                        {
                            Some(client) => TcpListenerAcceptResponse {
                                status: Status::Ok,
                                client: HandleRef {
                                    raw: client.client_control.unwrap_or(client.control),
                                },
                                peer_addr: client.peer,
                            },
                            None => TcpListenerAcceptResponse {
                                status: Status::ErrShouldWait,
                                client: HandleRef { raw: 0 },
                                peer_addr: empty_addr(),
                            },
                        }
                    } else {
                        TcpListenerAcceptResponse {
                            status: Status::ErrInvalidArgs,
                            client: HandleRef { raw: 0 },
                            peer_addr: empty_addr(),
                        }
                    };
                    reply(channel, &response);
                }
                2 => {
                    if TcpListenerCloseRequest::decode(req, &handles).is_ok() {
                        stack.remove_listener(control);
                    }
                }
                3 => {
                    let (status, readable) =
                        if TcpListenerGetInfoRequest::decode(req, &handles).is_err() {
                            (Status::ErrInvalidArgs, false)
                        } else {
                            stack
                                .listener_mut(control)
                                .map(|listener| {
                                    (Status::Ok, listener.closed || !listener.pending.is_empty())
                                })
                                .unwrap_or((Status::ErrNotFound, false))
                        };
                    reply(channel, &TcpListenerGetInfoResponse { status, readable });
                }
                _ => {}
            }
        }
    }
    changed
}

fn poll_udp_clients(stack: &mut Netstack) -> bool {
    let mut changed = false;
    let controls = stack
        .udp
        .iter()
        .map(|socket| socket.control)
        .collect::<Vec<_>>();
    for control in controls {
        let channel = Channel(control);
        if let Ok(message) = channel.try_recv() {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            match ordinal {
                1 => {
                    let response = match UdpSocketSendToRequest::decode(req, &handles) {
                        Ok(request) => {
                            let (status, actual) = stack
                                .udp_mut(control)
                                .map(|udp| udp.send_to(request.data, request.destination))
                                .unwrap_or((Status::ErrNotFound, 0));
                            UdpSocketSendToResponse { status, actual }
                        }
                        Err(_) => UdpSocketSendToResponse {
                            status: Status::ErrInvalidArgs,
                            actual: 0,
                        },
                    };
                    reply(channel, &response);
                }
                2 => reply_udp_recv_from(channel, stack, control, req, &handles),
                3 => {
                    let response = match UdpSocketBindRequest::decode(req, &handles) {
                        Ok(request) => UdpSocketBindResponse {
                            status: stack
                                .udp_mut(control)
                                .map(|udp| udp.bind(request.local_addr))
                                .unwrap_or(Status::ErrNotFound),
                        },
                        Err(_) => UdpSocketBindResponse {
                            status: Status::ErrInvalidArgs,
                        },
                    };
                    reply(channel, &response);
                }
                4 => {
                    if UdpSocketCloseRequest::decode(req, &handles).is_ok() {
                        stack.remove_udp(control);
                    }
                }
                5 => {
                    let (status, readable, writable) =
                        if UdpSocketGetInfoRequest::decode(req, &handles).is_err() {
                            (Status::ErrInvalidArgs, false, false)
                        } else {
                            stack
                                .udp_mut(control)
                                .map(|udp| {
                                    (
                                        Status::Ok,
                                        udp.closed || !udp.queue.is_empty(),
                                        udp.closed
                                            || (udp.local.is_some() && udp.outbound.len() < 64),
                                    )
                                })
                                .unwrap_or((Status::ErrNotFound, false, false))
                        };
                    reply(
                        channel,
                        &UdpSocketGetInfoResponse {
                            status,
                            readable,
                            writable,
                        },
                    );
                }
                _ => {}
            }
        }
    }
    changed
}

fn reply_resolve_host(
    channel: Channel,
    stack: &mut Netstack,
    link: Option<&mut PacketLink>,
    req: &[u8],
    handles: &[HandleRef],
) {
    let request = match NetstackResolveHostRequest::decode(req, handles) {
        Ok(request) => request,
        Err(_) => {
            reply(
                channel,
                &NetstackResolveHostResponse {
                    status: Status::ErrInvalidArgs,
                    addresses: net_fidl::WireVector::from_slice(&[]),
                },
            );
            return;
        }
    };
    let mut owned = Vec::new();
    if let Some(records) = stack.resolve_cached(request.hostname) {
        for record in records {
            match record {
                DnsRecord::A(address) => {
                    owned.push(IpAddress::Ipv4(Ipv4Address { octets: *address }))
                }
                DnsRecord::Aaaa(address) => {
                    owned.push(IpAddress::Ipv6(Ipv6Address { octets: *address }))
                }
            }
        }
    } else if request.hostname == "localhost" {
        let _ = stack.cache_dns(request.hostname, &[[127, 0, 0, 1]]);
        owned.push(IpAddress::Ipv4(Ipv4Address {
            octets: [127, 0, 0, 1],
        }));
    }
    if owned.is_empty() {
        let status = stack.query_dns(request.hostname, link);
        reply(
            channel,
            &NetstackResolveHostResponse {
                status,
                addresses: net_fidl::WireVector::from_slice(&[]),
            },
        );
    } else {
        reply(
            channel,
            &NetstackResolveHostResponse {
                status: Status::Ok,
                addresses: net_fidl::WireVector::from_slice(&owned),
            },
        );
    }
}

fn reply_udp_recv_from(
    channel: Channel,
    stack: &mut Netstack,
    control: u64,
    req: &[u8],
    handles: &[HandleRef],
) {
    if UdpSocketRecvFromRequest::decode(req, handles).is_err() {
        reply(
            channel,
            &UdpSocketRecvFromResponse {
                status: Status::ErrInvalidArgs,
                data: &[],
                source: empty_addr(),
            },
        );
        return;
    }
    match stack.udp_mut(control).and_then(|udp| udp.recv_from().ok()) {
        Some(packet) => {
            let data = packet.data[..packet.len].to_vec();
            reply(
                channel,
                &UdpSocketRecvFromResponse {
                    status: Status::Ok,
                    data: &data,
                    source: packet.source,
                },
            );
        }
        None => {
            reply(
                channel,
                &UdpSocketRecvFromResponse {
                    status: Status::ErrShouldWait,
                    data: &[],
                    source: empty_addr(),
                },
            );
        }
    }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 8];
    if let Ok(encoded) = response.encode(&mut out, &mut handles) {
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        if let Err(error) = channel.send(&out[..encoded.bytes], &raw_handles) {
            log(&format!(
                "netstackd: response send failed endpoint={} error={error:?}\n",
                channel.0
            ));
        }
    } else {
        log("netstackd: response encode failed\n");
    }
}

fn handle_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}

fn metadata_protocol(metadata: &str) -> Option<&str> {
    metadata.split('|').nth(1)
}

fn log_ready(stack: &Netstack, driver_mtu: u32) {
    let source = match stack.source() {
        ConfigSource::Static => "static",
        ConfigSource::Dhcp => "dhcp",
        ConfigSource::Unconfigured => "unconfigured",
    };
    let _ = stack.smoltcp_epoch_millis();
    log(&format!(
        "netstackd: service ready config={} mtu={} driver_mtu={}\n",
        source, stack.config.mtu, driver_mtu
    ));
}
