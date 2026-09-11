use alloc::vec;
use alloc::vec::Vec;

use bexos_userspace::Socket;
use net_fidl::{
    IpAddress as FidlIpAddress, Ipv4Address as FidlIpv4Address, Ipv6Address as FidlIpv6Address,
};
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, Ipv4Address, Ipv6Address,
};

use crate::config::{ActiveConfig, ConfigSource};
use crate::dns::DnsCache;
use crate::link::PacketLink;
use crate::tcp::{TcpEndpoint, TcpListenerState, TcpState};
use crate::udp::{Datagram, UdpEndpoint};

const TCP_RX_BYTES: usize = 8192;
const TCP_TX_BYTES: usize = 8192;
const UDP_PACKETS: usize = 64;
const UDP_BYTES: usize = 8192 * 4;

pub struct SmoltcpRuntime {
    iface: Interface,
    sockets: SocketSet<'static>,
    dns_udp: Option<smoltcp::iface::SocketHandle>,
}

impl SmoltcpRuntime {
    pub fn new(config: ActiveConfig, link: &mut PacketLink) -> Self {
        let mut iface_config = Config::new(EthernetAddress(link.resources.mac).into());
        iface_config.random_seed = 0x0be0_0005;
        let mut iface = Interface::new(iface_config, link, Instant::from_millis(0));
        apply_config(&mut iface, config);
        Self {
            iface,
            sockets: SocketSet::new(Vec::new()),
            dns_udp: None,
        }
    }

    pub fn apply_config(&mut self, config: ActiveConfig) {
        apply_config(&mut self.iface, config);
    }

