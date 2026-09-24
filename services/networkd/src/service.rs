use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use std::io::{Read, Write};

use bexos_userspace::{
    Channel, Memory, Rpc, Socket, Startup,
    live_migration::{RecordChanges, Source},
    log,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use net_fidl::{
    BackendControlKind, DnsTransport as WireDnsTransport, DomainEndpoint, Endpoint, FidlDecode,
    FidlEncode, HandleRef, IpAddress as WireIpAddress, NetstackConnectTcpRequest,
    NetstackConnectTcpResponse, NetstackCreateUdpSocketRequest, NetstackCreateUdpSocketResponse,
    NetstackGetLinkStatusRequest, NetstackGetLinkStatusResponse, NetstackListenTcpRequest,
    NetstackListenTcpResponse, NetstackPublicClient, NetstackResolveHostRequest,
    NetstackResolveHostResponse, NetstackWatchLinkStatusRequest, NetstackWatchLinkStatusResponse,
    NetworkRoutingManagerGetScopedSocketProviderRequest,
    NetworkRoutingManagerGetScopedSocketProviderResponse,
    NetworkRoutingManagerListEgressProvidersRequest,
    NetworkRoutingManagerListEgressProvidersResponse,
    NetworkRoutingManagerRegisterEgressProviderRequest,
    NetworkRoutingManagerRegisterEgressProviderResponse,
    NetworkRoutingManagerUnregisterEgressProviderRequest,
    NetworkRoutingManagerUnregisterEgressProviderResponse,
    NetworkRoutingManagerUpdateEgressProviderRequest,
    NetworkRoutingManagerUpdateEgressProviderResponse, ProviderInfo, ProviderRouteConfig,
    ProxyStreamHandlerConnectRequest, ProxyStreamHandlerPublicClient, RouteTargetKind,
    SocketAddress, SocketProviderConnectTcpRequest, SocketProviderConnectTcpResponse,
    SocketProviderCreateUdpSocketRequest, SocketProviderCreateUdpSocketResponse,
    SocketProviderGetLinkStatusRequest, SocketProviderGetLinkStatusResponse,
    SocketProviderListenTcpRequest, SocketProviderListenTcpResponse,
    SocketProviderResolveHostRequest, SocketProviderResolveHostResponse,
    SocketProviderWatchLinkStatusRequest, SocketProviderWatchLinkStatusResponse,
    StackBackendAdoptRecoveryRequest, StackBackendCheckpointRecoveryRequest,
    StackBackendConnectTcpRequest, StackBackendCreateUdpSocketRequest,
    StackBackendListenTcpRequest, StackBackendPublicClient, StackBackendRecoverConnectionRequest,
    StackBackendRecoverControlRequest, StackBackendResolveHostRequest,
    StackControllerAddRouteRequest, StackControllerAttachInterfaceRequest,
    StackControllerCreateTableRequest, StackControllerPublicClient, Status, TableId as WireTableId,
    TcpSocketCloseRequest, TcpSocketCloseResponse, TcpSocketGetLocalAddressRequest,
    TcpSocketGetLocalAddressResponse, TcpSocketGetPeerAddressRequest,
    TcpSocketGetPeerAddressResponse, TcpSocketGetStreamRequest, TcpSocketGetStreamResponse,
    TcpSocketShutdownRequest, TcpSocketShutdownResponse, UdpSocketBindRequest,
    UdpSocketPublicClient, UdpSocketRecvFromRequest, UdpSocketSendToRequest,
    VirtualSwitchControllerCommitGenerationRequest, VirtualSwitchControllerCreatePortRequest,
    VirtualSwitchControllerPublicClient, VirtualSwitchControllerRecoverGenerationRequest,
    WireVector,
};

use crate::config::BootConfig;
use crate::dns::{CacheKey, CacheValue, DnsError, RecordType, ResponseCode};
use crate::migration::{ControlProxy, EscrowedBackend, ProxyControl, RecoveryJournal, Runtime};
use crate::routing::{
    Destination, DnsTransport, DnsUpstream, IpAddress, IpPrefix, ProviderConfig, RouteLease,
    RoutingError, Target,
};

struct BackendObjectGuard {
    backend: u64,
    object_id: u64,
    local: Option<u64>,
}

impl Drop for BackendObjectGuard {
    fn drop(&mut self) {
        if let Some(local) = self.local.take() {
            let _ = Memory::close(local);
        }
        close_backend_object(self.backend, self.object_id);
    }
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("networkd startup");
    let mut runtime = if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(runtime) => runtime,
            Err(_) => bexos_userspace::exit(),
        }
    } else {
        let config = BootConfig::from_startup(&startup).unwrap_or_else(|_| {
            log("networkd: invalid or missing authoritative boot policy\n");
            bexos_userspace::exit()
        });
        let mut runtime = Runtime::new(control, startup.migration);
        runtime.instance_id = config.instance_id.clone();
        runtime.routing = crate::routing::Registry::new(config.max_dynamic_providers);
        runtime.dns = crate::dns::ResolverState::new(config.dns_cache_capacity);
        for domain in &config.domains {
            runtime
                .domain_tables
                .insert(domain.name.clone(), domain.table);
        }
        for grant in &startup.service_grants {
            if grant.service == "bexos.net.NetstackBackend" {
                let instance = grant
                    .provider_instance_id
                    .clone()
                    .unwrap_or_else(|| "system_default".to_string());
                runtime.backend_channels.insert(instance, grant.endpoint);
            } else if grant.service == "bexos.net.StackBackend" {
                let instance = grant
                    .provider_instance_id
                    .clone()
                    .unwrap_or_else(|| "system_default".to_string());
                runtime
                    .stack_backend_channels
                    .insert(instance, grant.endpoint);
            } else if grant.service == "bexos.net.StackController" {
                let instance = grant
                    .provider_instance_id
                    .clone()
                    .unwrap_or_else(|| "system_default".to_string());
                runtime
                    .stack_controller_channels
                    .insert(instance, grant.endpoint);
            } else if grant.service == "bexos.net.VirtualSwitchController" {
                runtime.vswitch_controller = Some(grant.endpoint);
            } else if grant.service == "bexos.security.trust.TlsTrustManager" {
                runtime.tls_trust = Some(grant.endpoint);
            }
        }
        if apply_boot_topology(&mut runtime, &config).is_err() {
            log("networkd: boot topology application failed\n");
            bexos_userspace::exit();
        }
        Startup::ready(control).expect("networkd ready");
        runtime
    };
    log("networkd: ready\n");
    let mut source = Source::new(runtime.migration);
    let mut changes = RecordChanges::default();
    loop {
        changes.poll(&runtime, &mut source);
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        accept_bindings(&mut runtime);
        let mut changed = poll_clients(&mut runtime);
        changed |= poll_control_proxies(&mut runtime);
        changed |= poll_proxy_controls(&mut runtime);
        changed |= checkpoint_backends(&mut runtime);
        if changed {
            source.changed_keys([0, 1, 2, 3, 4]);
        }
        bexos_userspace::yield_now();
    }
}

