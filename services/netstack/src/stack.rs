use alloc::vec::Vec;
use net_fidl::{IpAddress, Ipv4Address, Ipv6Address, SocketAddress, Status};

use crate::config::{ActiveConfig, ConfigSource, DhcpLease, DnsMode, NetConfig};
use crate::dns::{DnsCache, DnsRecord, DnsTransport};
use crate::link::PacketLink;
use crate::smoltcp_runtime::SmoltcpRuntime;
use crate::tcp::{TcpEndpoint, TcpListenerState, unspecified};
use crate::udp::UdpEndpoint;

pub struct Netstack {
    pub config: ActiveConfig,
    pub tcp: Vec<TcpEndpoint>,
    pub listeners: Vec<TcpListenerState>,
    pub udp: Vec<UdpEndpoint>,
    pub dns: DnsCache,
    pub next_ephemeral_port: u16,
    smoltcp_epoch: smoltcp::time::Instant,
    pub smoltcp: Option<SmoltcpRuntime>,
}

impl Netstack {
    pub fn new(config: NetConfig, dhcp: Option<DhcpLease>) -> Self {
        Self::from_active(config.choose_active(dhcp))
    }

    pub fn from_active(config: ActiveConfig) -> Self {
        Self {
            config,
            tcp: Vec::new(),
            listeners: Vec::new(),
            udp: Vec::new(),
            dns: DnsCache::default(),
            next_ephemeral_port: 49152,
            smoltcp_epoch: smoltcp::time::Instant::from_millis(0),
            smoltcp: None,
        }
    }

    pub fn apply_dhcp(&mut self, config: NetConfig, lease: DhcpLease) {
        self.config = config.choose_active(Some(lease));
        if let Some(smoltcp) = &mut self.smoltcp {
            smoltcp.apply_config(self.config.clone());
        }
    }

    pub fn poll_packet_plane(&mut self, link: Option<&mut PacketLink>) {
        let Some(link) = link else {
            return;
        };
        let _ = link.poll();
        let mut frame = [0u8; crate::link::SLOT_SIZE];
        while let Ok(len) = link.receive_device(&mut frame) {
            if !self.ingest_ethernet(&frame[..len]) {
                let _ = link.push_rx_backlog(&frame[..len]);
            }
        }
        self.ensure_smoltcp(link);
        if let Some(smoltcp) = &mut self.smoltcp {
            smoltcp.poll(
                link,
                &mut self.tcp,
                &mut self.listeners,
                &mut self.udp,
                &mut self.dns,
            );
        }
    }

    /// Keep every non-primary link active. Protocol state and default routing
    /// remain on the deterministic primary, while traffic arriving on another
    /// NIC is still consumed and queued for failover.
    pub fn poll_secondary_packet_plane(&mut self, link: &mut PacketLink) {
        let _ = link.poll();
        let mut frame = [0u8; crate::link::SLOT_SIZE];
        while let Ok(len) = link.receive_device(&mut frame) {
            if !self.ingest_ethernet(&frame[..len]) {
                let _ = link.push_rx_backlog(&frame[..len]);
            }
        }
    }

    pub fn source(&self) -> ConfigSource {
        self.config.source
    }

    pub fn smoltcp_epoch_millis(&self) -> i64 {
        self.smoltcp_epoch.total_millis()
    }

    pub fn connect_tcp(&mut self, control: u64, remote: SocketAddress) -> Status {
        if local_for_remote(remote.addr, &self.config).is_none() {
            return Status::ErrNetworkUnreachable;
        }
        if self.tcp.len() >= 64 {
            return Status::ErrResourceExhausted;
        }
        let local = SocketAddress {
            addr: local_for_remote(remote.addr, &self.config).unwrap_or(IpAddress::Ipv4(
                Ipv4Address {
                    octets: [0, 0, 0, 0],
                },
            )),
            port: self.allocate_port(),
        };
        self.tcp.push(TcpEndpoint::new(control, remote, local));
        Status::Ok
    }

    pub fn attach_tcp_to_link(&mut self, control: u64, link: Option<&mut PacketLink>) -> Status {
        let Some(link) = link else {
            return Status::ErrNetworkUnreachable;
        };
        self.ensure_smoltcp(link);
        let Some(smoltcp) = self.smoltcp.as_mut() else {
            return Status::ErrNetworkUnreachable;
        };
        let Some(endpoint) = self.tcp.iter_mut().find(|socket| socket.control == control) else {
            return Status::ErrNotFound;
        };
        if endpoint.smoltcp_handle.is_some() || smoltcp.connect_tcp(endpoint) {
            Status::Ok
        } else {
            Status::ErrInvalidArgs
        }
    }

    pub fn listen_tcp(&mut self, control: u64, local: SocketAddress) -> Status {
        if self.listeners.len() >= 32 {
            return Status::ErrResourceExhausted;
        }
        self.listeners.push(TcpListenerState::new(control, local));
        Status::Ok
    }