    pub fn connect_tcp(&mut self, endpoint: &mut TcpEndpoint) -> bool {
        let (Some(remote), Some(local)) = (
            to_ip_endpoint(endpoint.peer),
            to_ip_endpoint(endpoint.local),
        ) else {
            return false;
        };
        let socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; TCP_RX_BYTES]),
            tcp::SocketBuffer::new(vec![0; TCP_TX_BYTES]),
        );
        let handle = self.sockets.add(socket);
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if socket
            .connect(self.iface.context(), remote, listen_endpoint(local))
            .is_err()
        {
            socket.abort();
            return false;
        }
        endpoint.smoltcp_handle = Some(handle);
        endpoint.state = TcpState::Connecting;
        true
    }

    pub fn restore_tcp(&mut self, endpoint: &mut TcpEndpoint) -> bool {
        let Some(snapshot) = endpoint.smoltcp_migration.take() else {
            return false;
        };
        let socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; TCP_RX_BYTES]),
            tcp::SocketBuffer::new(vec![0; TCP_TX_BYTES]),
        );
        let handle = self.sockets.add(socket);
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if socket.restore(snapshot, now()).is_err() {
            socket.abort();
            return false;
        }
        endpoint.smoltcp_handle = Some(handle);
        true
    }

    pub fn checkpoint_tcp(
        &self,
        endpoint: &TcpEndpoint,
    ) -> Option<smoltcp::socket::tcp::MigrationState> {
        let Some(handle) = endpoint.smoltcp_handle else {
            return endpoint.smoltcp_migration.clone();
        };
        let socket = self.sockets.get::<tcp::Socket>(handle);
        Some(socket.checkpoint(now()))
    }

    pub fn listen_tcp(&mut self, listener: &mut TcpListenerState) -> bool {
        if listener.smoltcp_handle.is_some() {
            return true;
        }
        let Some(local) = to_listen_endpoint(listener.local) else {
            return false;
        };
        let socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; TCP_RX_BYTES]),
            tcp::SocketBuffer::new(vec![0; TCP_TX_BYTES]),
        );
        let handle = self.sockets.add(socket);
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if socket.listen(local).is_err() {
            socket.abort();
            return false;
        }
        listener.smoltcp_handle = Some(handle);
        true
    }

    pub fn poll(
        &mut self,
        link: &mut PacketLink,
        tcp: &mut [TcpEndpoint],
        listeners: &mut [TcpListenerState],
        udp: &mut [UdpEndpoint],
        dns: &mut DnsCache,
    ) {
        self.iface.poll(now(), link, &mut self.sockets);
        for endpoint in tcp {
            self.poll_endpoint(endpoint);
        }
        for listener in listeners {
            self.poll_listener(listener);
        }
        for endpoint in udp {
            self.poll_udp(endpoint);
        }
        self.poll_dns(dns);
        self.iface.poll(now(), link, &mut self.sockets);
    }

    pub fn send_udp_packet(
        &mut self,
        source: net_fidl::SocketAddress,
        destination: net_fidl::SocketAddress,
        payload: &[u8],
    ) -> bool {
        let (Some(src), Some(dst)) = (to_listen_endpoint(source), to_ip_endpoint(destination))
        else {
            return false;
        };
        if self.dns_udp.is_none() {
            let socket = udp::Socket::new(udp_buffer(), udp_buffer());
            self.dns_udp = Some(self.sockets.add(socket));
        }
        let Some(handle) = self.dns_udp else {
            return false;
        };
        let socket = self.sockets.get_mut::<udp::Socket>(handle);
        if !socket.is_open() && socket.bind(src).is_err() {
            return false;
        }
        socket.send_slice(payload, dst).is_ok()
    }

    fn poll_endpoint(&mut self, endpoint: &mut TcpEndpoint) {
        let Some(handle) = endpoint.smoltcp_handle else {
            return;
        };
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if socket.may_send() {
            endpoint.state = TcpState::Established;
        } else if !socket.is_open() {
            endpoint.state = TcpState::Closed;
        }
        if let Some(stream) = endpoint.stream {
            pump_stream_to_tcp(stream, socket);
            pump_tcp_to_stream(stream, socket);
        }
    }

    fn poll_listener(&mut self, listener: &mut TcpListenerState) {
        if listener.smoltcp_handle.is_none() && !listener.closed {
            let _ = self.listen_tcp(listener);
        }
        let Some(handle) = listener.smoltcp_handle else {
            return;
        };
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if !socket.is_active() {
            return;
        }
        let (Some(peer), Some(local)) = (socket.remote_endpoint(), socket.local_endpoint()) else {
            return;
        };
        let Ok((service, client)) = bexos_userspace::Channel::pair() else {
            socket.abort();
            return;
        };
        listener.pending.push(TcpEndpoint {
            control: service.0,
            client_control: Some(client.0),
            stream: None,
            peer: from_endpoint(peer),
            local: from_endpoint(local),
            state: TcpState::Established,
            smoltcp_handle: Some(handle),
            smoltcp_migration: None,
        });
        listener.smoltcp_handle = None;
    }

    fn poll_udp(&mut self, endpoint: &mut UdpEndpoint) {
        if endpoint.closed {
            return;
        }
        if endpoint.smoltcp_handle.is_none() {
            let socket = udp::Socket::new(udp_buffer(), udp_buffer());
            let handle = self.sockets.add(socket);
            endpoint.smoltcp_handle = Some(handle);
        }
        let Some(handle) = endpoint.smoltcp_handle else {
            return;
        };
        let socket = self.sockets.get_mut::<udp::Socket>(handle);
        if let Some(local) = endpoint.local {
            if !socket.is_open() {
                let _ = socket.bind(to_listen_endpoint(local).unwrap_or(IpListenEndpoint {
                    addr: None,
                    port: local.port,
                }));
            }
        }
        while let Some(packet) = endpoint.outbound.pop_front() {
            let Some(dst) = to_ip_endpoint(packet.destination) else {
                continue;
            };
            let _ = socket.send_slice(&packet.data[..packet.len], dst);
        }
        loop {
            let mut data = [0u8; 8192];
            match socket.recv_slice(&mut data) {
                Ok((len, meta)) => {
                    if endpoint.queue.len() < 64 {
                        endpoint.queue.push_back(Datagram {
                            data,
                            len,
                            source: from_endpoint(meta.endpoint),
                        });
                    }
                }
                Err(_) => break,
            }
        }
    }

    fn poll_dns(&mut self, dns: &mut DnsCache) {
        let Some(handle) = self.dns_udp else {
            return;
        };
        let socket = self.sockets.get_mut::<udp::Socket>(handle);
        loop {
            let mut data = [0u8; 8192];
            match socket.recv_slice(&mut data) {
                Ok((len, _)) => {
                    let _ = dns.complete_response(&data[..len], now_ms());
                }
                Err(_) => break,
            }
        }
    }
}