fn apply_boot_topology(runtime: &mut Runtime, config: &BootConfig) -> Result<(), Status> {
    let controller = runtime
        .stack_controller_channels
        .get(&config.instance_id)
        .or_else(|| runtime.stack_controller_channels.values().next())
        .copied()
        .ok_or(Status::ErrShouldWait)?;
    let backend_instance = runtime
        .stack_backend_channels
        .keys()
        .find(|instance| *instance == &config.instance_id)
        .or_else(|| runtime.stack_backend_channels.keys().next())
        .cloned()
        .ok_or(Status::ErrShouldWait)?;
    let mut stack = StackControllerPublicClient::new(Rpc(Channel(controller)));
    for table in config.tables.iter().copied().filter(|table| *table != 0) {
        let response = stack
            .create_table(
                &StackControllerCreateTableRequest {
                    table: WireTableId { value: table },
                },
                &mut [0; 128],
                &mut [HandleRef { raw: 0 }; 1],
                &mut [0; 128],
                &mut [HandleRef { raw: 0 }; 1],
            )
            .map_err(|_| Status::ErrShouldWait)?;
        if response.status != Status::Ok && response.status != Status::ErrAlreadyExists {
            return Err(response.status);
        }
    }
    if !config.ports.is_empty() {
        let switch = runtime.vswitch_controller.ok_or(Status::ErrShouldWait)?;
        let mut switch = VirtualSwitchControllerPublicClient::new(Rpc(Channel(switch)));
        for port in &config.ports {
            let response = switch
                .create_port(
                    &VirtualSwitchControllerCreatePortRequest {
                        port_id: port.id,
                        physical_interface: port.physical_interface,
                        mac: port.mac,
                        vlan_id: port.vlan_id,
                        tagged: port.tagged,
                        rx_queue_depth: port.rx_queue_depth,
                        tx_queue_depth: port.tx_queue_depth,
                    },
                    &mut [0; 256],
                    &mut [HandleRef { raw: 0 }; 2],
                    &mut [0; 128],
                    &mut [HandleRef { raw: 0 }; 2],
                )
                .map_err(|_| Status::ErrShouldWait)?;
            if response.status != Status::Ok {
                if response.device.raw != 0 {
                    let _ = Memory::close(response.device.raw);
                }
                return Err(response.status);
            }
            runtime.virtual_ports.push(port.id);
            let interface_name = alloc::format!("vport{}", port.id);
            let response = stack
                .attach_interface(
                    &StackControllerAttachInterfaceRequest {
                        table: WireTableId { value: port.table },
                        interface_id: port.id,
                        interface_name: &interface_name,
                        device: response.device,
                    },
                    &mut [0; 256],
                    &mut [HandleRef { raw: 0 }; 2],
                    &mut [0; 128],
                    &mut [HandleRef { raw: 0 }; 1],
                )
                .map_err(|_| Status::ErrShouldWait)?;
            if response.status != Status::Ok {
                return Err(response.status);
            }
        }
    }
    runtime.virtual_ports.sort_unstable();
    runtime.virtual_ports.dedup();
    for route in &config.routes {
        let response = stack
            .add_route(
                &StackControllerAddRouteRequest {
                    route: net_fidl::FibRoute {
                        table: WireTableId { value: route.table },
                        destination: net_fidl::IpSubnet {
                            network: wire_address(route.destination),
                            prefix_len: route.prefix_len,
                        },
                        gateway: wire_address(route.gateway.unwrap_or(route.destination)),
                        has_gateway: route.gateway.is_some(),
                        interface_id: route.interface_id,
                        metric: route.metric,
                    },
                },
                &mut [0; 256],
                &mut [HandleRef { raw: 0 }; 1],
                &mut [0; 128],
                &mut [HandleRef { raw: 0 }; 1],
            )
            .map_err(|_| Status::ErrShouldWait)?;
        if response.status != Status::Ok && response.status != Status::ErrAlreadyExists {
            return Err(response.status);
        }
    }
    for table in &config.tables {
        let routes = config
            .routes
            .iter()
            .filter(|route| route.table == *table)
            .collect::<Vec<_>>();
        if routes.is_empty() {
            return Err(Status::ErrNetworkUnreachable);
        }
        let mut provider_names = config
            .upstreams
            .iter()
            .filter(|upstream| upstream.table == *table)
            .map(|upstream| upstream.provider.clone())
            .collect::<Vec<_>>();
        if provider_names.is_empty() {
            provider_names.push(alloc::format!("table-{table}"));
        }
        provider_names.sort();
        provider_names.dedup();
        for provider_name in provider_names {
            let upstreams = config
                .upstreams
                .iter()
                .filter(|upstream| upstream.table == *table && upstream.provider == provider_name)
                .collect::<Vec<_>>();
            let default_route = upstreams.is_empty()
                || upstreams
                    .iter()
                    .any(|upstream| upstream.domain_suffix == ".");
            let priority = upstreams
                .iter()
                .map(|upstream| upstream.priority)
                .min()
                .unwrap_or(100);
            let domain_routes = upstreams
                .iter()
                .filter(|upstream| upstream.domain_suffix != ".")
                .map(|upstream| upstream.domain_suffix.clone())
                .collect();
            let dns_upstreams = upstreams
                .iter()
                .map(|upstream| DnsUpstream {
                    address: upstream.address,
                    port: upstream.port,
                    transport: upstream.transport,
                    tls_server_name: upstream.tls_server_name.clone(),
                    doh_path: upstream.doh_path.clone(),
                })
                .collect();
            runtime
                .routing
                .register(
                    alloc::format!("boot-{provider_name}-{table}"),
                    ProviderConfig {
                        target: Target::Device {
                            stack_instance: backend_instance.clone(),
                            table: *table,
                            interface_id: routes[0].interface_id,
                        },
                        ip_routes: routes
                            .iter()
                            .map(|route| IpPrefix::new(route.destination, route.prefix_len))
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(routing_status)?,
                        domain_routes,
                        dns_upstreams,
                        priority,
                        default_route,
                    },
                )
                .map_err(routing_status)?;
        }
    }
    Ok(())
}

fn accept_bindings(runtime: &mut Runtime) {
    if let Ok(message) = runtime.control.try_recv() {
        let Some(endpoint) = message.handles.first().copied() else {
            return;
        };
        let Ok(metadata) = core::str::from_utf8(&message.bytes) else {
            let _ = Memory::close(endpoint);
            return;
        };
        let Some(binding) = ServiceBinding::parse(metadata) else {
            let _ = Memory::close(endpoint);
            return;
        };
        if matches!(
            binding.protocol.as_str(),
            "Netstack" | "SocketProvider" | "NetworkRoutingManager"
        ) {
            if let Some(domain) = binding
                .permission_values
                .iter()
                .find_map(|value| value.strip_prefix("network.domain="))
            {
                runtime.client_domains.insert(endpoint, domain.to_string());
            }
            runtime
                .clients
                .push(BoundServiceEndpoint::new_with_protocol(
                    Channel(endpoint),
                    binding.method_ordinals,
                    &binding.protocol,
                ));
        } else {
            let _ = Memory::close(endpoint);
        }
    }
}

fn poll_clients(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut clients = core::mem::take(&mut runtime.clients);
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, request) = envelope(&message.bytes);
            if !client.allows(ordinal) {
                close_handles(&message.handles);
                return true;
            }
            let handles = handle_refs(&message.handles);
            match client.protocol.as_str() {
                "Netstack" => poll_legacy(runtime, client.channel, ordinal, request, &handles),
                "SocketProvider" => {
                    poll_scoped(runtime, client.channel, ordinal, request, &handles)
                }
                "NetworkRoutingManager" => {
                    poll_routing_manager(runtime, client.channel, ordinal, request, &handles)
                }
                _ => close_handles(&message.handles),
            }
            true
        }
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            changed = true;
            runtime.client_domains.remove(&client.channel.0);
            let _ = Memory::close(client.channel.0);
            false
        }
        Err(_) => true,
    });
    clients.append(&mut runtime.clients);
    runtime.clients = clients;
    changed
}

fn poll_routing_manager(
    runtime: &mut Runtime,
    channel: Channel,
    ordinal: u64,
    request: &[u8],
    handles: &[HandleRef],
) {
    match ordinal {
        1 => {
            let response = match NetworkRoutingManagerRegisterEgressProviderRequest::decode(
                request, handles,
            ) {
                Ok(request) => match provider_config(request.config) {
                    Ok(config) => match runtime.routing.register(request.name, config) {
                        Ok(provider_token) => NetworkRoutingManagerRegisterEgressProviderResponse {
                            status: Status::Ok,
                            provider_token,
                        },
                        Err(error) => NetworkRoutingManagerRegisterEgressProviderResponse {
                            status: routing_status(error),
                            provider_token: 0,
                        },
                    },
                    Err(status) => NetworkRoutingManagerRegisterEgressProviderResponse {
                        status,
                        provider_token: 0,
                    },
                },
                Err(_) => NetworkRoutingManagerRegisterEgressProviderResponse {
                    status: Status::ErrInvalidArgs,
                    provider_token: 0,
                },
            };
            reply(channel, &response);
        }
        2 => {
            let response =
                match NetworkRoutingManagerUpdateEgressProviderRequest::decode(request, handles) {
                    Ok(request) => NetworkRoutingManagerUpdateEgressProviderResponse {
                        status: provider_config(request.config)
                            .and_then(|config| {
                                runtime
                                    .routing
                                    .update(request.provider_token, config)
                                    .map(|_| ())
                                    .map_err(routing_status)
                            })
                            .map_or_else(|status| status, |_| Status::Ok),
                    },
                    Err(_) => NetworkRoutingManagerUpdateEgressProviderResponse {
                        status: Status::ErrInvalidArgs,
                    },
                };
            reply(channel, &response);
        }
        3 => {
            let response =
                NetworkRoutingManagerUnregisterEgressProviderRequest::decode(request, handles)
                    .map(
                        |request| NetworkRoutingManagerUnregisterEgressProviderResponse {
                            status: runtime
                                .routing
                                .remove(request.provider_token)
                                .map_or_else(routing_status, |_| Status::Ok),
                        },
                    )
                    .unwrap_or(NetworkRoutingManagerUnregisterEgressProviderResponse {
                        status: Status::ErrInvalidArgs,
                    });
            reply(channel, &response);
        }
        4 => {
            if NetworkRoutingManagerListEgressProvidersRequest::decode(request, handles).is_err() {
                reply(
                    channel,
                    &NetworkRoutingManagerListEgressProvidersResponse {
                        status: Status::ErrInvalidArgs,
                        providers: WireVector::from_slice(&[]),
                    },
                );
                return;
            }
            let providers = runtime
                .routing
                .providers()
                .map(|provider| {
                    let (table, kind) = match provider.config.target {
                        Target::Device { table, .. } => {
                            (WireTableId { value: table }, RouteTargetKind::Device)
                        }
                        Target::Proxy { .. } => (WireTableId { value: 0 }, RouteTargetKind::Proxy),
                    };
                    ProviderInfo {
                        token: provider.token,
                        name: provider.name.as_str(),
                        table,
                        kind,
                        priority: provider.config.priority,
                        active: provider.accepting,
                    }
                })
                .collect::<Vec<_>>();
            reply(
                channel,
                &NetworkRoutingManagerListEgressProvidersResponse {
                    status: Status::Ok,
                    providers: WireVector::from_slice(&providers),
                },
            );
        }
        5 => {
            let response =
                match NetworkRoutingManagerGetScopedSocketProviderRequest::decode(request, handles)
                {
                    Ok(request)
                        if valid_network_domain(request.domain)
                            && runtime.domain_tables.contains_key(request.domain) =>
                    {
                        runtime
                            .client_domains
                            .insert(request.provider.raw, request.domain.to_string());
                        runtime
                            .clients
                            .push(BoundServiceEndpoint::new_with_protocol(
                                Channel(request.provider.raw),
                                (1..=6).collect(),
                                "SocketProvider",
                            ));
                        NetworkRoutingManagerGetScopedSocketProviderResponse { status: Status::Ok }
                    }
                    Ok(request) => {
                        let _ = Memory::close(request.provider.raw);
                        NetworkRoutingManagerGetScopedSocketProviderResponse {
                            status: Status::ErrInvalidArgs,
                        }
                    }
                    Err(_) => NetworkRoutingManagerGetScopedSocketProviderResponse {
                        status: Status::ErrInvalidArgs,
                    },
                };
            reply(channel, &response);
        }
        _ => close_handles_raw(handles),
    }
}