    pub fn attach_listener_to_link(
        &mut self,
        control: u64,
        link: Option<&mut PacketLink>,
    ) -> Status {
        let Some(link) = link else {
            return Status::ErrNetworkUnreachable;
        };
        self.ensure_smoltcp(link);
        let Some(smoltcp) = self.smoltcp.as_mut() else {
            return Status::ErrNetworkUnreachable;
        };
        let Some(listener) = self
            .listeners
            .iter_mut()
            .find(|listener| listener.control == control)
        else {
            return Status::ErrNotFound;
        };
        if listener.smoltcp_handle.is_some() || smoltcp.listen_tcp(listener) {
            Status::Ok
        } else {
            Status::ErrInvalidArgs
        }
    }

    pub fn create_udp(&mut self, control: u64) -> Status {
        if self.udp.len() >= 64 {
            return Status::ErrResourceExhausted;
        }
        self.udp.push(UdpEndpoint::new(control));
        Status::Ok
    }

    pub fn tcp_mut(&mut self, control: u64) -> Option<&mut TcpEndpoint> {
        self.tcp.iter_mut().find(|socket| socket.control == control)
    }

    pub fn listener_mut(&mut self, control: u64) -> Option<&mut TcpListenerState> {
        self.listeners
            .iter_mut()
            .find(|listener| listener.control == control)
    }

    pub fn udp_mut(&mut self, control: u64) -> Option<&mut UdpEndpoint> {
        self.udp.iter_mut().find(|socket| socket.control == control)
    }

    pub fn remove_tcp(&mut self, control: u64) {
        if let Some(index) = self.tcp.iter().position(|socket| socket.control == control) {
            let mut endpoint = self.tcp.remove(index);
            if let (Some(runtime), Some(handle)) =
                (&mut self.smoltcp, endpoint.smoltcp_handle.take())
            {
                runtime.retire_tcp(handle);
            }
            endpoint.close();
            let _ = bexos_userspace::Memory::close(endpoint.control);
            if let Some(client) = endpoint.client_control.take() {
                let _ = bexos_userspace::Memory::close(client);
            }
        }
    }

    pub fn remove_listener(&mut self, control: u64) {
        if let Some(listener) = self.listener_mut(control) {
            listener.close();
        }
        self.listeners
            .retain(|listener| listener.control != control);
    }

    pub fn remove_udp(&mut self, control: u64) {
        if let Some(socket) = self.udp_mut(control) {
            socket.close();
        }
        self.udp.retain(|socket| socket.control != control);
    }

    pub fn restore_after_migration(&mut self, link: Option<&mut PacketLink>) -> Status {
        let Some(link) = link else {
            return if self.tcp.is_empty() && self.listeners.is_empty() {
                Status::Ok
            } else {
                Status::ErrNetworkUnreachable
            };
        };
        self.ensure_smoltcp(link);
        let Some(smoltcp) = self.smoltcp.as_mut() else {
            return Status::ErrNetworkUnreachable;
        };
        for endpoint in &mut self.tcp {
            if endpoint.state == crate::tcp::TcpState::Established {
                endpoint.smoltcp_handle = None;
                if !smoltcp.restore_tcp(endpoint) {
                    return Status::ErrNetworkUnreachable;
                }
            } else if endpoint.state == crate::tcp::TcpState::Connecting {
                endpoint.smoltcp_handle = None;
                if endpoint.smoltcp_migration.is_some() {
                    if !smoltcp.restore_tcp(endpoint) {
                        return Status::ErrNetworkUnreachable;
                    }
                } else if !smoltcp.connect_tcp(endpoint) {
                    return Status::ErrNetworkUnreachable;
                }
            }
        }
        for listener in &mut self.listeners {
            listener.smoltcp_handle = None;
            if !listener.closed && !smoltcp.listen_tcp(listener) {
                return Status::ErrNetworkUnreachable;
            }
        }
        Status::Ok
    }

    pub fn resolve_cached(&self, hostname: &str) -> Option<&[DnsRecord]> {
        self.dns.lookup(hostname, now_ms())
    }

    pub fn cache_dns(&mut self, hostname: &str, addresses: &[[u8; 4]]) -> Status {
        let records = addresses
            .iter()
            .copied()
            .map(DnsRecord::A)
            .collect::<Vec<_>>();
        self.dns
            .insert(hostname, &records, now_ms().saturating_add(60_000))
    }

