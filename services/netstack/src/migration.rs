use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::vec::Vec;

use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;
use net_fidl::{IpAddress, Ipv4Address, SocketAddress};

use crate::config::{ActiveConfig, ConfigSource, DnsMode};
use crate::dns::{DnsEntry, DnsRecord, DnsTransport, PendingDnsQuery};
use crate::link::{LinkResources, PacketLink};
use crate::router::{FibRoute, InterfaceState, Router, Table, TableQuota};
use crate::stack::Netstack;
use crate::tcp::{TcpEndpoint, TcpListenerState, TcpState};
use crate::udp::{Datagram, OutboundDatagram, UdpEndpoint};
use smoltcp::socket::tcp::{
    MigrationRttEstimator, MigrationState as TcpMigrationState, State as SmoltcpTcpState,
};
use smoltcp::wire::{
    IpAddress as SmoltcpIpAddress, IpEndpoint as SmoltcpIpEndpoint,
    IpListenEndpoint as SmoltcpIpListenEndpoint,
};

const HEADER_RECORD_KEY: u64 = 0;
const TCP_RECORD_BASE: u64 = 1;
const MAX_TCP_RECORDS: usize = 64;
pub(crate) const LINK_QUEUES: u64 = TCP_RECORD_BASE + MAX_TCP_RECORDS as u64;
const LINK_STRIDE: u64 = 1 + crate::link::BACKLOG_CHUNKS as u64;
const MAX_LINKS: usize = 16;