fn poll_legacy(
    runtime: &mut Runtime,
    channel: Channel,
    ordinal: u64,
    request: &[u8],
    handles: &[HandleRef],
) {
    let table = runtime
        .domain_tables
        .get("system_default")
        .copied()
        .unwrap_or(0);
    match ordinal {
        1 => {
            let response = NetstackConnectTcpRequest::decode(request, handles)
                .map(|request| {
                    connect(
                        runtime,
                        table,
                        request.remote_addr,
                        request.options,
                        request.socket.raw,
                    )
                })
                .unwrap_or(NetstackConnectTcpResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(channel, &response);
        }
        2 => {
            let response = NetstackListenTcpRequest::decode(request, handles)
                .map(|request| {
                    listen(
                        runtime,
                        table,
                        request.local_addr,
                        request.options,
                        request.listener.raw,
                    )
                })
                .unwrap_or(NetstackListenTcpResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(channel, &response);
        }
        3 => {
            let response = NetstackCreateUdpSocketRequest::decode(request, handles)
                .map(|request| udp(runtime, table, request.options, request.socket.raw))
                .unwrap_or(NetstackCreateUdpSocketResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(channel, &response);
        }
        4 => {
            let resolved = NetstackResolveHostRequest::decode(request, handles)
                .map(|request| resolve(runtime, table, request.hostname))
                .unwrap_or_else(|_| empty_resolve(Status::ErrInvalidArgs));
            reply(
                channel,
                &NetstackResolveHostResponse {
                    status: resolved.status,
                    addresses: WireVector::from_slice(&resolved.addresses),
                },
            );
        }
        5 => {
            let response = if NetstackGetLinkStatusRequest::decode(request, handles).is_ok() {
                link_status(runtime)
            } else {
                NetstackGetLinkStatusResponse {
                    status: Status::ErrInvalidArgs,
                    link: net_fidl::LinkStatus {
                        available: false,
                        metered: false,
                    },
                }
            };
            reply(channel, &response);
        }
        6 => {
            let response = NetstackWatchLinkStatusRequest::decode(request, handles)
                .map(|request| watch_link(runtime, request.watcher.raw))
                .unwrap_or(NetstackWatchLinkStatusResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(channel, &response);
        }
        _ => close_handles_raw(handles),
    }
}

fn poll_scoped(
    runtime: &mut Runtime,
    channel: Channel,
    ordinal: u64,
    request: &[u8],
    handles: &[HandleRef],
) {
    let Some(table) = runtime
        .client_domains
        .get(&channel.0)
        .and_then(|domain| runtime.domain_tables.get(domain))
        .copied()
    else {
        close_handles_raw(handles);
        return;
    };
    match ordinal {
        1 => {
            let response = SocketProviderConnectTcpRequest::decode(request, handles)
                .map(|request| match request.target {
                    Endpoint::Ip(endpoint) => connect(
                        runtime,
                        table,
                        endpoint,
                        request.options,
                        request.socket.raw,
                    ),
                    Endpoint::Domain(endpoint) => connect_domain(
                        runtime,
                        table,
                        endpoint.host,
                        endpoint.port,
                        request.options,
                        request.socket.raw,
                    ),
                })
                .unwrap_or(NetstackConnectTcpResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(
                channel,
                &SocketProviderConnectTcpResponse {
                    status: response.status,
                },
            );
        }
        2 => {
            let response = SocketProviderListenTcpRequest::decode(request, handles)
                .map(|request| {
                    listen(
                        runtime,
                        table,
                        request.local_addr,
                        request.options,
                        request.listener.raw,
                    )
                })
                .unwrap_or(NetstackListenTcpResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(
                channel,
                &SocketProviderListenTcpResponse {
                    status: response.status,
                },
            );
        }
        3 => {
            let response = SocketProviderCreateUdpSocketRequest::decode(request, handles)
                .map(|request| udp(runtime, table, request.options, request.socket.raw))
                .unwrap_or(NetstackCreateUdpSocketResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(
                channel,
                &SocketProviderCreateUdpSocketResponse {
                    status: response.status,
                },
            );
        }
        4 => {
            let resolved = SocketProviderResolveHostRequest::decode(request, handles)
                .map(|request| resolve(runtime, table, request.hostname))
                .unwrap_or_else(|_| empty_resolve(Status::ErrInvalidArgs));
            reply(
                channel,
                &SocketProviderResolveHostResponse {
                    status: resolved.status,
                    addresses: WireVector::from_slice(&resolved.addresses),
                },
            );
        }
        5 => {
            let response = if SocketProviderGetLinkStatusRequest::decode(request, handles).is_ok() {
                link_status(runtime)
            } else {
                NetstackGetLinkStatusResponse {
                    status: Status::ErrInvalidArgs,
                    link: net_fidl::LinkStatus {
                        available: false,
                        metered: false,
                    },
                }
            };
            reply(
                channel,
                &SocketProviderGetLinkStatusResponse {
                    status: response.status,
                    link: response.link,
                },
            );
        }
        6 => {
            let response = SocketProviderWatchLinkStatusRequest::decode(request, handles)
                .map(|request| watch_link(runtime, request.watcher.raw))
                .unwrap_or(NetstackWatchLinkStatusResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(
                channel,
                &SocketProviderWatchLinkStatusResponse {
                    status: response.status,
                },
            );
        }
        _ => close_handles_raw(handles),
    }
}

fn connect(
    runtime: &mut Runtime,
    table: u32,
    remote_addr: SocketAddress,
    options: net_fidl::SocketOptions,
    front: u64,
) -> NetstackConnectTcpResponse {
    let address = match wire_ip(remote_addr.addr) {
        Ok(address) => address,
        Err(status) => return NetstackConnectTcpResponse { status },
    };
    let lease = match runtime.routing.open_flow_for_table(
        Destination::Ip {
            address,
            port: remote_addr.port,
        },
        table,
    ) {
        Ok(lease) => lease,
        Err(error) => {
            return NetstackConnectTcpResponse {
                status: routing_status(error),
            };
        }
    };
    connect_with_lease(runtime, lease, remote_addr, options, front)
}

fn connect_domain(
    runtime: &mut Runtime,
    table: u32,
    host: &str,
    port: u16,
    options: net_fidl::SocketOptions,
    front: u64,
) -> NetstackConnectTcpResponse {
    let lease = match runtime.routing.open_flow_for_table(
        Destination::Domain {
            host: host.to_string(),
            port,
        },
        table,
    ) {
        Ok(lease) => lease,
        Err(error) => {
            return NetstackConnectTcpResponse {
                status: routing_status(error),
            };
        }
    };
    if matches!(lease.target, Target::Proxy { .. }) {
        return connect_proxy(runtime, lease, host, port, options, front);
    }
    let response = resolve_on_lease(runtime, &lease, host);
    if response.status != Status::Ok {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackConnectTcpResponse {
            status: response.status,
        };
    }
    let Some(address) = response.addresses.first().copied() else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackConnectTcpResponse {
            status: Status::ErrNotFound,
        };
    };
    let allowed = wire_ip(address).ok().is_some_and(|address| {
        runtime
            .routing
            .provider_allows(lease.provider_token, address)
    });
    if !allowed {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackConnectTcpResponse {
            status: Status::ErrAccessDenied,
        };
    }
    connect_with_lease(
        runtime,
        lease,
        SocketAddress {
            addr: address,
            port,
        },
        options,
        front,
    )
}

fn connect_proxy(
    runtime: &mut Runtime,
    lease: RouteLease,
    host: &str,
    port: u16,
    options: net_fidl::SocketOptions,
    front: u64,
) -> NetstackConnectTcpResponse {
    let Target::Proxy { channel } = lease.target else {
        unreachable!()
    };
    let normalized = lease.normalized_domain.as_deref().unwrap_or(host);
    let mut client = ProxyStreamHandlerPublicClient::new(Rpc(Channel(channel)));
    let mut req = [0; 512];
    let mut resp = [0; 128];
    let mut req_handles = [HandleRef { raw: 0 }; 1];
    let mut resp_handles = [HandleRef { raw: 0 }; 2];
    let response = client.connect(
        &ProxyStreamHandlerConnectRequest {
            target: Endpoint::Domain(DomainEndpoint {
                host: normalized,
                port,
            }),
            options,
        },
        &mut req,
        &mut req_handles,
        &mut resp,
        &mut resp_handles,
    );
    match response {
        Ok(response) if response.status == Status::Ok && response.socket.raw != 0 => {
            runtime.proxy_controls.push(ProxyControl {
                channel: Channel(front),
                stream: Some(response.socket.raw),
                flow_id: lease.flow_id,
                backend: None,
            });
            NetstackConnectTcpResponse { status: Status::Ok }
        }
        Ok(response) => {
            if response.socket.raw != 0 {
                let _ = Memory::close(response.socket.raw);
            }
            let _ = runtime.routing.close_flow(lease.flow_id);
            let _ = Memory::close(front);
            NetstackConnectTcpResponse {
                status: response.status,
            }
        }
        Err(_) => {
            let _ = runtime.routing.close_flow(lease.flow_id);
            let _ = Memory::close(front);
            NetstackConnectTcpResponse {
                status: Status::ErrNetworkUnreachable,
            }
        }
    }
}

fn connect_with_lease(
    runtime: &mut Runtime,
    lease: RouteLease,
    remote_addr: SocketAddress,
    _options: net_fidl::SocketOptions,
    front: u64,
) -> NetstackConnectTcpResponse {
    let Target::Device {
        stack_instance,
        table,
        ..
    } = &lease.target
    else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackConnectTcpResponse {
            status: Status::ErrInvalidArgs,
        };
    };
    let Some(backend) = runtime
        .stack_backend_channels
        .get(stack_instance)
        .copied()
        .map(Channel)
    else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackConnectTcpResponse {
            status: Status::ErrShouldWait,
        };
    };
    let Ok((application_stream, backend_stream)) = Socket::pair() else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackConnectTcpResponse {
            status: Status::ErrNoMemory,
        };
    };
    // Retain a duplicate of the stack-facing endpoint.  The duplicate keeps
    // the kernel socket object alive while a crashed netstack instance is
    // absent and is transferred with networkd during a planned replacement.
    let Ok(escrowed_stream) = Memory::duplicate(backend_stream.0, 1 | 2 | 16 | 32) else {
        let _ = Memory::close(application_stream.0);
        let _ = Memory::close(backend_stream.0);
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackConnectTcpResponse {
            status: Status::ErrNoMemory,
        };
    };
    let mut client = StackBackendPublicClient::new(Rpc(backend));
    let mut req = [0; 512];
    let mut resp = [0; 128];
    let mut req_handles = [HandleRef { raw: 0 }; 2];
    let mut resp_handles = [HandleRef { raw: 0 }; 2];
    let response = client.connect_tcp(
        &StackBackendConnectTcpRequest {
            connection_id: lease.flow_id,
            table: WireTableId { value: *table },
            remote_addr,
            stream: HandleRef {
                raw: backend_stream.0,
            },
        },
        &mut req,
        &mut req_handles,
        &mut resp,
        &mut resp_handles,
    );
    match response {
        Ok(response) if response.status == Status::Ok => {
            runtime.escrowed_backends.insert(
                lease.flow_id,
                EscrowedBackend {
                    socket: escrowed_stream,
                    stack_instance: stack_instance.clone(),
                    table: *table,
                    remote: remote_addr,
                },
            );
            runtime.proxy_controls.push(ProxyControl {
                channel: Channel(front),
                stream: Some(application_stream.0),
                flow_id: lease.flow_id,
                backend: Some(backend.0),
            });
            NetstackConnectTcpResponse { status: Status::Ok }
        }
        Ok(response) => {
            let _ = Memory::close(escrowed_stream);
            let _ = Memory::close(application_stream.0);
            let _ = runtime.routing.close_flow(lease.flow_id);
            let _ = Memory::close(front);
            NetstackConnectTcpResponse {
                status: response.status,
            }
        }
        Err(_) => {
            let _ = Memory::close(escrowed_stream);
            let _ = Memory::close(application_stream.0);
            let _ = Memory::close(backend_stream.0);
            let _ = runtime.routing.close_flow(lease.flow_id);
            let _ = Memory::close(front);
            NetstackConnectTcpResponse {
                status: Status::ErrShouldWait,
            }
        }
    }
}

fn listen(
    runtime: &mut Runtime,
    table: u32,
    local_addr: SocketAddress,
    _options: net_fidl::SocketOptions,
    front: u64,
) -> NetstackListenTcpResponse {
    let Ok(lease) = runtime.routing.open_default_flow_for_table(table) else {
        let _ = Memory::close(front);
        return NetstackListenTcpResponse {
            status: Status::ErrNetworkUnreachable,
        };
    };
    let Target::Device {
        stack_instance,
        table: selected_table,
        ..
    } = &lease.target
    else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackListenTcpResponse {
            status: Status::ErrInvalidArgs,
        };
    };
    let Some(backend) = runtime
        .stack_backend_channels
        .get(stack_instance)
        .copied()
        .map(Channel)
    else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackListenTcpResponse {
            status: Status::ErrShouldWait,
        };
    };
    let Ok((back_client, back_server)) = Channel::pair() else {
        return NetstackListenTcpResponse {
            status: Status::ErrNoMemory,
        };
    };
    let Ok(escrow_control) = Memory::duplicate(back_server.0, 1 | 2 | 16 | 32) else {
        let _ = Memory::close(back_client.0);
        let _ = Memory::close(back_server.0);
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackListenTcpResponse {
            status: Status::ErrNoMemory,
        };
    };
    let mut client = StackBackendPublicClient::new(Rpc(backend));
    let mut req = [0; 512];
    let mut resp = [0; 128];
    let mut req_handles = [HandleRef { raw: 0 }; 2];
    let mut resp_handles = [HandleRef { raw: 0 }; 2];
    let response = client.listen_tcp(
        &StackBackendListenTcpRequest {
            listener_id: lease.flow_id,
            table: WireTableId {
                value: *selected_table,
            },
            local_addr,
            listener: HandleRef { raw: back_server.0 },
        },
        &mut req,
        &mut req_handles,
        &mut resp,
        &mut resp_handles,
    );
    finish_proxy(
        runtime,
        lease.flow_id,
        front,
        back_client,
        escrow_control,
        backend.0,
        stack_instance,
        *selected_table,
        1,
        response.map(|value| value.status).map_err(|_| ()),
    )
    .map_or_else(
        |status| NetstackListenTcpResponse { status },
        |_| NetstackListenTcpResponse { status: Status::Ok },
    )
}

fn udp(
    runtime: &mut Runtime,
    table: u32,
    _options: net_fidl::SocketOptions,
    front: u64,
) -> NetstackCreateUdpSocketResponse {
    let Ok(lease) = runtime.routing.open_default_flow_for_table(table) else {
        let _ = Memory::close(front);
        return NetstackCreateUdpSocketResponse {
            status: Status::ErrNetworkUnreachable,
        };
    };
    let Target::Device {
        stack_instance,
        table: selected_table,
        ..
    } = &lease.target
    else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackCreateUdpSocketResponse {
            status: Status::ErrInvalidArgs,
        };
    };
    let Some(backend) = runtime
        .stack_backend_channels
        .get(stack_instance)
        .copied()
        .map(Channel)
    else {
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackCreateUdpSocketResponse {
            status: Status::ErrShouldWait,
        };
    };
    let Ok((back_client, back_server)) = Channel::pair() else {
        return NetstackCreateUdpSocketResponse {
            status: Status::ErrNoMemory,
        };
    };
    let Ok(escrow_control) = Memory::duplicate(back_server.0, 1 | 2 | 16 | 32) else {
        let _ = Memory::close(back_client.0);
        let _ = Memory::close(back_server.0);
        let _ = runtime.routing.close_flow(lease.flow_id);
        let _ = Memory::close(front);
        return NetstackCreateUdpSocketResponse {
            status: Status::ErrNoMemory,
        };
    };
    let mut client = StackBackendPublicClient::new(Rpc(backend));
    let mut req = [0; 256];
    let mut resp = [0; 128];
    let mut req_handles = [HandleRef { raw: 0 }; 2];
    let mut resp_handles = [HandleRef { raw: 0 }; 2];
    let response = client.create_udp_socket(
        &StackBackendCreateUdpSocketRequest {
            object_id: lease.flow_id,
            table: WireTableId {
                value: *selected_table,
            },
            socket: HandleRef { raw: back_server.0 },
        },
        &mut req,
        &mut req_handles,
        &mut resp,
        &mut resp_handles,
    );
    finish_proxy(
        runtime,
        lease.flow_id,
        front,
        back_client,
        escrow_control,
        backend.0,
        stack_instance,
        *selected_table,
        2,
        response.map(|value| value.status).map_err(|_| ()),
    )
    .map_or_else(
        |status| NetstackCreateUdpSocketResponse { status },
        |_| NetstackCreateUdpSocketResponse { status: Status::Ok },
    )
}

fn finish_proxy(
    runtime: &mut Runtime,
    flow_id: u64,
    front: u64,
    back: Channel,
    escrow_control: u64,
    backend: u64,
    stack_instance: &str,
    table: u32,
    kind: u8,
    status: Result<Status, ()>,
) -> Result<(), Status> {
    match status {
        Ok(Status::Ok) => {
            runtime.control_proxies.push(ControlProxy {
                front: Channel(front),
                back,
                flow_id,
                escrow_control,
                backend,
                stack_instance: stack_instance.into(),
                table,
                kind,
            });
            Ok(())
        }
        Ok(status) => {
            let _ = Memory::close(back.0);
            let _ = Memory::close(escrow_control);
            let _ = runtime.routing.close_flow(flow_id);
            let _ = Memory::close(front);
            Err(status)
        }
        Err(_) => {
            let _ = Memory::close(back.0);
            let _ = Memory::close(escrow_control);
            let _ = runtime.routing.close_flow(flow_id);
            let _ = Memory::close(front);
            Err(Status::ErrShouldWait)
        }
    }
}

struct OwnedResolve {
    status: Status,
    addresses: Vec<WireIpAddress>,
}

fn resolve(runtime: &mut Runtime, table: u32, hostname: &str) -> OwnedResolve {
    let Ok(lease) = runtime.routing.open_flow_for_table(
        Destination::Domain {
            host: hostname.to_string(),
            port: 0,
        },
        table,
    ) else {
        return empty_resolve(Status::ErrNetworkUnreachable);
    };
    let response = resolve_on_lease(runtime, &lease, hostname);
    let _ = runtime.routing.close_flow(lease.flow_id);
    response
}

fn resolve_on_lease(runtime: &mut Runtime, lease: &RouteLease, hostname: &str) -> OwnedResolve {
    let Target::Device { table, .. } = &lease.target else {
        return empty_resolve(Status::ErrNetworkUnreachable);
    };
    let normalized = lease.normalized_domain.as_deref().unwrap_or(hostname);
    let now = bexos_userspace::live_migration::now_ms();
    let key_a = CacheKey {
        table: *table,
        provider: lease.provider_token,
        hostname: normalized.to_string(),
        record_type: RecordType::A,
    };
    let key_aaaa = CacheKey {
        record_type: RecordType::Aaaa,
        ..key_a.clone()
    };
    let cached_a = runtime
        .dns
        .lookup(&key_a, now, lease.provider_generation)
        .cloned();
    let cached_aaaa = runtime
        .dns
        .lookup(&key_aaaa, now, lease.provider_generation)
        .cloned();
    if cached_a.is_some() && cached_aaaa.is_some() {
        let addresses = [cached_a, cached_aaaa]
            .into_iter()
            .flatten()
            .flat_map(|value| match value {
                CacheValue::Positive(addresses) => addresses,
                CacheValue::Negative(_) => Vec::new(),
            })
            .take(8)
            .map(wire_address)
            .collect::<Vec<_>>();
        return OwnedResolve {
            status: if addresses.is_empty() {
                Status::ErrNotFound
            } else {
                Status::Ok
            },
            addresses,
        };
    }
    for (key, cached) in [(&key_a, cached_a), (&key_aaaa, cached_aaaa)] {
        if cached.is_none() {
            let status = query_record(runtime, lease, key);
            if status != Status::Ok && status != Status::ErrNotFound {
                return empty_resolve(status);
            }
        }
    }
    let addresses = [&key_a, &key_aaaa]
        .into_iter()
        .filter_map(|key| {
            runtime
                .dns
                .lookup(key, now, lease.provider_generation)
                .cloned()
        })
        .flat_map(|value| match value {
            CacheValue::Positive(addresses) => addresses,
            CacheValue::Negative(_) => Vec::new(),
        })
        .take(8)
        .map(wire_address)
        .collect::<Vec<_>>();
    OwnedResolve {
        status: if addresses.is_empty() {
            Status::ErrNotFound
        } else {
            Status::Ok
        },
        addresses,
    }
}

fn query_record(runtime: &mut Runtime, lease: &RouteLease, original: &CacheKey) -> Status {
    let Some(provider) = runtime.routing.provider(lease.provider_token) else {
        return Status::ErrNetworkUnreachable;
    };
    let upstreams = provider.config.dns_upstreams.clone();
    if upstreams.is_empty() {
        return resolve_record_through_backend(runtime, lease, original);
    }
    let mut hostname = original.hostname.clone();
    let mut seen = Vec::new();
    let mut ttl = u32::MAX;
    for _ in 0..8 {
        if seen.contains(&hostname) {
            return Status::ErrInvalidArgs;
        }
        seen.push(hostname.clone());
        let dns_id = take_backend_object(runtime) as u16;
        let deadline = bexos_userspace::live_migration::now_ms().saturating_add(5_000);
        let pending_key = match runtime.dns.begin(
            original.table,
            original.provider,
            lease.provider_generation,
            &hostname,
            original.record_type,
            dns_id,
            deadline,
        ) {
            Ok(pending) => pending.key.clone(),
            Err(DnsError::Capacity) => return Status::ErrResourceExhausted,
            Err(_) => return Status::ErrInvalidArgs,
        };
        let request = runtime
            .dns
            .pending()
            .get(&pending_key)
            .map(|pending| pending.encoded_request.clone())
            .unwrap_or_default();
        let mut response = None;
        for upstream in &upstreams {
            match exchange_dns(runtime, lease, upstream, &request) {
                Ok(bytes) => {
                    response = Some(bytes);
                    break;
                }
                Err(_) => {
                    let _ = runtime.dns.retry(&pending_key, upstreams.len());
                }
            }
        }
        runtime.dns.pending.remove(&pending_key);
        let Some(response) = response else {
            return Status::ErrNetworkUnreachable;
        };
        let parsed =
            match crate::dns::parse_response(&response, dns_id, &hostname, original.record_type) {
                Ok(parsed) => parsed,
                Err(_) => return Status::ErrNetworkUnreachable,
            };
        ttl = ttl.min(parsed.ttl_seconds);
        if parsed.response_code != ResponseCode::NoError {
            runtime.dns.insert_value(
                original.clone(),
                CacheValue::Negative(parsed.response_code),
                ttl,
                bexos_userspace::live_migration::now_ms(),
                lease.provider_generation,
            );
            return Status::ErrNotFound;
        }
        if !parsed.addresses.is_empty() {
            if parsed.addresses.iter().any(|address| {
                !runtime
                    .routing
                    .provider_allows(lease.provider_token, *address)
            }) {
                return Status::ErrAccessDenied;
            }
            runtime.dns.insert_value(
                original.clone(),
                CacheValue::Positive(parsed.addresses),
                ttl,
                bexos_userspace::live_migration::now_ms(),
                lease.provider_generation,
            );
            return Status::Ok;
        }
        let Some(cname) = parsed.cname else {
            runtime.dns.insert_value(
                original.clone(),
                CacheValue::Negative(ResponseCode::NameError),
                ttl,
                bexos_userspace::live_migration::now_ms(),
                lease.provider_generation,
            );
            return Status::ErrNotFound;
        };
        hostname = cname;
    }
    Status::ErrInvalidArgs
}

fn resolve_record_through_backend(
    runtime: &mut Runtime,
    lease: &RouteLease,
    key: &CacheKey,
) -> Status {
    let Target::Device {
        stack_instance,
        table,
        ..
    } = &lease.target
    else {
        return Status::ErrNetworkUnreachable;
    };
    let Some(backend) = runtime
        .stack_backend_channels
        .get(stack_instance)
        .copied()
        .map(Channel)
    else {
        return Status::ErrShouldWait;
    };
    let mut client = StackBackendPublicClient::new(Rpc(backend));
    let mut request_bytes = [0; 512];
    let mut request_handles = [HandleRef { raw: 0 }; 1];
    let mut response_bytes = [0; 512];
    let mut response_handles = [HandleRef { raw: 0 }; 1];
    let response = client.resolve_host(
        &StackBackendResolveHostRequest {
            table: WireTableId { value: *table },
            hostname: &key.hostname,
        },
        &mut request_bytes,
        &mut request_handles,
        &mut response_bytes,
        &mut response_handles,
    );
    let Ok(response) = response else {
        return Status::ErrNetworkUnreachable;
    };
    if response.status != Status::Ok {
        return response.status;
    }
    let addresses = (0..response.addresses.len())
        .filter_map(|index| response.addresses.get(index).ok())
        .filter_map(|address| wire_ip(address).ok())
        .filter(|address| match key.record_type {
            RecordType::A => matches!(address, IpAddress::V4(_)),
            RecordType::Aaaa => matches!(address, IpAddress::V6(_)),
        })
        .collect::<Vec<_>>();
    if addresses.iter().any(|address| {
        !runtime
            .routing
            .provider_allows(lease.provider_token, *address)
    }) {
        return Status::ErrAccessDenied;
    }
    runtime.dns.insert_value(
        key.clone(),
        if addresses.is_empty() {
            CacheValue::Negative(ResponseCode::NameError)
        } else {
            CacheValue::Positive(addresses)
        },
        60,
        bexos_userspace::live_migration::now_ms(),
        lease.provider_generation,
    );
    Status::Ok
}

fn exchange_dns(
    runtime: &mut Runtime,
    lease: &RouteLease,
    upstream: &DnsUpstream,
    query: &[u8],
) -> Result<Vec<u8>, Status> {
    match upstream.transport {
        DnsTransport::Udp53 => exchange_dns_udp(runtime, lease, upstream, query),
        DnsTransport::Dot => exchange_dns_dot(runtime, lease, upstream, query),
        DnsTransport::Doh => exchange_dns_doh(runtime, lease, upstream, query),
    }
}

fn exchange_dns_udp(
    runtime: &mut Runtime,
    lease: &RouteLease,
    upstream: &DnsUpstream,
    query: &[u8],
) -> Result<Vec<u8>, Status> {
    let Target::Device {
        stack_instance,
        table,
        ..
    } = &lease.target
    else {
        return Err(Status::ErrNetworkUnreachable);
    };
    let backend = runtime
        .stack_backend_channels
        .get(stack_instance)
        .copied()
        .ok_or(Status::ErrShouldWait)?;
    let (client_end, server_end) = Channel::pair().map_err(map_kernel_status)?;
    let object_id = take_backend_object(runtime);
    let response = StackBackendPublicClient::new(Rpc(Channel(backend)))
        .create_udp_socket(
            &StackBackendCreateUdpSocketRequest {
                object_id,
                table: WireTableId { value: *table },
                socket: HandleRef { raw: server_end.0 },
            },
            &mut [0; 128],
            &mut [HandleRef { raw: 0 }; 2],
            &mut [0; 64],
            &mut [HandleRef { raw: 0 }; 1],
        )
        .map_err(|_| Status::ErrShouldWait)?;
    if response.status != Status::Ok {
        let _ = Memory::close(client_end.0);
        return Err(response.status);
    }
    let _guard = BackendObjectGuard {
        backend,
        object_id,
        local: Some(client_end.0),
    };
    let mut udp = UdpSocketPublicClient::new(Rpc(client_end));
    let local = SocketAddress {
        addr: match upstream.address {
            IpAddress::V4(_) => wire_address(IpAddress::V4([0; 4])),
            IpAddress::V6(_) => wire_address(IpAddress::V6([0; 16])),
        },
        port: 0,
    };
    let bound = udp
        .bind(
            &UdpSocketBindRequest { local_addr: local },
            &mut [0; 64],
            &mut [HandleRef { raw: 0 }; 1],
            &mut [0; 64],
            &mut [HandleRef { raw: 0 }; 1],
        )
        .map_err(|_| Status::ErrNetworkUnreachable)?;
    if bound.status != Status::Ok {
        return Err(bound.status);
    }
    let destination = SocketAddress {
        addr: wire_address(upstream.address),
        port: upstream.port,
    };
    let sent = udp
        .send_to(
            &UdpSocketSendToRequest {
                data: query,
                destination,
            },
            &mut vec![0; query.len() + 128],
            &mut [HandleRef { raw: 0 }; 1],
            &mut [0; 64],
            &mut [HandleRef { raw: 0 }; 1],
        )
        .map_err(|_| Status::ErrNetworkUnreachable)?;
    if sent.status != Status::Ok || sent.actual != query.len() as u64 {
        return Err(sent.status);
    }
    let deadline = bexos_userspace::live_migration::now_ms().saturating_add(5_000);
    loop {
        let mut response_bytes = vec![0; crate::dns::MAX_DNS_MESSAGE + 256];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response = udp.recv_from(
            &UdpSocketRecvFromRequest {},
            &mut [0; 32],
            &mut [HandleRef { raw: 0 }; 1],
            &mut response_bytes,
            &mut response_handles,
        );
        if let Ok(response) = response {
            if response.status == Status::Ok {
                if response.source != destination {
                    return Err(Status::ErrAccessDenied);
                }
                return Ok(response.data.to_vec());
            }
            if response.status != Status::ErrShouldWait {
                return Err(response.status);
            }
        }
        if bexos_userspace::live_migration::now_ms() >= deadline {
            return Err(Status::ErrNetworkUnreachable);
        }
        bexos_userspace::yield_now();
    }
}

fn exchange_dns_dot(
    runtime: &mut Runtime,
    lease: &RouteLease,
    upstream: &DnsUpstream,
    query: &[u8],
) -> Result<Vec<u8>, Status> {
    let (socket, object_id, backend) = connect_dns_stream(runtime, lease, upstream)?;
    let result = (|| {
        let trust = runtime.tls_trust.ok_or(Status::ErrAccessDenied)?;
        let mut roots = bexos_net::secure::RootConfigCache::new(Channel(trust));
        let config = roots.config(&[]).map_err(|_| Status::ErrAccessDenied)?;
        let io = bexos_net::secure::BexosSocketIo::new(socket);
        let mut tls = bexos_net::secure::connect_tls(io, &upstream.tls_server_name, config)
            .map_err(|_| Status::ErrAccessDenied)?;
        write_all_dns(
            &mut tls,
            &crate::dns::dot_frame(query).map_err(|_| Status::ErrInvalidArgs)?,
        )?;
        let mut length = [0; 2];
        read_exact_dns(&mut tls, &mut length)?;
        let length = usize::from(u16::from_be_bytes(length));
        if length == 0 || length > crate::dns::MAX_DNS_MESSAGE {
            return Err(Status::ErrBufferTooSmall);
        }
        let mut response = vec![0; length];
        read_exact_dns(&mut tls, &mut response)?;
        Ok(response)
    })();
    close_backend_object(backend, object_id);
    result
}

fn exchange_dns_doh(
    runtime: &mut Runtime,
    lease: &RouteLease,
    upstream: &DnsUpstream,
    query: &[u8],
) -> Result<Vec<u8>, Status> {
    let (socket, object_id, backend) = connect_dns_stream(runtime, lease, upstream)?;
    let result = (|| {
        let trust = runtime.tls_trust.ok_or(Status::ErrAccessDenied)?;
        let mut roots = bexos_net::secure::RootConfigCache::new(Channel(trust));
        let request = crate::dns::doh_request(&upstream.tls_server_name, &upstream.doh_path, query)
            .map_err(|_| Status::ErrInvalidArgs)?;
        let response = bexos_net::secure::tls_exchange(
            bexos_net::secure::BexosSocketIo::new(socket),
            &mut roots,
            &upstream.tls_server_name,
            &[],
            &request,
            crate::dns::MAX_DNS_MESSAGE + 8192,
        )
        .map_err(|_| Status::ErrAccessDenied)?;
        let response = bexos_net::http1::parse_response(&response, crate::dns::MAX_DNS_MESSAGE)
            .map_err(|_| Status::ErrNetworkUnreachable)?;
        if response.status != 200
            || !response.headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("content-type")
                    && value.split(';').next().is_some_and(|value| {
                        value.trim().eq_ignore_ascii_case("application/dns-message")
                    })
            })
        {
            return Err(Status::ErrNetworkUnreachable);
        }
        Ok(response.body)
    })();
    close_backend_object(backend, object_id);
    result
}

