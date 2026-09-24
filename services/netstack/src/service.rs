use alloc::format;
use alloc::vec::Vec;
use bexos_userspace::live_migration::Source;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Memory, Startup, log};
use net_fidl::{
    BackendControlKind, FidlDecode, FidlEncode, HandleRef, IpAddress, Ipv4Address, Ipv6Address,
    LinkStatus, LinkWatcherOnLinkStatusRequest, LinkWatcherPublicClient, NetstackConnectTcpRequest,
    NetstackConnectTcpResponse, NetstackCreateUdpSocketRequest, NetstackCreateUdpSocketResponse,
    NetstackGetLinkStatusRequest, NetstackGetLinkStatusResponse, NetstackListenTcpRequest,
    NetstackListenTcpResponse, NetstackResolveHostRequest, NetstackResolveHostResponse,
    NetstackWatchLinkStatusRequest, NetstackWatchLinkStatusResponse,
    StackBackendAdoptRecoveryRequest, StackBackendAdoptRecoveryResponse,
    StackBackendCheckpointRecoveryRequest, StackBackendCheckpointRecoveryResponse,
    StackBackendCloseRequest, StackBackendCloseResponse, StackBackendConnectTcpRequest,
    StackBackendConnectTcpResponse, StackBackendCreateUdpSocketRequest,
    StackBackendCreateUdpSocketResponse, StackBackendListenTcpRequest,
    StackBackendListenTcpResponse, StackBackendRecoverConnectionRequest,
    StackBackendRecoverConnectionResponse, StackBackendRecoverControlRequest,
    StackBackendRecoverControlResponse, StackBackendResolveHostRequest,
    StackBackendResolveHostResponse, StackControllerAddRouteRequest,
    StackControllerAddRouteResponse, StackControllerAttachInterfaceRequest,
    StackControllerAttachInterfaceResponse, StackControllerCreateTableRequest,
    StackControllerCreateTableResponse, StackControllerDetachInterfaceRequest,
    StackControllerDetachInterfaceResponse, StackControllerRemoveRouteRequest,
    StackControllerRemoveRouteResponse, StackControllerRemoveTableRequest,
    StackControllerRemoveTableResponse, Status, TcpListenerAcceptRequest,
    TcpListenerAcceptResponse, TcpListenerCloseRequest, TcpListenerGetInfoRequest,
    TcpListenerGetInfoResponse, TcpSocketCloseRequest, TcpSocketGetLocalAddressRequest,
    TcpSocketGetLocalAddressResponse, TcpSocketGetPeerAddressRequest,
    TcpSocketGetPeerAddressResponse, TcpSocketGetStreamRequest, TcpSocketGetStreamResponse,
    TcpSocketShutdownRequest, TcpSocketShutdownResponse, UdpSocketBindRequest,
    UdpSocketBindResponse, UdpSocketCloseRequest, UdpSocketGetInfoRequest,
    UdpSocketGetInfoResponse, UdpSocketRecvFromRequest, UdpSocketRecvFromResponse,
    UdpSocketSendToRequest, UdpSocketSendToResponse,
};