    pub fn query_dns(&mut self, hostname: &str, link: Option<&mut PacketLink>) -> Status {
        if self.config.dns_mode == DnsMode::Doh && self.config.doh_strict {
            return Status::ErrNetworkUnreachable;
        }
        let Some(link) = link else {
            return Status::ErrNetworkUnreachable;
        };
        let (dns_addr, source_addr) = match choose_dns_server(&self.config) {
            Some(pair) => pair,
            None if self.config.dns_mode == DnsMode::Doh && !self.config.doh_strict => {
                match (self.config.dns, self.config.ipv4) {
                    (Some(dns), Some(ipv4)) => (
                        IpAddress::Ipv4(Ipv4Address { octets: dns }),
                        IpAddress::Ipv4(Ipv4Address { octets: ipv4 }),
                    ),
                    _ => return Status::ErrNetworkUnreachable,
                }
            }
            None => return Status::ErrNetworkUnreachable,
        };
        let mut query = [0u8; 512];
        let len = match self
            .dns
            .begin_query(hostname, DnsTransport::Udp53, &mut query)
        {
            Ok(len) => len,
            Err(Status::ErrShouldWait) => return Status::ErrShouldWait,
            Err(status) => return status,
        };
        let source = SocketAddress {
            addr: source_addr,
            port: self.allocate_port(),
        };
        let destination = SocketAddress {
            addr: dns_addr,
            port: 53,
        };
        self.ensure_smoltcp(link);
        if let Some(smoltcp) = self.smoltcp.as_mut() {
            if smoltcp.send_udp_packet(source, destination, &query[..len]) {
                return Status::ErrShouldWait;
            }
        }
        Status::ErrNetworkUnreachable
    }

    fn allocate_port(&mut self) -> u16 {
        let port = self.next_ephemeral_port;
        self.next_ephemeral_port = if self.next_ephemeral_port == 65535 {
            49152
        } else {
            self.next_ephemeral_port + 1
        };
        port
    }

    fn ingest_ethernet(&mut self, frame: &[u8]) -> bool {
        if let Some(datagram) = crate::udp::decode_udp_ipv4_ethernet(frame) {
            for udp in &mut self.udp {
                if udp.matches_destination(datagram.destination) {
                    let _ = udp.push_datagram(&datagram.data[..datagram.len], datagram.source);
                }
            }
            if datagram.destination.port == 68 {
                if let Ok(lease) = crate::dhcp::parse_ack(&datagram.data[..datagram.len]) {
                    self.config.source = ConfigSource::Dhcp;
                    self.config.ipv4 = Some(lease.ipv4);
                    self.config.prefix_len = lease.prefix_len;
                    self.config.gateway = lease.gateway;
                    self.config.dns = lease.dns;
                }
            } else if datagram.source.port == 53 {
                let _ = self
                    .dns
                    .complete_response(&datagram.data[..datagram.len], now_ms());
            }
            return true;
        }
        false
    }

    fn ensure_smoltcp(&mut self, link: &mut PacketLink) {
        if self.smoltcp.is_none() {
            self.smoltcp = Some(SmoltcpRuntime::new(self.config.clone(), link));
        }
    }
}

fn now_ms() -> u64 {
    bexos_userspace::syscall::ticks().saturating_mul(1000) / bexos_userspace::syscall::frequency()
}

impl Default for Netstack {
    fn default() -> Self {
        Self::new(NetConfig::default(), None)
    }
}

pub fn empty_addr() -> SocketAddress {
    unspecified()
}

fn local_for_remote(remote: IpAddress, config: &ActiveConfig) -> Option<IpAddress> {
    match remote {
        IpAddress::Ipv4(_) => config
            .ipv4
            .map(|octets| IpAddress::Ipv4(Ipv4Address { octets })),
        IpAddress::Ipv6(_) => config
            .ipv6
            .map(|octets| IpAddress::Ipv6(Ipv6Address { octets })),
    }
}

fn choose_dns_server(config: &ActiveConfig) -> Option<(IpAddress, IpAddress)> {
    if config.dns_mode == DnsMode::Doh {
        if let (Some(server), Some(local)) = (config.doh_bootstrap_ipv6, config.ipv6) {
            return Some((
                IpAddress::Ipv6(Ipv6Address { octets: server }),
                IpAddress::Ipv6(Ipv6Address { octets: local }),
            ));
        }
        if let (Some(server), Some(local)) = (config.doh_bootstrap_ipv4, config.ipv4) {
            return Some((
                IpAddress::Ipv4(Ipv4Address { octets: server }),
                IpAddress::Ipv4(Ipv4Address { octets: local }),
            ));
        }
        return None;
    }
    if let (Some(server), Some(local)) = (config.dns_ipv6, config.ipv6) {
        return Some((
            IpAddress::Ipv6(Ipv6Address { octets: server }),
            IpAddress::Ipv6(Ipv6Address { octets: local }),
        ));
    }
    if let (Some(server), Some(local)) = (config.dns, config.ipv4) {
        return Some((
            IpAddress::Ipv4(Ipv4Address { octets: server }),
            IpAddress::Ipv4(Ipv4Address { octets: local }),
        ));
    }
    None
}