fn connect_dns_stream(
    runtime: &mut Runtime,
    lease: &RouteLease,
    upstream: &DnsUpstream,
) -> Result<(Socket, u64, u64), Status> {
    let Target::Device {
        stack_instance,
        table,
        ..
    } = &lease.target
    else {
        return Err(Status::ErrNetworkUnreachable);
    };
    let backend = runtime
        .stack_backend_channels
        .get(stack_instance)
        .copied()
        .ok_or(Status::ErrShouldWait)?;
    let object_id = take_backend_object(runtime);
    let (application, stack) = Socket::pair().map_err(map_kernel_status)?;
    let response = StackBackendPublicClient::new(Rpc(Channel(backend)))
        .connect_tcp(
            &StackBackendConnectTcpRequest {
                connection_id: object_id,
                table: WireTableId { value: *table },
                remote_addr: SocketAddress {
                    addr: wire_address(upstream.address),
                    port: upstream.port,
                },
                stream: HandleRef { raw: stack.0 },
            },
            &mut [0; 256],
            &mut [HandleRef { raw: 0 }; 2],
            &mut [0; 64],
            &mut [HandleRef { raw: 0 }; 1],
        )
        .map_err(|_| Status::ErrShouldWait)?;
    if response.status != Status::Ok {
        let _ = Memory::close(application.0);
        return Err(response.status);
    }
    Ok((application, object_id, backend))
}