use crate::config::{ConfigSource, NetConfig};
use crate::dns::DnsRecord;
use crate::ethernet;
use crate::link::PacketLink;
use crate::migration::{NodeLink, Runtime};
use crate::router::{FibRoute as RouterFibRoute, InterfaceState, Router, TableQuota};
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
    let mut ethernet_endpoints: Vec<(u64, u64)> = startup
        .service_grants
        .iter()
        .filter(|grant| grant.service == "bexos.hardware.ethernet.Device")
        .map(|grant| {
            (
                grant
                    .provider_instance_id
                    .as_deref()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(grant.endpoint),
                grant.endpoint,
            )
        })
        .collect();
    ethernet_endpoints.sort_by_key(|entry| entry.0);
    ethernet_endpoints.dedup_by_key(|entry| entry.0);
    let tls_trust = startup
        .service_grants
        .iter()
        .find(|grant| grant.service == "bexos.security.trust.TlsTrustManager")
        .map(|grant| Channel(grant.endpoint));
    let links: Vec<NodeLink> = ethernet_endpoints
        .into_iter()
        .take(16)
        .filter_map(|(node_id, endpoint)| {
            let link = ethernet::connect(endpoint)
                .map_err(|error| {
                    log(&format!(
                        "netstackd: Ethernet connection failed: {error:?}\n"
                    ));
                })
                .ok()?;
            PacketLink::new(link)
                .map(|link| NodeLink { node_id, link })
                .map_err(|error| {
                    log(&format!(
                        "netstackd: Ethernet packet mapping failed: {error:?}\n"
                    ));
                })
                .ok()
        })
        .collect();
    let active = config.clone().choose_active(None);
    let stack = Netstack::new(config, None);
    log_ready(
        &stack,
        links
            .first()
            .map_or(0, |node_link| node_link.link.resources.mtu),
    );
    Startup::ready(control).unwrap();
    serve(Runtime {
        control,
        migration: startup.migration,
        tls_trust,
        clients: Vec::new(),
        backend_clients: Vec::new(),
        controller_clients: Vec::new(),
        backend_connections: alloc::collections::BTreeMap::new(),
        backend_controls: alloc::collections::BTreeMap::new(),
        link_watchers: Vec::new(),
        stack,
        router: Router::new(active),
        links,
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
            .links
            .iter_mut()
            .any(|link| !link.link.drain_for_quiesce())
        {
            source.changed(0);
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        let had_links = runtime.links.len();
        bexos_trace::trace_scope!(
            bexos_trace::CATEGORY_NETWORK_STACK,
            "netstack:poll_packet_plane"
        );
        if let Some((primary, secondary)) = runtime.links.split_first_mut() {
            runtime.stack.poll_packet_plane(Some(&mut primary.link));
            for node_link in secondary {
                runtime
                    .stack
                    .poll_secondary_packet_plane(&mut node_link.link);
            }
        }
        runtime.router.poll_packet_planes();
        if had_links != runtime.links.len() {
            notify_link_watchers(
                &mut runtime.link_watchers,
                runtime.links.first().map(|link| &link.link),
            );
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
                    } else if binding.protocol_is("StackBackend")
                        && binding.caller_package.as_deref() == Some("bexos.service.networkd")
                    {
                        runtime
                            .backend_clients
                            .push(BoundServiceEndpoint::new_with_protocol(
                                Channel(endpoint),
                                binding.method_ordinals,
                                "StackBackend",
                            ));
                        source.changed(0);
                    } else if binding.protocol_is("StackController")
                        && binding.caller_package.as_deref() == Some("bexos.service.networkd")
                    {
                        runtime
                            .controller_clients
                            .push(BoundServiceEndpoint::new_with_protocol(
                                Channel(endpoint),
                                binding.method_ordinals,
                                "StackController",
                            ));
                        source.changed(0);
                    } else {
                        let _ = bexos_userspace::Memory::close(endpoint);
                    }
                } else if metadata_protocol(metadata) == Some("Netstack") {
                    let _ = bexos_userspace::Memory::close(endpoint);
                }
            }
        }
        // Every class must be polled even when another one made progress.
        // Short-circuiting here lets repeated directory/DNS activity starve
        // TCP setup, stream control and cancellation indefinitely.
        let mut changed = {
            let Runtime {
                clients,
                link_watchers,
                router,
                ..
            } = &mut runtime;
            let default_table = router.tables.get_mut(&0).expect("default VRF table");
            poll_netstack_clients(
                clients,
                link_watchers,
                &mut default_table.stack,
                default_table.links.values_mut().next(),
            )
        };
        changed |= poll_backend_clients(&mut runtime);
        changed |= poll_controller_clients(&mut runtime);
        for table in runtime.router.tables.values_mut() {
            changed |= poll_tcp_clients(&mut table.stack);
            changed |= poll_listener_clients(&mut table.stack);
            changed |= poll_udp_clients(&mut table.stack);
        }
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