pub struct NodeLink {
    pub node_id: u64,
    pub link: PacketLink,
}

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub tls_trust: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub backend_clients: Vec<BoundServiceEndpoint>,
    pub controller_clients: Vec<BoundServiceEndpoint>,
    pub backend_connections: BTreeMap<u64, u32>,
    pub backend_controls: BTreeMap<u64, (u8, u64)>,
    pub link_watchers: Vec<u64>,
    pub stack: Netstack,
    pub router: Router,
    pub links: Vec<NodeLink>,
    pub generation: u64,
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            tls_trust: None,
            clients: Vec::new(),
            backend_clients: Vec::new(),
            controller_clients: Vec::new(),
            backend_connections: BTreeMap::new(),
            backend_controls: BTreeMap::new(),
            link_watchers: Vec::new(),
            stack: Netstack::from_active(ActiveConfig {
                source: ConfigSource::Unconfigured,
                ipv4: None,
                prefix_len: 0,
                gateway: None,
                dns: None,
                ipv6: None,
                ipv6_prefix_len: 0,
                ipv6_gateway: None,
                dns_ipv6: None,
                doh_bootstrap_ipv6: None,
                slaac_enabled: true,
                mtu: 1500,
                dns_mode: DnsMode::Udp53,
                doh_host: "dns.google".to_string(),
                doh_path: "/dns-query".to_string(),
                doh_bootstrap_ipv4: None,
                doh_port: 443,
                doh_strict: false,
                dns_cache_capacity: 64,
            }),
            router: Router::new(ActiveConfig {
                source: ConfigSource::Unconfigured,
                ipv4: None,
                prefix_len: 0,
                gateway: None,
                dns: None,
                ipv6: None,
                ipv6_prefix_len: 0,
                ipv6_gateway: None,
                dns_ipv6: None,
                doh_bootstrap_ipv6: None,
                slaac_enabled: true,
                mtu: 1500,
                dns_mode: DnsMode::Udp53,
                doh_host: "dns.google".to_string(),
                doh_path: "/dns-query".to_string(),
                doh_bootstrap_ipv4: None,
                doh_port: 443,
                doh_strict: false,
                dns_cache_capacity: 64,
            }),
            links: Vec::new(),
            generation: 0,
        }
    }

    fn keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![HEADER_RECORD_KEY];
        for (index, tcp) in self.stack.tcp.iter().enumerate().take(MAX_TCP_RECORDS) {
            if tcp.state == TcpState::Established || tcp.smoltcp_migration.is_some() {
                keys.push(TCP_RECORD_BASE + index as u64);
            }
        }
        for index in 0..self.links.len() {
            let base = LINK_QUEUES + index as u64 * LINK_STRIDE;
            keys.extend(base..base + LINK_STRIDE);
        }
        keys
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key >= LINK_QUEUES {
            let offset = key - LINK_QUEUES;
            let index = usize::try_from(offset / LINK_STRIDE).map_err(|_| Error::InvalidData)?;
            let record = offset % LINK_STRIDE;
            let Some(node_link) = self.links.get(index) else {
                return Ok(None);
            };
            return if record == 0 {
                node_link.link.checkpoint_queues()
            } else {
                node_link.link.checkpoint_backlog((record - 1) as usize)
            }
            .map(Some);
        }
        if key >= TCP_RECORD_BASE {
            let index = usize::try_from(key - TCP_RECORD_BASE).map_err(|_| Error::InvalidData)?;
            let tcp = self.stack.tcp.get(index).ok_or(Error::InvalidData)?;
            let snapshot = self.tcp_checkpoint(index).ok_or(Error::InvalidData)?;
            let mut w = Encoder::new();
            w.word(index as u64);
            w.word(tcp.control);
            encode_tcp_snapshot(&mut w, &snapshot);
            return Ok(Some(w.finish()));
        }
        if key != HEADER_RECORD_KEY {
            return Err(Error::InvalidData);
        }
        let mut w = Encoder::new();
        w.word(8);
        w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
        w.word(self.control.0);
        w.word(self.migration.map_or(0, |channel| channel.0));
        w.word(self.tls_trust.map_or(0, |channel| channel.0));
        w.word(self.generation);
        encode_config(&mut w, self.stack.config.clone());
        w.word(self.stack.next_ephemeral_port as u64);
        w.word(self.clients.len() as u64);
        for client in &self.clients {
            w.word(client.channel.0);
            w.word(client.allowed_methods.len() as u64);
            for ordinal in &client.allowed_methods {
                w.word(*ordinal);
            }
        }
        encode_bound_clients(&mut w, &self.backend_clients);
        encode_bound_clients(&mut w, &self.controller_clients);
        w.word(self.backend_connections.len() as u64);
        for (connection, table) in &self.backend_connections {
            w.word(*connection);
            w.word(u64::from(*table));
        }
        w.word(self.backend_controls.len() as u64);
        for (object, (kind, control)) in &self.backend_controls {
            w.word(*object);
            w.word(u64::from(*kind));
            w.word(*control);
        }
        w.word(self.link_watchers.len() as u64);
        for watcher in &self.link_watchers {
            w.word(*watcher);
        }
        w.word(self.stack.tcp.len() as u64);
        for tcp in &self.stack.tcp {
            encode_tcp(&mut w, tcp);
        }
        w.word(self.stack.listeners.len() as u64);
        for listener in &self.stack.listeners {
            encode_listener(&mut w, listener);
        }
        w.word(self.stack.udp.len() as u64);
        for udp in &self.stack.udp {
            encode_udp_v7(&mut w, udp);
        }
        w.word(self.stack.dns.entries().len() as u64);
        for entry in self.stack.dns.entries() {
            w.text(&entry.hostname);
            w.word(entry.expires_at_ms);
            w.word(entry.records.len() as u64);
            for record in &entry.records {
                match record {
                    DnsRecord::A(address) => {
                        w.word(1);
                        w.bytes(address);
                    }
                    DnsRecord::Aaaa(address) => {
                        w.word(28);
                        w.bytes(address);
                    }
                }
            }
        }
        w.word(self.stack.dns.pending().len() as u64);
        for pending in self.stack.dns.pending() {
            w.text(&pending.hostname);
            w.word(pending.id as u64);
            w.word(match pending.transport {
                DnsTransport::Udp53 => 1,
                DnsTransport::Doh => 2,
            });
        }
        w.word(self.links.len() as u64);
        for node_link in &self.links {
            w.word(node_link.node_id);
            encode_link(&mut w, node_link.link.resources);
        }
        encode_router(&mut w, &self.router)?;
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key >= LINK_QUEUES {
            let Some(bytes) = bytes else {
                return Ok(());
            };
            let offset = key - LINK_QUEUES;
            let index = usize::try_from(offset / LINK_STRIDE).map_err(|_| Error::InvalidData)?;
            let record = offset % LINK_STRIDE;
            let link = &mut self.links.get_mut(index).ok_or(Error::InvalidData)?.link;
            return if record == 0 {
                link.adopt_queues(bytes)
            } else {
                link.adopt_backlog((record - 1) as usize, bytes)
            };
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if key >= TCP_RECORD_BASE {
            let index = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            if key != TCP_RECORD_BASE + index as u64 {
                return Err(Error::InvalidData);
            }
            let control = r.word()?;
            let snapshot = decode_tcp_snapshot(&mut r)?;
            r.finish()?;
            let tcp = self.stack.tcp.get_mut(index).ok_or(Error::InvalidData)?;
            if tcp.control != control {
                return Err(Error::InvalidData);
            }
            tcp.smoltcp_migration = Some(snapshot);
            return Ok(());
        }
        if key != HEADER_RECORD_KEY {
            return Err(Error::InvalidData);
        }
        let version = r.word()?;
        if !(3..=8).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let architecture = if version < 5 { 1 } else { r.word()? };
        if architecture != if cfg!(bexos_arch_x86_64) { 2 } else { 1 } {
            return Err(Error::InvalidData);
        }
        self.control = Channel(r.word()?);
        let migration = r.word()?;
        self.migration = (migration != 0).then_some(Channel(migration));
        let tls_trust = r.word()?;
        self.tls_trust = (tls_trust != 0).then_some(Channel(tls_trust));
        self.generation = r.word()?;
        // Header deltas describe control-plane state. Preserve packet records
        // already adopted for the same endpoint until their own deltas arrive.
        let previous_tcp = core::mem::take(&mut self.stack.tcp);
        self.stack = Netstack::from_active(decode_config(&mut r, version)?);
        self.stack.next_ephemeral_port = r.word()? as u16;
        self.clients.clear();
        for _ in 0..r.count(256)? {
            let channel = Channel(r.word()?);
            let mut allowed_methods = Vec::new();
            for _ in 0..r.count(64)? {
                allowed_methods.push(r.word()?);
            }
            self.clients
                .push(BoundServiceEndpoint::new(channel, allowed_methods));
        }
        self.backend_clients = if version >= 7 {
            decode_bound_clients(&mut r)?
        } else {
            Vec::new()
        };
        self.controller_clients = if version >= 7 {
            decode_bound_clients(&mut r)?
        } else {
            Vec::new()
        };
        self.backend_connections.clear();
        if version >= 7 {
            for _ in 0..r.count(4096)? {
                self.backend_connections.insert(
                    r.word()?,
                    u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                );
            }
        }
        self.backend_controls.clear();
        if version >= 8 {
            for _ in 0..r.count(4096)? {
                let object = r.word()?;
                let kind = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let control = r.word()?;
                if !matches!(kind, 1 | 2) || control == 0 {
                    return Err(Error::InvalidData);
                }
                self.backend_controls.insert(object, (kind, control));
            }
        }
        self.link_watchers.clear();
        for _ in 0..r.count(64)? {
            self.link_watchers.push(r.word()?);
        }
        self.stack.tcp.clear();
        for _ in 0..r.count(64)? {
            let mut tcp = decode_tcp(&mut r)?;
            if let Some(previous) = previous_tcp.iter().find(|previous| {
                previous.control == tcp.control
                    && previous.peer == tcp.peer
                    && previous.local == tcp.local
            }) {
                tcp.smoltcp_migration = previous.smoltcp_migration.clone();
            }
            self.stack.tcp.push(tcp);
        }
        self.stack.listeners.clear();
        for _ in 0..r.count(32)? {
            self.stack.listeners.push(decode_listener(&mut r)?);
        }
        self.stack.udp.clear();
        for _ in 0..r.count(64)? {
            self.stack.udp.push(if version >= 7 {
                decode_udp_v7(&mut r)?
            } else {
                decode_udp(&mut r)?
            });
        }
        let mut entries = Vec::new();
        for _ in 0..r.count(64)? {
            let hostname = r.text(255)?.to_string();
            let expires_at_ms = r.word()?;
            let mut records = Vec::new();
            for _ in 0..r.count(8)? {
                match r.word()? {
                    1 => {
                        let bytes = r.bytes(4)?;
                        if bytes.len() != 4 {
                            return Err(Error::InvalidData);
                        }
                        records.push(DnsRecord::A([bytes[0], bytes[1], bytes[2], bytes[3]]));
                    }
                    28 => {
                        let bytes = r.bytes(16)?;
                        if bytes.len() != 16 {
                            return Err(Error::InvalidData);
                        }
                        let mut address = [0u8; 16];
                        address.copy_from_slice(bytes);
                        records.push(DnsRecord::Aaaa(address));
                    }
                    _ => return Err(Error::InvalidData),
                }
            }
            entries.push(DnsEntry {
                hostname,
                records,
                expires_at_ms,
            });
        }
        self.stack.dns.replace_entries(entries);
        let mut pending = Vec::new();
        for _ in 0..r.count(32)? {
            pending.push(PendingDnsQuery {
                hostname: r.text(255)?.to_string(),
                id: r.word()? as u16,
                transport: match r.word()? {
                    1 => DnsTransport::Udp53,
                    2 => DnsTransport::Doh,
                    _ => return Err(Error::InvalidData),
                },
            });
        }
        self.stack.dns.replace_pending(pending);
        let mut previous_links = core::mem::take(&mut self.links);
        self.links = if version >= 6 {
            let mut links = Vec::new();
            for _ in 0..r.count(MAX_LINKS)? {
                let node_id = r.word()?;
                let resources = decode_link(&mut r)?;
                if node_id == 0 || links.iter().any(|link: &NodeLink| link.node_id == node_id) {
                    return Err(Error::InvalidData);
                }
                let link = if let Some(index) = previous_links
                    .iter()
                    .position(|link| link.node_id == node_id && link.link.resources == resources)
                {
                    previous_links.remove(index).link
                } else {
                    let mut link = PacketLink::from_resources(resources);
                    link.expect_queues();
                    link
                };
                links.push(NodeLink { node_id, link });
            }
            links.sort_by_key(|link| link.node_id);
            links
        } else if r.flag()? {
            let resources = decode_link(&mut r)?;
            let mut link = PacketLink::from_resources(resources);
            if version >= 5 {
                link.expect_queues();
            }
            alloc::vec![NodeLink { node_id: 1, link }]
        } else {
            Vec::new()
        };
        self.router = if version >= 7 {
            decode_router(&mut r)?
        } else {
            Router::new(self.stack.config.clone())
        };
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        if self.links.len() > MAX_LINKS
            || self
                .links
                .windows(2)
                .any(|pair| pair[0].node_id >= pair[1].node_id)
            || self.links.iter().any(|node_link| {
                node_link.node_id == 0
                    || node_link.link.resources.fifo == 0
                    || node_link.link.resources.rx_vaddr == 0
                    || !node_link.link.queues_valid()
            })
        {
            return Err(Error::InvalidData);
        }
        for (index, tcp) in self.stack.tcp.iter().enumerate() {
            if tcp.state == TcpState::Established && self.tcp_checkpoint(index).is_none() {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut resources = alloc::vec![Resource::Handle(self.control.0)];
        if let Some(migration) = self.migration {
            resources.push(Resource::Handle(migration.0));
        }
        if let Some(tls_trust) = self.tls_trust {
            resources.push(Resource::Handle(tls_trust.0));
        }
        resources.extend(
            self.clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        resources.extend(
            self.backend_clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        resources.extend(
            self.controller_clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        resources.extend(self.link_watchers.iter().copied().map(Resource::Handle));
        for tcp in &self.stack.tcp {
            if !self.backend_connections.contains_key(&tcp.control) {
                resources.push(Resource::Handle(tcp.control));
            }
            if let Some(client) = tcp.client_control {
                resources.push(Resource::Handle(client));
            }
            if let Some(stream) = tcp.stream {
                resources.push(Resource::Handle(stream.0));
            }
        }
        for listener in &self.stack.listeners {
            for pending in &listener.pending {
                resources.push(Resource::Handle(pending.control));
                if let Some(client) = pending.client_control {
                    resources.push(Resource::Handle(client));
                }
            }
        }
        resources.extend(
            self.stack
                .listeners
                .iter()
                .map(|listener| Resource::Handle(listener.control)),
        );
        resources.extend(
            self.stack
                .udp
                .iter()
                .map(|socket| Resource::Handle(socket.control)),
        );
        for node_link in &self.links {
            let r = node_link.link.resources;
            resources.extend([
                Resource::Handle(r.control),
                Resource::Handle(r.fifo),
                Resource::Handle(r.rx_vmo),
                Resource::Handle(r.tx_vmo),
                Resource::Mapping {
                    handle: r.rx_vmo,
                    offset: 0,
                    va: r.rx_vaddr,
                    size: crate::ethernet::RX_BYTES,
                    rights: 6,
                },
                Resource::Mapping {
                    handle: r.tx_vmo,
                    offset: 0,
                    va: r.tx_vaddr,
                    size: crate::ethernet::TX_BYTES,
                    rights: 6,
                },
            ]);
        }
        append_router_resources(&mut resources, &self.router, &self.backend_connections);
        resources
    }

    fn activated(&mut self, generation: u64) {
        self.generation = generation;
        if self
            .stack
            .restore_after_migration(self.links.first_mut().map(|link| &mut link.link))
            != net_fidl::Status::Ok
        {
            let _ = bexos_userspace::migration::abort();
        }
        for table in self.router.tables.values_mut() {
            let mut ids = table.links.keys().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            let status = ids
                .first()
                .and_then(|id| table.links.get_mut(id))
                .map_or(net_fidl::Status::Ok, |link| {
                    table.stack.restore_after_migration(Some(link))
                });
            if status != net_fidl::Status::Ok {
                let _ = bexos_userspace::migration::abort();
            }
        }
    }
}

impl Runtime {
    fn tcp_checkpoint(&self, index: usize) -> Option<TcpMigrationState> {
        let tcp = self.stack.tcp.get(index)?;
        self.stack
            .smoltcp
            .as_ref()
            .and_then(|runtime| runtime.checkpoint_tcp(tcp))
            .or_else(|| tcp.smoltcp_migration.clone())
    }
}

fn encode_bound_clients(w: &mut Encoder, clients: &[BoundServiceEndpoint]) {
    w.word(clients.len() as u64);
    for client in clients {
        w.word(client.channel.0);
        w.text(&client.protocol);
        w.word(client.allowed_methods.len() as u64);
        for ordinal in &client.allowed_methods {
            w.word(*ordinal);
        }
    }
}

fn decode_bound_clients(r: &mut Decoder<'_>) -> Result<Vec<BoundServiceEndpoint>, Error> {
    let mut clients = Vec::new();
    for _ in 0..r.count(256)? {
        let channel = Channel(r.word()?);
        let protocol = r.text(64)?.to_string();
        let mut methods = Vec::new();
        for _ in 0..r.count(64)? {
            methods.push(r.word()?);
        }
        clients.push(BoundServiceEndpoint::new_with_protocol(
            channel, methods, &protocol,
        ));
    }
    Ok(clients)
}

fn encode_router(w: &mut Encoder, router: &Router) -> Result<(), Error> {
    encode_config(w, router.default_config.clone());
    w.word(router.tables.len() as u64);
    for table in router.tables.values() {
        w.word(u64::from(table.id));
        for quota in [
            table.quota.tcp,
            table.quota.listeners,
            table.quota.udp,
            table.quota.interfaces,
            table.quota.routes,
        ] {
            w.word(quota as u64);
        }
        w.word(table.interfaces.len() as u64);
        for interface in table.interfaces.values() {
            w.word(interface.id);
            w.text(&interface.name);
            w.word(interface.up as u64);
            w.word(interface.neighbor_generation);
            w.word(interface.packet_generation);
            if let Some(link) = table.links.get(&interface.id) {
                w.word(1);
                encode_link(w, link.resources);
                w.bytes(&link.checkpoint_queues()?);
                w.word(crate::link::BACKLOG_CHUNKS as u64);
                for chunk in 0..crate::link::BACKLOG_CHUNKS {
                    w.bytes(&link.checkpoint_backlog(chunk)?);
                }
            } else {
                w.word(0);
            }
        }
        w.word(table.fib.len() as u64);
        for route in &table.fib {
            encode_socket_addr(
                w,
                SocketAddress {
                    addr: route.destination,
                    port: 0,
                },
            );
            w.word(u64::from(route.prefix_len));
            w.word(route.gateway.is_some() as u64);
            if let Some(gateway) = route.gateway {
                encode_socket_addr(
                    w,
                    SocketAddress {
                        addr: gateway,
                        port: 0,
                    },
                );
            }
            w.word(route.interface_id);
            w.word(u64::from(route.metric));
        }
        encode_stack_v7(w, &table.stack)?;
    }
    Ok(())
}

fn decode_router(r: &mut Decoder<'_>) -> Result<Router, Error> {
    let default_config = decode_config(r, 7)?;
    let mut router = Router::new(default_config.clone());
    for _ in 0..r.count(64)? {
        let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        if id != 0 && router.tables.contains_key(&id) {
            return Err(Error::InvalidData);
        }
        let quota = TableQuota {
            tcp: r.count(4096)?,
            listeners: r.count(4096)?,
            udp: r.count(4096)?,
            interfaces: r.count(256)?,
            routes: r.count(4096)?,
        };
        let mut interfaces = alloc::collections::BTreeMap::new();
        let mut links = alloc::collections::BTreeMap::new();
        for _ in 0..r.count(quota.interfaces.min(256))? {
            let interface = InterfaceState {
                id: r.word()?,
                name: r.text(32)?.to_string(),
                up: r.flag()?,
                neighbor_generation: r.word()?,
                packet_generation: r.word()?,
            };
            if interface.id == 0 || interfaces.contains_key(&interface.id) {
                return Err(Error::InvalidData);
            }
            if r.flag()? {
                let resources = decode_link(r)?;
                let queues = r.bytes(2 * 1024 * 1024)?.to_vec();
                let chunks = r.count(crate::link::BACKLOG_CHUNKS)?;
                if chunks != crate::link::BACKLOG_CHUNKS {
                    return Err(Error::InvalidData);
                }
                let mut link = PacketLink::from_resources(resources);
                link.adopt_queues(&queues)?;
                for chunk in 0..chunks {
                    let bytes = r.bytes(2 * 1024 * 1024)?;
                    link.adopt_backlog(chunk, bytes)?;
                }
                links.insert(interface.id, link);
            }
            router.interface_table.insert(interface.id, id);
            interfaces.insert(interface.id, interface);
        }
        let mut fib = Vec::new();
        for _ in 0..r.count(quota.routes.min(4096))? {
            let destination = decode_socket_addr(r)?.addr;
            let prefix_len = r.count(128)? as u8;
            let gateway = if r.flag()? {
                Some(decode_socket_addr(r)?.addr)
            } else {
                None
            };
            fib.push(FibRoute {
                destination,
                prefix_len,
                gateway,
                interface_id: r.word()?,
                metric: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            });
        }
        let stack = decode_stack_v7(r)?;
        router.tables.insert(
            id,
            Table {
                id,
                stack,
                fib,
                interfaces,
                links,
                quota,
            },
        );
    }
    Ok(router)
}

fn encode_stack_v7(w: &mut Encoder, stack: &Netstack) -> Result<(), Error> {
    encode_config(w, stack.config.clone());
    w.word(u64::from(stack.next_ephemeral_port));
    w.word(stack.tcp.len() as u64);
    for tcp in &stack.tcp {
        encode_tcp(w, tcp);
        let snapshot = stack
            .smoltcp
            .as_ref()
            .and_then(|runtime| runtime.checkpoint_tcp(tcp))
            .or_else(|| tcp.smoltcp_migration.clone());
        w.word(snapshot.is_some() as u64);
        if let Some(snapshot) = snapshot {
            encode_tcp_snapshot(w, &snapshot);
        }
    }
    w.word(stack.listeners.len() as u64);
    for listener in &stack.listeners {
        encode_listener(w, listener);
    }
    w.word(stack.udp.len() as u64);
    for udp in &stack.udp {
        encode_udp_v7(w, udp);
    }
    encode_dns_state(w, stack);
    Ok(())
}

fn decode_stack_v7(r: &mut Decoder<'_>) -> Result<Netstack, Error> {
    let mut stack = Netstack::from_active(decode_config(r, 7)?);
    stack.next_ephemeral_port = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    for _ in 0..r.count(4096)? {
        let mut tcp = decode_tcp(r)?;
        if r.flag()? {
            tcp.smoltcp_migration = Some(decode_tcp_snapshot(r)?);
        }
        stack.tcp.push(tcp);
    }
    for _ in 0..r.count(4096)? {
        stack.listeners.push(decode_listener(r)?);
    }
    for _ in 0..r.count(4096)? {
        stack.udp.push(decode_udp_v7(r)?);
    }
    decode_dns_state(r, &mut stack)?;
    Ok(stack)
}

fn encode_dns_state(w: &mut Encoder, stack: &Netstack) {
    w.word(stack.dns.entries().len() as u64);
    for entry in stack.dns.entries() {
        w.text(&entry.hostname);
        w.word(entry.expires_at_ms);
        w.word(entry.records.len() as u64);
        for record in &entry.records {
            match record {
                DnsRecord::A(address) => {
                    w.word(1);
                    w.bytes(address);
                }
                DnsRecord::Aaaa(address) => {
                    w.word(28);
                    w.bytes(address);
                }
            }
        }
    }
    w.word(stack.dns.pending().len() as u64);
    for pending in stack.dns.pending() {
        w.text(&pending.hostname);
        w.word(u64::from(pending.id));
        w.word(match pending.transport {
            DnsTransport::Udp53 => 1,
            DnsTransport::Doh => 2,
        });
    }
}

fn decode_dns_state(r: &mut Decoder<'_>, stack: &mut Netstack) -> Result<(), Error> {
    let mut entries = Vec::new();
    for _ in 0..r.count(1024)? {
        let hostname = r.text(255)?.to_string();
        let expires_at_ms = r.word()?;
        let mut records = Vec::new();
        for _ in 0..r.count(8)? {
            match r.word()? {
                1 => {
                    let bytes = r.bytes(4)?;
                    records.push(DnsRecord::A(
                        bytes.try_into().map_err(|_| Error::InvalidData)?,
                    ));
                }
                28 => {
                    let bytes = r.bytes(16)?;
                    records.push(DnsRecord::Aaaa(
                        bytes.try_into().map_err(|_| Error::InvalidData)?,
                    ));
                }
                _ => return Err(Error::InvalidData),
            }
        }
        entries.push(DnsEntry {
            hostname,
            records,
            expires_at_ms,
        });
    }
    stack.dns.replace_entries(entries);
    let mut pending = Vec::new();
    for _ in 0..r.count(256)? {
        pending.push(PendingDnsQuery {
            hostname: r.text(255)?.to_string(),
            id: u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            transport: match r.word()? {
                1 => DnsTransport::Udp53,
                2 => DnsTransport::Doh,
                _ => return Err(Error::InvalidData),
            },
        });
    }
    stack.dns.replace_pending(pending);
    Ok(())
}

fn append_router_resources(
    resources: &mut Vec<Resource>,
    router: &Router,
    backend_connections: &BTreeMap<u64, u32>,
) {
    for table in router.tables.values() {
        for tcp in &table.stack.tcp {
            if tcp.control != 0 && !backend_connections.contains_key(&tcp.control) {
                resources.push(Resource::Handle(tcp.control));
            }
            if let Some(client) = tcp.client_control {
                resources.push(Resource::Handle(client));
            }
            if let Some(stream) = tcp.stream {
                resources.push(Resource::Handle(stream.0));
            }
        }
        for listener in &table.stack.listeners {
            if listener.control != 0 {
                resources.push(Resource::Handle(listener.control));
            }
        }
        for udp in &table.stack.udp {
            if udp.control != 0 {
                resources.push(Resource::Handle(udp.control));
            }
        }
        for link in table.links.values() {
            let r = link.resources;
            resources.extend([
                Resource::Handle(r.control),
                Resource::Handle(r.fifo),
                Resource::Handle(r.rx_vmo),
                Resource::Handle(r.tx_vmo),
                Resource::Mapping {
                    handle: r.rx_vmo,
                    offset: 0,
                    va: r.rx_vaddr,
                    size: crate::ethernet::RX_BYTES,
                    rights: 6,
                },
                Resource::Mapping {
                    handle: r.tx_vmo,
                    offset: 0,
                    va: r.tx_vaddr,
                    size: crate::ethernet::TX_BYTES,
                    rights: 6,
                },
            ]);
        }
    }
}

fn encode_config(w: &mut Encoder, config: ActiveConfig) {
    w.word(match config.source {
        ConfigSource::Static => 1,
        ConfigSource::Dhcp => 2,
        ConfigSource::Unconfigured => 3,
    });
    encode_optional_ipv4(w, config.ipv4);
    w.word(config.prefix_len as u64);
    encode_optional_ipv4(w, config.gateway);
    encode_optional_ipv4(w, config.dns);
    encode_optional_ipv6(w, config.ipv6);
    w.word(config.ipv6_prefix_len as u64);
    encode_optional_ipv6(w, config.ipv6_gateway);
    encode_optional_ipv6(w, config.dns_ipv6);
    encode_optional_ipv6(w, config.doh_bootstrap_ipv6);
    w.word(config.slaac_enabled as u64);
    w.word(config.mtu as u64);
    w.word(match config.dns_mode {
        DnsMode::Udp53 => 1,
        DnsMode::Doh => 2,
    });
    w.text(&config.doh_host);
    w.text(&config.doh_path);
    encode_optional_ipv4(w, config.doh_bootstrap_ipv4);
    w.word(config.doh_port as u64);
    w.word(config.doh_strict as u64);
    w.word(config.dns_cache_capacity as u64);
}

fn decode_config(r: &mut Decoder<'_>, version: u64) -> Result<ActiveConfig, Error> {
    let source = match r.word()? {
        1 => ConfigSource::Static,
        2 => ConfigSource::Dhcp,
        3 => ConfigSource::Unconfigured,
        _ => return Err(Error::InvalidData),
    };
    let ipv4 = decode_optional_ipv4(r)?;
    let prefix_len = r.count(32)? as u8;
    let gateway = decode_optional_ipv4(r)?;
    let dns = decode_optional_ipv4(r)?;
    let (ipv6, ipv6_prefix_len, ipv6_gateway, dns_ipv6, doh_bootstrap_ipv6, slaac_enabled) =
        if version >= 4 {
            (
                decode_optional_ipv6(r)?,
                r.count(128)? as u8,
                decode_optional_ipv6(r)?,
                decode_optional_ipv6(r)?,
                decode_optional_ipv6(r)?,
                r.word()? != 0,
            )
        } else {
            (None, 0, None, None, None, true)
        };
    let mtu = r.count(9000)? as u32;
    let dns_mode = match r.word()? {
        1 => DnsMode::Udp53,
        2 => DnsMode::Doh,
        _ => return Err(Error::InvalidData),
    };
    let doh_host = r.text(255)?.to_string();
    let doh_path = r.text(255)?.to_string();
    let doh_bootstrap_ipv4 = decode_optional_ipv4(r)?;
    let doh_port = r.count(65535)? as u16;
    let doh_strict = r.word()? != 0;
    let dns_cache_capacity = r.count(1024)? as u32;
    Ok(ActiveConfig {
        source,
        ipv4,
        prefix_len,
        gateway,
        dns,
        ipv6,
        ipv6_prefix_len,
        ipv6_gateway,
        dns_ipv6,
        doh_bootstrap_ipv6,
        slaac_enabled,
        mtu,
        dns_mode,
        doh_host,
        doh_path,
        doh_bootstrap_ipv4,
        doh_port,
        doh_strict,
        dns_cache_capacity,
    })
}

fn encode_tcp(w: &mut Encoder, tcp: &TcpEndpoint) {
    w.word(tcp.control);
    w.word(tcp.stream.map_or(0, |socket| socket.0));
    w.word(tcp.client_control.unwrap_or(0));
    encode_socket_addr(w, tcp.peer);
    encode_socket_addr(w, tcp.local);
    w.word(match tcp.state {
        TcpState::Connecting => 1,
        TcpState::Established => 2,
        TcpState::Listening => 3,
        TcpState::Closed => 4,
    });
}

fn decode_tcp(r: &mut Decoder<'_>) -> Result<TcpEndpoint, Error> {
    let control = r.word()?;
    let stream = r.word()?;
    let client_control = r.word()?;
    let peer = decode_socket_addr(r)?;
    let local = decode_socket_addr(r)?;
    let state = match r.word()? {
        1 => TcpState::Connecting,
        2 => TcpState::Established,
        3 => TcpState::Listening,
        4 => TcpState::Closed,
        _ => return Err(Error::InvalidData),
    };
    Ok(TcpEndpoint {
        control,
        stream: (stream != 0).then_some(bexos_userspace::Socket(stream)),
        client_control: (client_control != 0).then_some(client_control),
        peer,
        local,
        state,
        smoltcp_handle: None,
        smoltcp_migration: None,
    })
}

fn encode_tcp_snapshot(w: &mut Encoder, snapshot: &TcpMigrationState) {
    w.word(match snapshot.state {
        SmoltcpTcpState::Closed => 0,
        SmoltcpTcpState::Listen => 1,
        SmoltcpTcpState::SynSent => 2,
        SmoltcpTcpState::SynReceived => 3,
        SmoltcpTcpState::Established => 4,
        SmoltcpTcpState::FinWait1 => 5,
        SmoltcpTcpState::FinWait2 => 6,
        SmoltcpTcpState::CloseWait => 7,
        SmoltcpTcpState::Closing => 8,
        SmoltcpTcpState::LastAck => 9,
        SmoltcpTcpState::TimeWait => 10,
    });
    encode_smoltcp_listen_endpoint(w, snapshot.listen_endpoint);
    encode_optional_smoltcp_endpoint(w, snapshot.local_endpoint);
    encode_optional_smoltcp_endpoint(w, snapshot.remote_endpoint);
    encode_i32(w, snapshot.local_seq_no);
    encode_i32(w, snapshot.remote_seq_no);
    encode_i32(w, snapshot.remote_last_seq);
    encode_optional_i32(w, snapshot.remote_last_ack);
    w.word(snapshot.remote_last_win as u64);
    w.word(snapshot.remote_win_shift as u64);
    w.word(snapshot.remote_win_len as u64);
    encode_optional_u8(w, snapshot.remote_win_scale);
    w.word(snapshot.remote_has_sack as u64);
    w.word(snapshot.remote_mss as u64);
    encode_optional_i64(w, snapshot.remote_last_ts_delta_millis);
    encode_optional_i32(w, snapshot.local_rx_last_seq);
    encode_optional_i32(w, snapshot.local_rx_last_ack);
    w.word(snapshot.local_rx_dup_acks as u64);
    w.word(snapshot.pending_fast_retransmit as u64);
    w.word(snapshot.rx_fin_received as u64);
    encode_optional_u64(w, snapshot.timeout_millis);
    encode_optional_u64(w, snapshot.keep_alive_millis);
    encode_optional_u8(w, snapshot.hop_limit);
    encode_optional_u64(w, snapshot.ack_delay_millis);
    w.word(snapshot.nagle as u64);
    w.word(snapshot.last_remote_tsval as u64);
    encode_rtt(w, &snapshot.rtte);
    w.word(snapshot.assembler_ranges.len() as u64);
    for (hole, data) in &snapshot.assembler_ranges {
        w.word(*hole as u64);
        w.word(*data as u64);
    }
    w.bytes(&snapshot.rx_bytes);
    w.bytes(&snapshot.tx_bytes);
}

fn decode_tcp_snapshot(r: &mut Decoder<'_>) -> Result<TcpMigrationState, Error> {
    let state = match r.word()? {
        0 => SmoltcpTcpState::Closed,
        1 => SmoltcpTcpState::Listen,
        2 => SmoltcpTcpState::SynSent,
        3 => SmoltcpTcpState::SynReceived,
        4 => SmoltcpTcpState::Established,
        5 => SmoltcpTcpState::FinWait1,
        6 => SmoltcpTcpState::FinWait2,
        7 => SmoltcpTcpState::CloseWait,
        8 => SmoltcpTcpState::Closing,
        9 => SmoltcpTcpState::LastAck,
        10 => SmoltcpTcpState::TimeWait,
        _ => return Err(Error::InvalidData),
    };
    let listen_endpoint = decode_smoltcp_listen_endpoint(r)?;
    let local_endpoint = decode_optional_smoltcp_endpoint(r)?;
    let remote_endpoint = decode_optional_smoltcp_endpoint(r)?;
    let local_seq_no = decode_i32(r)?;
    let remote_seq_no = decode_i32(r)?;
    let remote_last_seq = decode_i32(r)?;
    let remote_last_ack = decode_optional_i32(r)?;
    let remote_last_win = r.word()? as u16;
    let remote_win_shift = r.word()? as u8;
    let remote_win_len = r.word()? as usize;
    let remote_win_scale = decode_optional_u8(r)?;
    let remote_has_sack = r.flag()?;
    let remote_mss = r.word()? as usize;
    let remote_last_ts_delta_millis = decode_optional_i64(r)?;
    let local_rx_last_seq = decode_optional_i32(r)?;
    let local_rx_last_ack = decode_optional_i32(r)?;
    let local_rx_dup_acks = r.word()? as u8;
    let pending_fast_retransmit = r.flag()?;
    let rx_fin_received = r.flag()?;
    let timeout_millis = decode_optional_u64(r)?;
    let keep_alive_millis = decode_optional_u64(r)?;
    let hop_limit = decode_optional_u8(r)?;
    let ack_delay_millis = decode_optional_u64(r)?;
    let nagle = r.flag()?;
    let last_remote_tsval = r.word()? as u32;
    let rtte = decode_rtt(r)?;
    let mut assembler_ranges = Vec::new();
    for _ in 0..r.count(64)? {
        assembler_ranges.push((r.word()? as usize, r.word()? as usize));
    }
    let rx_bytes = r.bytes(8192)?.to_vec();
    let tx_bytes = r.bytes(8192)?.to_vec();
    Ok(TcpMigrationState {
        state,
        listen_endpoint,
        local_endpoint,
        remote_endpoint,
        local_seq_no,
        remote_seq_no,
        remote_last_seq,
        remote_last_ack,
        remote_last_win,
        remote_win_shift,
        remote_win_len,
        remote_win_scale,
        remote_has_sack,
        remote_mss,
        remote_last_ts_delta_millis,
        local_rx_last_seq,
        local_rx_last_ack,
        local_rx_dup_acks,
        pending_fast_retransmit,
        rx_fin_received,
        timeout_millis,
        keep_alive_millis,
        hop_limit,
        ack_delay_millis,
        nagle,
        last_remote_tsval,
        rtte,
        assembler_ranges,
        rx_bytes,
        tx_bytes,
    })
}

fn encode_rtt(w: &mut Encoder, rtt: &MigrationRttEstimator) {
    w.word(rtt.have_measurement as u64);
    w.word(rtt.srtt_millis as u64);
    w.word(rtt.rttvar_millis as u64);
    w.word(rtt.rto_millis as u64);
    w.word(rtt.timestamp_delta_millis.is_some() as u64);
    if let Some((delta, seq)) = rtt.timestamp_delta_millis {
        encode_i64(w, delta);
        encode_i32(w, seq);
    }
    encode_optional_i32(w, rtt.max_seq_sent);
    w.word(rtt.rto_count as u64);
}

fn decode_rtt(r: &mut Decoder<'_>) -> Result<MigrationRttEstimator, Error> {
    let have_measurement = r.flag()?;
    let srtt_millis = r.word()? as u32;
    let rttvar_millis = r.word()? as u32;
    let rto_millis = r.word()? as u32;
    let timestamp_delta_millis = if r.flag()? {
        Some((decode_i64(r)?, decode_i32(r)?))
    } else {
        None
    };
    let max_seq_sent = decode_optional_i32(r)?;
    let rto_count = r.word()? as u8;
    Ok(MigrationRttEstimator {
        have_measurement,
        srtt_millis,
        rttvar_millis,
        rto_millis,
        timestamp_delta_millis,
        max_seq_sent,
        rto_count,
    })
}

fn encode_listener(w: &mut Encoder, listener: &TcpListenerState) {
    w.word(listener.control);
    encode_socket_addr(w, listener.local);
    w.word(listener.closed as u64);
    w.word(listener.pending.len() as u64);
    for pending in &listener.pending {
        encode_tcp(w, pending);
    }
}

fn decode_listener(r: &mut Decoder<'_>) -> Result<TcpListenerState, Error> {
    let control = r.word()?;
    let local = decode_socket_addr(r)?;
    let closed = r.flag()?;
    let mut pending = Vec::new();
    for _ in 0..r.count(64)? {
        pending.push(decode_tcp(r)?);
    }
    Ok(TcpListenerState {
        control,
        local,
        pending,
        closed,
        smoltcp_handle: None,
    })
}

fn encode_udp(w: &mut Encoder, udp: &UdpEndpoint) {
    w.word(udp.control);
    w.word(udp.local.is_some() as u64);
    if let Some(local) = udp.local {
        encode_socket_addr(w, local);
    }
    w.word(udp.closed as u64);
}

fn decode_udp(r: &mut Decoder<'_>) -> Result<UdpEndpoint, Error> {
    let mut udp = UdpEndpoint::new(r.word()?);
    if r.flag()? {
        udp.local = Some(decode_socket_addr(r)?);
    }
    udp.closed = r.flag()?;
    Ok(udp)
}

fn encode_udp_v7(w: &mut Encoder, udp: &UdpEndpoint) {
    encode_udp(w, udp);
    w.word(udp.queue.len() as u64);
    for packet in &udp.queue {
        w.bytes(&packet.data[..packet.len]);
        encode_socket_addr(w, packet.source);
    }
    w.word(udp.outbound.len() as u64);
    for packet in &udp.outbound {
        w.bytes(&packet.data[..packet.len]);
        encode_socket_addr(w, packet.source);
        encode_socket_addr(w, packet.destination);
    }
}

fn decode_udp_v7(r: &mut Decoder<'_>) -> Result<UdpEndpoint, Error> {
    let mut udp = decode_udp(r)?;
    for _ in 0..r.count(64)? {
        let bytes = r.bytes(8192)?;
        let mut data = [0; 8192];
        data[..bytes.len()].copy_from_slice(bytes);
        udp.queue.push_back(Datagram {
            data,
            len: bytes.len(),
            source: decode_socket_addr(r)?,
        });
    }
    for _ in 0..r.count(64)? {
        let bytes = r.bytes(8192)?;
        let mut data = [0; 8192];
        data[..bytes.len()].copy_from_slice(bytes);
        udp.outbound.push_back(OutboundDatagram {
            data,
            len: bytes.len(),
            source: decode_socket_addr(r)?,
            destination: decode_socket_addr(r)?,
        });
    }
    Ok(udp)
}

fn encode_link(w: &mut Encoder, link: LinkResources) {
    for word in [
        link.control,
        link.fifo,
        link.rx_vmo,
        link.tx_vmo,
        link.rx_vaddr,
        link.tx_vaddr,
        link.rx_vmo_id as u64,
        link.tx_vmo_id as u64,
        link.mtu as u64,
    ] {
        w.word(word);
    }
    w.bytes(&link.mac);
}

fn decode_link(r: &mut Decoder<'_>) -> Result<LinkResources, Error> {
    let control = r.word()?;
    let fifo = r.word()?;
    let rx_vmo = r.word()?;
    let tx_vmo = r.word()?;
    let rx_vaddr = r.word()?;
    let tx_vaddr = r.word()?;
    let rx_vmo_id = r.word()? as u32;
    let tx_vmo_id = r.word()? as u32;
    let mtu = r.word()? as u32;
    let mac = r.bytes(6)?;
    if mac.len() != 6 {
        return Err(Error::InvalidData);
    }
    Ok(LinkResources {
        control,
        fifo,
        rx_vmo,
        tx_vmo,
        rx_vaddr,
        tx_vaddr,
        rx_vmo_id,
        tx_vmo_id,
        mtu,
        mac: [mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]],
    })
}

fn encode_optional_ipv4(w: &mut Encoder, addr: Option<[u8; 4]>) {
    w.word(addr.is_some() as u64);
    if let Some(addr) = addr {
        w.bytes(&addr);
    }
}

fn decode_optional_ipv4(r: &mut Decoder<'_>) -> Result<Option<[u8; 4]>, Error> {
    if !r.flag()? {
        return Ok(None);
    }
    let bytes = r.bytes(4)?;
    if bytes.len() != 4 {
        return Err(Error::InvalidData);
    }
    Ok(Some([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn encode_optional_ipv6(w: &mut Encoder, addr: Option<[u8; 16]>) {
    w.word(addr.is_some() as u64);
    if let Some(addr) = addr {
        w.bytes(&addr);
    }
}

fn decode_optional_ipv6(r: &mut Decoder<'_>) -> Result<Option<[u8; 16]>, Error> {
    if !r.flag()? {
        return Ok(None);
    }
    let bytes = r.bytes(16)?;
    if bytes.len() != 16 {
        return Err(Error::InvalidData);
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(bytes);
    Ok(Some(out))
}

fn encode_i32(w: &mut Encoder, value: i32) {
    w.word(value as u32 as u64);
}

fn decode_i32(r: &mut Decoder<'_>) -> Result<i32, Error> {
    Ok((r.word()? as u32) as i32)
}

fn encode_i64(w: &mut Encoder, value: i64) {
    w.word(value as u64);
}

fn decode_i64(r: &mut Decoder<'_>) -> Result<i64, Error> {
    Ok(r.word()? as i64)
}

fn encode_optional_i32(w: &mut Encoder, value: Option<i32>) {
    w.word(value.is_some() as u64);
    if let Some(value) = value {
        encode_i32(w, value);
    }
}

fn decode_optional_i32(r: &mut Decoder<'_>) -> Result<Option<i32>, Error> {
    Ok(if r.flag()? {
        Some(decode_i32(r)?)
    } else {
        None
    })
}

fn encode_optional_i64(w: &mut Encoder, value: Option<i64>) {
    w.word(value.is_some() as u64);
    if let Some(value) = value {
        encode_i64(w, value);
    }
}

fn decode_optional_i64(r: &mut Decoder<'_>) -> Result<Option<i64>, Error> {
    Ok(if r.flag()? {
        Some(decode_i64(r)?)
    } else {
        None
    })
}

fn encode_optional_u64(w: &mut Encoder, value: Option<u64>) {
    w.word(value.is_some() as u64);
    if let Some(value) = value {
        w.word(value);
    }
}

fn decode_optional_u64(r: &mut Decoder<'_>) -> Result<Option<u64>, Error> {
    Ok(if r.flag()? { Some(r.word()?) } else { None })
}

fn encode_optional_u8(w: &mut Encoder, value: Option<u8>) {
    w.word(value.is_some() as u64);
    if let Some(value) = value {
        w.word(value as u64);
    }
}

fn decode_optional_u8(r: &mut Decoder<'_>) -> Result<Option<u8>, Error> {
    Ok(if r.flag()? {
        Some(r.word()? as u8)
    } else {
        None
    })
}

fn encode_socket_addr(w: &mut Encoder, addr: SocketAddress) {
    match addr.addr {
        IpAddress::Ipv4(ip) => {
            w.word(4);
            w.bytes(&ip.octets);
        }
        IpAddress::Ipv6(ip) => {
            w.word(6);
            w.bytes(&ip.octets);
        }
    }
    w.word(addr.port as u64);
}

fn decode_socket_addr(r: &mut Decoder<'_>) -> Result<SocketAddress, Error> {
    let family = r.word()?;
    let addr = match family {
        4 => {
            let bytes = r.bytes(4)?;
            if bytes.len() != 4 {
                return Err(Error::InvalidData);
            }
            IpAddress::Ipv4(Ipv4Address {
                octets: [bytes[0], bytes[1], bytes[2], bytes[3]],
            })
        }
        6 => {
            let bytes = r.bytes(16)?;
            if bytes.len() != 16 {
                return Err(Error::InvalidData);
            }
            IpAddress::Ipv6(net_fidl::Ipv6Address {
                octets: [
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                    bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
                    bytes[15],
                ],
            })
        }
        _ => return Err(Error::InvalidData),
    };
    Ok(SocketAddress {
        addr,
        port: r.count(u16::MAX as usize)? as u16,
    })
}

fn encode_optional_smoltcp_endpoint(w: &mut Encoder, endpoint: Option<SmoltcpIpEndpoint>) {
    w.word(endpoint.is_some() as u64);
    if let Some(endpoint) = endpoint {
        encode_smoltcp_endpoint(w, endpoint);
    }
}

fn decode_optional_smoltcp_endpoint(
    r: &mut Decoder<'_>,
) -> Result<Option<SmoltcpIpEndpoint>, Error> {
    Ok(if r.flag()? {
        Some(decode_smoltcp_endpoint(r)?)
    } else {
        None
    })
}

fn encode_smoltcp_endpoint(w: &mut Encoder, endpoint: SmoltcpIpEndpoint) {
    encode_smoltcp_ip(w, endpoint.addr);
    w.word(endpoint.port as u64);
}

fn decode_smoltcp_endpoint(r: &mut Decoder<'_>) -> Result<SmoltcpIpEndpoint, Error> {
    Ok(SmoltcpIpEndpoint {
        addr: decode_smoltcp_ip(r)?,
        port: r.word()? as u16,
    })
}

fn encode_smoltcp_listen_endpoint(w: &mut Encoder, endpoint: SmoltcpIpListenEndpoint) {
    w.word(endpoint.addr.is_some() as u64);
    if let Some(addr) = endpoint.addr {
        encode_smoltcp_ip(w, addr);
    }
    w.word(endpoint.port as u64);
}

fn decode_smoltcp_listen_endpoint(r: &mut Decoder<'_>) -> Result<SmoltcpIpListenEndpoint, Error> {
    let addr = if r.flag()? {
        Some(decode_smoltcp_ip(r)?)
    } else {
        None
    };
    Ok(SmoltcpIpListenEndpoint {
        addr,
        port: r.word()? as u16,
    })
}

fn encode_smoltcp_ip(w: &mut Encoder, addr: SmoltcpIpAddress) {
    match addr {
        SmoltcpIpAddress::Ipv4(ip) => {
            w.word(4);
            w.bytes(&ip.octets());
        }
        SmoltcpIpAddress::Ipv6(ip) => {
            w.word(6);
            w.bytes(&ip.octets());
        }
    }
}

fn decode_smoltcp_ip(r: &mut Decoder<'_>) -> Result<SmoltcpIpAddress, Error> {
    match r.word()? {
        4 => {
            let bytes = r.bytes(4)?;
            if bytes.len() != 4 {
                return Err(Error::InvalidData);
            }
            Ok(SmoltcpIpAddress::Ipv4(core::net::Ipv4Addr::new(
                bytes[0], bytes[1], bytes[2], bytes[3],
            )))
        }
        6 => {
            let bytes = r.bytes(16)?;
            if bytes.len() != 16 {
                return Err(Error::InvalidData);
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(bytes);
            Ok(SmoltcpIpAddress::Ipv6(core::net::Ipv6Addr::from(octets)))
        }
        _ => Err(Error::InvalidData),
    }
}