fn close_backend_object(backend: u64, object_id: u64) {
    let _ = StackBackendPublicClient::new(Rpc(Channel(backend))).close(
        &net_fidl::StackBackendCloseRequest { object_id },
        &mut [0; 64],
        &mut [HandleRef { raw: 0 }; 1],
        &mut [0; 64],
        &mut [HandleRef { raw: 0 }; 1],
    );
}

fn take_backend_object(runtime: &mut Runtime) -> u64 {
    let id = runtime.next_backend_object;
    runtime.next_backend_object = runtime.next_backend_object.wrapping_add(1).max(1 << 63);
    id
}

fn write_all_dns(stream: &mut impl Write, mut bytes: &[u8]) -> Result<(), Status> {
    let deadline = bexos_userspace::live_migration::now_ms().saturating_add(5_000);
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => {}
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(Status::ErrNetworkUnreachable),
        }
        if bexos_userspace::live_migration::now_ms() >= deadline {
            return Err(Status::ErrTimedOut);
        }
        bexos_userspace::yield_now();
    }
    Ok(())
}

fn read_exact_dns(stream: &mut impl Read, mut bytes: &mut [u8]) -> Result<(), Status> {
    let deadline = bexos_userspace::live_migration::now_ms().saturating_add(5_000);
    while !bytes.is_empty() {
        match stream.read(bytes) {
            Ok(0) => return Err(Status::ErrPeerClosed),
            Ok(read) => bytes = &mut bytes[read..],
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(Status::ErrNetworkUnreachable),
        }
        if bexos_userspace::live_migration::now_ms() >= deadline {
            return Err(Status::ErrTimedOut);
        }
        bexos_userspace::yield_now();
    }
    Ok(())
}