fn poll_backend_clients(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut clients = core::mem::take(&mut runtime.backend_clients);
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if !client.allows(ordinal) {
                close_handles(&message.handles);
                return true;
            }
            match ordinal {
                1 => {
                    let response = StackBackendConnectTcpRequest::decode(req, &handles)
                        .map(|request| {
                            let status = runtime.router.connect_tcp_with_stream(
                                request.table.value,
                                request.connection_id,
                                request.remote_addr,
                                request.stream.raw,
                            );
                            if status == Status::Ok {
                                runtime
                                    .backend_connections
                                    .insert(request.connection_id, request.table.value);
                            }
                            StackBackendConnectTcpResponse { status }
                        })
                        .unwrap_or(StackBackendConnectTcpResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                2 => {
                    let response = StackBackendListenTcpRequest::decode(req, &handles)
                        .map(|request| {
                            let status = runtime.router.listen_tcp(
                                request.table.value,
                                request.listener.raw,
                                request.local_addr,
                            );
                            if status == Status::Ok {
                                runtime
                                    .backend_connections
                                    .insert(request.listener_id, request.table.value);
                                runtime
                                    .backend_controls
                                    .insert(request.listener_id, (1, request.listener.raw));
                            } else {
                                let _ = bexos_userspace::Memory::close(request.listener.raw);
                            }
                            StackBackendListenTcpResponse { status }
                        })
                        .unwrap_or(StackBackendListenTcpResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                3 => {
                    let response = StackBackendCloseRequest::decode(req, &handles)
                        .map(|request| {
                            let known = runtime.backend_connections.remove(&request.object_id);
                            runtime.backend_controls.remove(&request.object_id);
                            let mut status = if runtime.stack.remove_backend_tcp(request.object_id)
                            {
                                Status::Ok
                            } else {
                                runtime.router.close_object(request.object_id)
                            };
                            if status == Status::ErrNotFound && known.is_some() {
                                status = Status::Ok;
                            }
                            StackBackendCloseResponse { status }
                        })
                        .unwrap_or(StackBackendCloseResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                4 => {
                    let response = match StackBackendAdoptRecoveryRequest::decode(req, &handles) {
                        Ok(request) => {
                            let adopted = crate::recovery::adopt(
                                runtime,
                                request.journal.raw,
                                request.journal_len,
                                request.expected_generation,
                            );
                            let status = adopted.map_or_else(|status| status, |_| Status::Ok);
                            let _ = bexos_userspace::Memory::close(request.journal.raw);
                            StackBackendAdoptRecoveryResponse {
                                status,
                                adopted_generation: adopted.unwrap_or(0),
                            }
                        }
                        Err(_) => StackBackendAdoptRecoveryResponse {
                            status: Status::ErrInvalidArgs,
                            adopted_generation: 0,
                        },
                    };
                    reply(client.channel, &response);
                }
                5 => {
                    let response = StackBackendCreateUdpSocketRequest::decode(req, &handles)
                        .map(|request| {
                            let status = runtime
                                .router
                                .create_udp(request.table.value, request.socket.raw);
                            if status == Status::Ok {
                                runtime
                                    .backend_connections
                                    .insert(request.object_id, request.table.value);
                                runtime
                                    .backend_controls
                                    .insert(request.object_id, (2, request.socket.raw));
                            } else {
                                let _ = bexos_userspace::Memory::close(request.socket.raw);
                            }
                            StackBackendCreateUdpSocketResponse { status }
                        })
                        .unwrap_or(StackBackendCreateUdpSocketResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                6 => match StackBackendResolveHostRequest::decode(req, &handles) {
                    Ok(request) => {
                        let response = runtime
                            .router
                            .table_mut(request.table.value)
                            .map(|table| {
                                let mut interfaces =
                                    table.links.keys().copied().collect::<Vec<_>>();
                                interfaces.sort_unstable();
                                let link = interfaces
                                    .first()
                                    .and_then(|interface| table.links.get_mut(interface));
                                resolve_host(&mut table.stack, link, request.hostname)
                            })
                            .unwrap_or_else(|| (Status::ErrNotFound, Vec::new()));
                        reply(
                            client.channel,
                            &StackBackendResolveHostResponse {
                                status: response.0,
                                addresses: net_fidl::WireVector::from_slice(&response.1),
                            },
                        );
                    }
                    Err(_) => reply(
                        client.channel,
                        &StackBackendResolveHostResponse {
                            status: Status::ErrInvalidArgs,
                            addresses: net_fidl::WireVector::from_slice(&[]),
                        },
                    ),
                },
                7 => {
                    let response =
                        match StackBackendCheckpointRecoveryRequest::decode(req, &handles) {
                            Ok(request) if runtime.generation >= request.minimum_generation => {
                                let previous = runtime.generation;
                                runtime.generation = runtime.generation.saturating_add(1);
                                match crate::recovery::checkpoint(runtime, runtime.generation) {
                                    Ok((journal, journal_len)) => {
                                        StackBackendCheckpointRecoveryResponse {
                                            status: Status::Ok,
                                            journal: HandleRef { raw: journal },
                                            journal_len,
                                            generation: runtime.generation,
                                        }
                                    }
                                    Err(status) => {
                                        runtime.generation = previous;
                                        StackBackendCheckpointRecoveryResponse {
                                            status,
                                            journal: HandleRef { raw: 0 },
                                            journal_len: 0,
                                            generation: previous,
                                        }
                                    }
                                }
                            }
                            Ok(_) => StackBackendCheckpointRecoveryResponse {
                                status: Status::ErrShouldWait,
                                journal: HandleRef { raw: 0 },
                                journal_len: 0,
                                generation: runtime.generation,
                            },
                            Err(_) => StackBackendCheckpointRecoveryResponse {
                                status: Status::ErrInvalidArgs,
                                journal: HandleRef { raw: 0 },
                                journal_len: 0,
                                generation: runtime.generation,
                            },
                        };
                    reply(client.channel, &response);
                }
                8 => {
                    let response = StackBackendRecoverConnectionRequest::decode(req, &handles)
                        .map(|request| {
                            let status = runtime.router.recover_connection(
                                request.table.value,
                                request.connection_id,
                                request.stream.raw,
                            );
                            if status == Status::Ok {
                                runtime
                                    .backend_connections
                                    .insert(request.connection_id, request.table.value);
                            } else {
                                let _ = Memory::close(request.stream.raw);
                            }
                            StackBackendRecoverConnectionResponse { status }
                        })
                        .unwrap_or(StackBackendRecoverConnectionResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                9 => {
                    let response = StackBackendRecoverControlRequest::decode(req, &handles)
                        .map(|request| {
                            let expected_kind = match request.kind {
                                BackendControlKind::TcpListener => 1,
                                BackendControlKind::UdpSocket => 2,
                            };
                            let status = match (
                                runtime.backend_connections.get(&request.object_id).copied(),
                                runtime.backend_controls.get(&request.object_id).copied(),
                            ) {
                                (Some(table), Some((kind, old_control)))
                                    if table == request.table.value && kind == expected_kind =>
                                {
                                    runtime.router.recover_control(
                                        table,
                                        old_control,
                                        kind,
                                        request.control.raw,
                                    )
                                }
                                _ => Status::ErrNotFound,
                            };
                            if status == Status::Ok {
                                runtime.backend_controls.insert(
                                    request.object_id,
                                    (expected_kind, request.control.raw),
                                );
                            } else {
                                let _ = Memory::close(request.control.raw);
                            }
                            StackBackendRecoverControlResponse { status }
                        })
                        .unwrap_or(StackBackendRecoverControlResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                _ => close_handles(&message.handles),
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
    runtime.backend_clients = clients;
    changed
}

fn poll_controller_clients(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut clients = core::mem::take(&mut runtime.controller_clients);
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if !client.allows(ordinal) {
                close_handles(&message.handles);
                return true;
            }
            match ordinal {
                1 => {
                    let response = StackControllerCreateTableRequest::decode(req, &handles)
                        .map(|request| StackControllerCreateTableResponse {
                            status: runtime
                                .router
                                .create_table(request.table.value, TableQuota::default()),
                        })
                        .unwrap_or(StackControllerCreateTableResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                2 => {
                    let response = StackControllerRemoveTableRequest::decode(req, &handles)
                        .map(|request| StackControllerRemoveTableResponse {
                            status: runtime.router.remove_table(request.table.value),
                        })
                        .unwrap_or(StackControllerRemoveTableResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                3 => {
                    let response = StackControllerAddRouteRequest::decode(req, &handles)
                        .map(|request| StackControllerAddRouteResponse {
                            status: runtime
                                .router
                                .add_route(request.route.table.value, router_route(request.route)),
                        })
                        .unwrap_or(StackControllerAddRouteResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                4 => {
                    let response = StackControllerRemoveRouteRequest::decode(req, &handles)
                        .map(|request| StackControllerRemoveRouteResponse {
                            status: runtime.router.remove_route(
                                request.route.table.value,
                                router_route(request.route),
                            ),
                        })
                        .unwrap_or(StackControllerRemoveRouteResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                5 => {
                    let response =
                        match StackControllerAttachInterfaceRequest::decode(req, &handles) {
                            Ok(request) => {
                                let link = ethernet::connect(request.device.raw)
                                    .map_err(|_| Status::ErrInvalidArgs)
                                    .and_then(PacketLink::new);
                                let status = link
                                    .map(|link| {
                                        runtime.router.attach_link(
                                            request.table.value,
                                            InterfaceState {
                                                id: request.interface_id,
                                                name: request.interface_name.into(),
                                                up: true,
                                                neighbor_generation: 0,
                                                packet_generation: runtime.generation,
                                            },
                                            link,
                                        )
                                    })
                                    .unwrap_or(Status::ErrInvalidArgs);
                                StackControllerAttachInterfaceResponse { status }
                            }
                            Err(_) => StackControllerAttachInterfaceResponse {
                                status: Status::ErrInvalidArgs,
                            },
                        };
                    reply(client.channel, &response);
                }
                6 => {
                    let response = StackControllerDetachInterfaceRequest::decode(req, &handles)
                        .map(|request| StackControllerDetachInterfaceResponse {
                            status: runtime
                                .router
                                .detach_interface(request.table.value, request.interface_id),
                        })
                        .unwrap_or(StackControllerDetachInterfaceResponse {
                            status: Status::ErrInvalidArgs,
                        });
                    reply(client.channel, &response);
                }
                _ => close_handles(&message.handles),
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
    runtime.controller_clients = clients;
    changed
}

fn router_route(route: net_fidl::FibRoute) -> RouterFibRoute {
    RouterFibRoute {
        destination: route.destination.network,
        prefix_len: route.destination.prefix_len,
        gateway: route.has_gateway.then_some(route.gateway),
        interface_id: route.interface_id,
        metric: route.metric,
    }
}

fn close_handles(handles: &[u64]) {
    for handle in handles {
        let _ = bexos_userspace::Memory::close(*handle);
    }
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
        match channel.try_recv() {
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                stack.remove_listener(control);
                let _ = bexos_userspace::Memory::close(control);
                changed = true;
            }
            Err(_) => {}
            Ok(message) => {
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
                        let (status, readable) = if TcpListenerGetInfoRequest::decode(req, &handles)
                            .is_err()
                        {
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
        match channel.try_recv() {
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                stack.remove_udp(control);
                let _ = bexos_userspace::Memory::close(control);
                changed = true;
            }
            Err(_) => {}
            Ok(message) => {
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
                                status: stack.bind_udp(control, request.local_addr),
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
    let (status, owned) = resolve_host(stack, link, request.hostname);
    reply(
        channel,
        &NetstackResolveHostResponse {
            status,
            addresses: net_fidl::WireVector::from_slice(&owned),
        },
    );
}

fn resolve_host(
    stack: &mut Netstack,
    link: Option<&mut PacketLink>,
    hostname: &str,
) -> (Status, Vec<IpAddress>) {
    let mut owned = Vec::new();
    if let Some(records) = stack.resolve_cached(hostname) {
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
    } else if hostname == "localhost" {
        let _ = stack.cache_dns(hostname, &[[127, 0, 0, 1]]);
        owned.push(IpAddress::Ipv4(Ipv4Address {
            octets: [127, 0, 0, 1],
        }));
    }
    if owned.is_empty() {
        (stack.query_dns(hostname, link), owned)
    } else {
        (Status::Ok, owned)
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