fn apply_config(iface: &mut Interface, config: ActiveConfig) {
    iface.update_ip_addrs(|addrs| {
        addrs.clear();
        if let Some(ipv4) = config.ipv4 {
            let _ = addrs.push(IpCidr::new(
                IpAddress::Ipv4(Ipv4Address::new(ipv4[0], ipv4[1], ipv4[2], ipv4[3])),
                config.prefix_len,
            ));
        }
        if let Some(ipv6) = config.ipv6 {
            let _ = addrs.push(IpCidr::new(
                IpAddress::Ipv6(Ipv6Address::from_octets(ipv6)),
                config.ipv6_prefix_len,
            ));
        }
    });
    if let Some(gateway) = config.gateway {
        let _ = iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(
            gateway[0], gateway[1], gateway[2], gateway[3],
        ));
    }
    if config.source == ConfigSource::Unconfigured {
        iface.routes_mut().remove_default_ipv4_route();
    }
    if let Some(gateway) = config.ipv6_gateway {
        let _ = iface
            .routes_mut()
            .add_default_ipv6_route(Ipv6Address::from_octets(gateway));
    } else if config.source == ConfigSource::Unconfigured && !config.slaac_enabled {
        iface.routes_mut().remove_default_ipv6_route();
    }
}

fn pump_stream_to_tcp(stream: Socket, socket: &mut tcp::Socket<'_>) {
    if !socket.can_send() {
        return;
    }
    let Ok(info) = stream.info() else {
        return;
    };
    if info.readable_bytes == 0 {
        return;
    }
    let max = info.readable_bytes.min(4096) as u32;
    if let Ok(bytes) = stream.read(max) {
        let _ = socket.send_slice(&bytes);
    }
}

fn pump_tcp_to_stream(stream: Socket, socket: &mut tcp::Socket<'_>) {
    if !socket.can_recv() {
        return;
    }
    let mut bytes = [0u8; 4096];
    if let Ok(len) = socket.recv_slice(&mut bytes) {
        if len != 0 {
            let _ = stream.write(&bytes[..len]);
        }
    }
}

fn to_ip_endpoint(addr: net_fidl::SocketAddress) -> Option<IpEndpoint> {
    Some(IpEndpoint::new(to_ip_address(addr.addr)?, addr.port))
}

fn to_listen_endpoint(addr: net_fidl::SocketAddress) -> Option<IpListenEndpoint> {
    let endpoint = to_ip_endpoint(addr)?;
    Some(listen_endpoint(endpoint))
}

fn listen_endpoint(endpoint: IpEndpoint) -> IpListenEndpoint {
    IpListenEndpoint {
        addr: Some(endpoint.addr),
        port: endpoint.port,
    }
}

fn from_endpoint(endpoint: IpEndpoint) -> net_fidl::SocketAddress {
    let addr = match endpoint.addr {
        IpAddress::Ipv4(ip) => FidlIpAddress::Ipv4(FidlIpv4Address {
            octets: ip.octets(),
        }),
        IpAddress::Ipv6(ip) => FidlIpAddress::Ipv6(FidlIpv6Address {
            octets: ip.octets(),
        }),
    };
    net_fidl::SocketAddress {
        addr,
        port: endpoint.port,
    }
}

fn to_ip_address(addr: FidlIpAddress) -> Option<IpAddress> {
    match addr {
        FidlIpAddress::Ipv4(ip) => Some(IpAddress::Ipv4(Ipv4Address::new(
            ip.octets[0],
            ip.octets[1],
            ip.octets[2],
            ip.octets[3],
        ))),
        FidlIpAddress::Ipv6(ip) => Some(IpAddress::Ipv6(Ipv6Address::from_octets(ip.octets))),
    }
}

fn udp_buffer() -> udp::PacketBuffer<'static> {
    udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; UDP_PACKETS],
        vec![0; UDP_BYTES],
    )
}

fn now() -> Instant {
    Instant::from_millis(bexos_userspace::live_migration::now_ms() as i64)
}

fn now_ms() -> u64 {
    bexos_userspace::live_migration::now_ms()
}