fn map_kernel_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        kernel_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        _ => Status::ErrInvalidHandle,
    }
}

fn empty_resolve(status: Status) -> OwnedResolve {
    OwnedResolve {
        status,
        addresses: Vec::new(),
    }
}

fn link_status(runtime: &Runtime) -> NetstackGetLinkStatusResponse {
    let Some(backend) = runtime
        .backend_channels
        .values()
        .next()
        .copied()
        .map(Channel)
    else {
        return NetstackGetLinkStatusResponse {
            status: Status::ErrShouldWait,
            link: net_fidl::LinkStatus {
                available: false,
                metered: false,
            },
        };
    };
    let mut client = NetstackPublicClient::new(Rpc(backend));
    let mut req = [0; 64];
    let mut resp = [0; 64];
    let mut req_handles = [HandleRef { raw: 0 }; 1];
    let mut resp_handles = [HandleRef { raw: 0 }; 1];
    client
        .get_link_status(
            &NetstackGetLinkStatusRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .unwrap_or(NetstackGetLinkStatusResponse {
            status: Status::ErrShouldWait,
            link: net_fidl::LinkStatus {
                available: false,
                metered: false,
            },
        })
}

fn watch_link(runtime: &Runtime, watcher: u64) -> NetstackWatchLinkStatusResponse {
    let Some(backend) = runtime
        .backend_channels
        .values()
        .next()
        .copied()
        .map(Channel)
    else {
        let _ = Memory::close(watcher);
        return NetstackWatchLinkStatusResponse {
            status: Status::ErrShouldWait,
        };
    };
    let mut client = NetstackPublicClient::new(Rpc(backend));
    let mut req = [0; 64];
    let mut resp = [0; 64];
    let mut req_handles = [HandleRef { raw: 0 }; 1];
    let mut resp_handles = [HandleRef { raw: 0 }; 1];
    client
        .watch_link_status(
            &NetstackWatchLinkStatusRequest {
                watcher: HandleRef { raw: watcher },
            },
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .unwrap_or(NetstackWatchLinkStatusResponse {
            status: Status::ErrShouldWait,
        })
}

fn poll_control_proxies(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut proxies = core::mem::take(&mut runtime.control_proxies);
    proxies.retain(|proxy| {
        let mut alive = true;
        match proxy.front.try_recv() {
            Ok(message) => {
                changed = true;
                if proxy.back.send(&message.bytes, &message.handles).is_err() {
                    alive = false;
                }
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => alive = false,
            Err(_) => {}
        }
        match proxy.back.try_recv() {
            Ok(message) => {
                changed = true;
                if proxy.front.send(&message.bytes, &message.handles).is_err() {
                    alive = false;
                }
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => alive = false,
            Err(_) => {}
        }
        if !alive {
            let _ = runtime.routing.close_flow(proxy.flow_id);
            let _ = Memory::close(proxy.front.0);
            let _ = Memory::close(proxy.back.0);
            if proxy.escrow_control != 0 {
                let _ = Memory::close(proxy.escrow_control);
            }
            if proxy.backend != 0 {
                close_backend_object(proxy.backend, proxy.flow_id);
            }
        }
        alive
    });
    runtime.control_proxies = proxies;
    changed
}

fn checkpoint_backends(runtime: &mut Runtime) -> bool {
    let now = bexos_userspace::live_migration::now_ms();
    if now.saturating_sub(runtime.last_recovery_checkpoint_ms) < 1_000 {
        return false;
    }
    runtime.last_recovery_checkpoint_ms = now;
    let instances = runtime
        .stack_backend_channels
        .iter()
        .map(|(instance, channel)| (instance.clone(), *channel))
        .collect::<Vec<_>>();
    let mut committed_generation = None;
    let changed = true;
    for (instance, backend) in instances {
        let minimum_generation = runtime
            .recovery_journals
            .get(&instance)
            .map_or(0, |journal| journal.generation);
        let response = StackBackendPublicClient::new(Rpc(Channel(backend))).checkpoint_recovery(
            &StackBackendCheckpointRecoveryRequest { minimum_generation },
            &mut [0; 128],
            &mut [HandleRef { raw: 0 }; 1],
            &mut [0; 128],
            &mut [HandleRef { raw: 0 }; 2],
        );
        match response {
            Ok(response)
                if response.status == Status::Ok
                    && response.journal.raw != 0
                    && response.journal_len != 0
                    && response.generation > minimum_generation =>
            {
                if let Some(previous) = runtime.recovery_journals.insert(
                    instance.clone(),
                    RecoveryJournal {
                        handle: response.journal.raw,
                        len: response.journal_len,
                        generation: response.generation,
                    },
                ) {
                    let _ = Memory::close(previous.handle);
                }
                if instance == runtime.instance_id {
                    committed_generation = Some(response.generation);
                }
            }
            Ok(response) if response.status == Status::ErrShouldWait => {
                if response.journal.raw != 0 {
                    let _ = Memory::close(response.journal.raw);
                }
                let Some(journal) = runtime.recovery_journals.get(&instance).copied() else {
                    continue;
                };
                let Ok(copy) = Memory::duplicate(journal.handle, 1 | 4 | 16 | 32) else {
                    continue;
                };
                let adopted = StackBackendPublicClient::new(Rpc(Channel(backend))).adopt_recovery(
                    &StackBackendAdoptRecoveryRequest {
                        journal: HandleRef { raw: copy },
                        journal_len: journal.len,
                        expected_generation: journal.generation,
                    },
                    &mut [0; 128],
                    &mut [HandleRef { raw: 0 }; 2],
                    &mut [0; 128],
                    &mut [HandleRef { raw: 0 }; 1],
                );
                match adopted {
                    Ok(response)
                        if response.status == Status::Ok
                            && response.adopted_generation == journal.generation =>
                    {
                        if instance == runtime.instance_id {
                            recover_virtual_ports(runtime, journal.generation);
                        }
                        recover_escrowed_connections(runtime, &instance, backend);
                    }
                    Ok(_) => {}
                    Err(_) => {
                        let _ = Memory::close(copy);
                    }
                }
            }
            Ok(response) => {
                if response.journal.raw != 0 {
                    let _ = Memory::close(response.journal.raw);
                }
            }
            Err(_) => {}
        }
    }
    if let (Some(generation), Some(controller)) = (committed_generation, runtime.vswitch_controller)
    {
        for port_id in runtime.virtual_ports.iter().copied() {
            let _ = VirtualSwitchControllerPublicClient::new(Rpc(Channel(controller)))
                .commit_generation(
                    &VirtualSwitchControllerCommitGenerationRequest {
                        port_id,
                        generation,
                    },
                    &mut [0; 64],
                    &mut [HandleRef { raw: 0 }; 1],
                    &mut [0; 64],
                    &mut [HandleRef { raw: 0 }; 1],
                );
        }
    }
    changed
}

fn recover_virtual_ports(runtime: &Runtime, generation: u64) {
    let Some(controller) = runtime.vswitch_controller else {
        return;
    };
    for port_id in runtime.virtual_ports.iter().copied() {
        let _ = VirtualSwitchControllerPublicClient::new(Rpc(Channel(controller)))
            .recover_generation(
                &VirtualSwitchControllerRecoverGenerationRequest {
                    port_id,
                    generation,
                },
                &mut [0; 64],
                &mut [HandleRef { raw: 0 }; 1],
                &mut [0; 64],
                &mut [HandleRef { raw: 0 }; 1],
            );
    }
}

fn recover_escrowed_connections(runtime: &Runtime, instance: &str, backend: u64) {
    for (connection_id, escrow) in runtime
        .escrowed_backends
        .iter()
        .filter(|(_, escrow)| escrow.stack_instance == instance)
    {
        let Ok(stream) = Memory::duplicate(escrow.socket, 1 | 2 | 16 | 32) else {
            continue;
        };
        let response = StackBackendPublicClient::new(Rpc(Channel(backend))).recover_connection(
            &StackBackendRecoverConnectionRequest {
                connection_id: *connection_id,
                table: WireTableId {
                    value: escrow.table,
                },
                stream: HandleRef { raw: stream },
            },
            &mut [0; 128],
            &mut [HandleRef { raw: 0 }; 2],
            &mut [0; 64],
            &mut [HandleRef { raw: 0 }; 1],
        );
        if response.is_err() {
            let _ = Memory::close(stream);
        }
    }
    for proxy in runtime
        .control_proxies
        .iter()
        .filter(|proxy| proxy.stack_instance == instance && proxy.escrow_control != 0)
    {
        let Ok(control) = Memory::duplicate(proxy.escrow_control, 1 | 2 | 16 | 32) else {
            continue;
        };
        let kind = match proxy.kind {
            1 => BackendControlKind::TcpListener,
            2 => BackendControlKind::UdpSocket,
            _ => {
                let _ = Memory::close(control);
                continue;
            }
        };
        let response = StackBackendPublicClient::new(Rpc(Channel(backend))).recover_control(
            &StackBackendRecoverControlRequest {
                object_id: proxy.flow_id,
                table: WireTableId { value: proxy.table },
                kind,
                control: HandleRef { raw: control },
            },
            &mut [0; 128],
            &mut [HandleRef { raw: 0 }; 2],
            &mut [0; 64],
            &mut [HandleRef { raw: 0 }; 1],
        );
        if response.is_err() {
            let _ = Memory::close(control);
        }
    }
}

fn poll_proxy_controls(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut controls = core::mem::take(&mut runtime.proxy_controls);
    controls.retain_mut(|control| {
        let message = match control.channel.try_recv() {
            Ok(message) => message,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                finish_proxy_control(runtime, control);
                changed = true;
                return false;
            }
            Err(_) => return true,
        };
        changed = true;
        let (ordinal, request) = envelope(&message.bytes);
        let handles = handle_refs(&message.handles);
        match ordinal {
            1 if TcpSocketGetStreamRequest::decode(request, &handles).is_ok() => {
                let (status, socket) = control
                    .stream
                    .take()
                    .map_or((Status::ErrAlreadyExists, 0), |stream| (Status::Ok, stream));
                reply(
                    control.channel,
                    &TcpSocketGetStreamResponse {
                        status,
                        socket: HandleRef { raw: socket },
                    },
                );
            }
            2 if TcpSocketGetPeerAddressRequest::decode(request, &handles).is_ok() => reply(
                control.channel,
                &TcpSocketGetPeerAddressResponse {
                    status: Status::ErrNotFound,
                    addr: unspecified_address(),
                },
            ),
            3 if TcpSocketGetLocalAddressRequest::decode(request, &handles).is_ok() => reply(
                control.channel,
                &TcpSocketGetLocalAddressResponse {
                    status: Status::ErrNotFound,
                    addr: unspecified_address(),
                },
            ),
            4 if TcpSocketShutdownRequest::decode(request, &handles).is_ok() => reply(
                control.channel,
                &TcpSocketShutdownResponse { status: Status::Ok },
            ),
            5 if TcpSocketCloseRequest::decode(request, &handles).is_ok() => {
                reply(control.channel, &TcpSocketCloseResponse {});
                finish_proxy_control(runtime, control);
                return false;
            }
            _ => close_handles(&message.handles),
        }
        true
    });
    runtime.proxy_controls = controls;
    changed
}

fn finish_proxy_control(runtime: &mut Runtime, control: &mut ProxyControl) {
    if let Some(stream) = control.stream.take() {
        let _ = Memory::close(stream);
    }
    let _ = Memory::close(control.channel.0);
    if let Some(escrow) = runtime.escrowed_backends.remove(&control.flow_id) {
        let _ = Memory::close(escrow.socket);
    }
    if let Some(backend) = control.backend {
        let mut client = StackBackendPublicClient::new(Rpc(Channel(backend)));
        let mut req = [0; 64];
        let mut resp = [0; 64];
        let mut req_handles = [HandleRef { raw: 0 }; 1];
        let mut resp_handles = [HandleRef { raw: 0 }; 1];
        let _ = client.close(
            &net_fidl::StackBackendCloseRequest {
                object_id: control.flow_id,
            },
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        );
    }
    let _ = runtime.routing.close_flow(control.flow_id);
}

fn unspecified_address() -> SocketAddress {
    SocketAddress {
        addr: WireIpAddress::Ipv4(net_fidl::Ipv4Address { octets: [0; 4] }),
        port: 0,
    }
}

fn wire_ip(address: WireIpAddress) -> Result<IpAddress, Status> {
    Ok(match address {
        WireIpAddress::Ipv4(value) => IpAddress::V4(value.octets),
        WireIpAddress::Ipv6(value) => IpAddress::V6(value.octets),
    })
}

fn wire_address(address: IpAddress) -> WireIpAddress {
    match address {
        IpAddress::V4(octets) => WireIpAddress::Ipv4(net_fidl::Ipv4Address { octets }),
        IpAddress::V6(octets) => WireIpAddress::Ipv6(net_fidl::Ipv6Address { octets }),
    }
}

fn provider_config(config: ProviderRouteConfig<'_>) -> Result<ProviderConfig, Status> {
    let target = match config.target.kind {
        RouteTargetKind::Device => {
            if config.target.proxy_channel.raw != 0 {
                let _ = Memory::close(config.target.proxy_channel.raw);
            }
            Target::Device {
                stack_instance: config.target.stack_instance.to_string(),
                table: config.target.table.value,
                interface_id: config.target.interface_id,
            }
        }
        RouteTargetKind::Proxy => Target::Proxy {
            channel: config.target.proxy_channel.raw,
        },
    };
    let mut ip_routes = Vec::new();
    for index in 0..config.ip_routes.len() {
        let route = config
            .ip_routes
            .get(index)
            .map_err(|_| Status::ErrInvalidArgs)?;
        ip_routes.push(
            IpPrefix::new(wire_ip(route.network)?, route.prefix_len).map_err(routing_status)?,
        );
    }
    let mut domain_routes = Vec::new();
    for index in 0..config.domain_routes.len() {
        domain_routes.push(
            config
                .domain_routes
                .get(index)
                .map_err(|_| Status::ErrInvalidArgs)?
                .to_string(),
        );
    }
    let mut dns_upstreams = Vec::new();
    for index in 0..config.dns_servers.len() {
        let server = config
            .dns_servers
            .get(index)
            .map_err(|_| Status::ErrInvalidArgs)?;
        dns_upstreams.push(DnsUpstream {
            address: wire_ip(server.endpoint.addr)?,
            port: server.endpoint.port,
            transport: match server.transport {
                WireDnsTransport::Udp53 => DnsTransport::Udp53,
                WireDnsTransport::Dot => DnsTransport::Dot,
                WireDnsTransport::Doh => DnsTransport::Doh,
            },
            tls_server_name: server.tls_server_name.to_string(),
            doh_path: server.doh_path.to_string(),
        });
    }
    Ok(ProviderConfig {
        target,
        ip_routes,
        domain_routes,
        dns_upstreams,
        priority: config.priority,
        default_route: config.default_route,
    })
}

fn valid_network_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 64
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn routing_status(error: RoutingError) -> Status {
    match error {
        RoutingError::InvalidName
        | RoutingError::InvalidDomain
        | RoutingError::InvalidPrefix
        | RoutingError::InvalidTarget
        | RoutingError::InvalidDns => Status::ErrInvalidArgs,
        RoutingError::Capacity => Status::ErrResourceExhausted,
        RoutingError::AlreadyExists => Status::ErrAlreadyExists,
        RoutingError::NotFound => Status::ErrNotFound,
        RoutingError::NetworkUnreachable => Status::ErrNetworkUnreachable,
        RoutingError::ShouldWait => Status::ErrShouldWait,
    }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut bytes = vec![0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 8];
    if let Ok(encoded) = response.encode(&mut bytes, &mut handles) {
        let handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = channel.send(&bytes[..encoded.bytes], &handles);
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn handle_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

fn close_handles(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}

fn close_handles_raw(handles: &[HandleRef]) {
    for handle in handles {
        let _ = Memory::close(handle.raw);
    }
}
