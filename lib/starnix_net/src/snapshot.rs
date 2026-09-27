use crate::*;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const SNAPSHOT_VERSION: u64 = 4;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotError {
    Corrupt,
    UnsupportedVersion,
    TooLarge,
}

// The runner migration codec owns handles separately. This header binds the
// state record to an exact version and provides bounded corruption detection;
// namespace records are encoded by the runner alongside its VFS resources.
impl NetworkState {
    pub fn snapshot_header(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(80);
        bytes.extend_from_slice(b"BEXNET70");
        bytes.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(self.namespaces.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.sockets.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.namespace_fds.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.task_namespaces.len() as u64).to_le_bytes());
        let checksum = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        });
        bytes.extend_from_slice(&checksum.to_le_bytes());
        bytes
    }

    pub fn validate_snapshot_header(
        bytes: &[u8],
    ) -> Result<(usize, usize, usize, usize), SnapshotError> {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::TooLarge);
        }
        if bytes.len() != 56 || &bytes[..8] != b"BEXNET70" {
            return Err(SnapshotError::Corrupt);
        }
        let word =
            |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        if !matches!(word(8), 1..=SNAPSHOT_VERSION) {
            return Err(SnapshotError::UnsupportedVersion);
        }
        let checksum = bytes[..48]
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
            });
        if checksum != word(48) {
            return Err(SnapshotError::Corrupt);
        }
        let counts = (word(16), word(24), word(32), word(40));
        if counts.0 > 64 || counts.1 > 1024 || counts.2 > 256 || counts.3 > 4096 {
            return Err(SnapshotError::Corrupt);
        }
        Ok((
            counts.0 as usize,
            counts.1 as usize,
            counts.2 as usize,
            counts.3 as usize,
        ))
    }

    pub fn capability_snapshot(&self, tid: u32) -> Result<Capabilities, NetError> {
        self.task_capabilities
            .get(&tid)
            .copied()
            .ok_or(NetError::NotFound)
    }

    pub fn snapshot(&self) -> Result<Vec<u8>, SnapshotError> {
        let mut out = Writer::new();
        out.bytes(b"BEXNET71")?;
        out.word(SNAPSHOT_VERSION);
        for value in [
            self.next_namespace,
            self.next_interface,
            self.next_socket,
            self.next_namespace_fd,
        ] {
            out.word(value);
        }
        out.count(self.namespaces.len(), 64)?;
        for namespace in self.namespaces.values() {
            out.word(namespace.id);
            out.word(u64::from(namespace.owner_references));
            out.word(u64::from(namespace.fd_references));
            out.count(namespace.interfaces.len(), 64)?;
            for interface in namespace.interfaces.values() {
                encode_interface(&mut out, interface)?;
            }
            out.count(namespace.routes.len(), 256)?;
            for route in &namespace.routes {
                encode_prefix(&mut out, &route.destination)?;
                encode_optional_ip(&mut out, route.gateway);
                out.word(route.interface_id);
                out.word(u64::from(route.metric));
            }
            out.count(namespace.dns_servers.len(), 8)?;
            for server in &namespace.dns_servers {
                encode_ip(&mut out, *server);
            }
            encode_netfilter(&mut out, &namespace.netfilter)?;
        }
        out.count(self.sockets.len(), 1024)?;
        for socket in self.sockets.values() {
            out.word(socket.id);
            out.count(socket.linux_fds.len(), 256)?;
            for fd in &socket.linux_fds {
                out.word(*fd);
            }
            out.word(socket.namespace);
            out.word(socket_domain(socket.domain));
            out.word(socket_kind(socket.kind));
            out.word(u64::from(socket.protocol));
            out.flag(socket.net_admin);
            out.flag(socket.net_raw);
            out.word(socket.bound_interface.unwrap_or(0));
            encode_optional_endpoint(&mut out, socket.local);
            encode_optional_endpoint(&mut out, socket.peer);
            out.flag(socket.nonblocking);
            out.flag(socket.listening);
            out.flag(socket.shutdown_read);
            out.flag(socket.shutdown_write);
            out.flag(socket.receive_packet_info);
            out.count(socket.multicast_groups.len(), 64)?;
            for group in &socket.multicast_groups {
                encode_ip(&mut out, *group);
            }
            out.flag(socket.netfilter_batch.is_some());
            if let Some(batch) = &socket.netfilter_batch {
                encode_netfilter(&mut out, batch)?;
            }
            out.word(socket.backend_control);
            out.count(socket.queued_packets.len(), 256)?;
            for packet in &socket.queued_packets {
                out.bytes(packet)?;
            }
        }
        encode_map(&mut out, &self.namespace_fds, 256)?;
        out.count(self.task_namespaces.len(), 4096)?;
        for (tid, namespace) in &self.task_namespaces {
            out.word(u64::from(*tid));
            out.word(*namespace);
        }
        out.count(self.task_capabilities.len(), 4096)?;
        for (tid, capabilities) in &self.task_capabilities {
            out.word(u64::from(*tid));
            out.word(capabilities.permitted);
            out.word(capabilities.effective);
            out.word(capabilities.inheritable);
        }
        out.finish()
    }

    pub fn restore(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let mut input = Reader::new(bytes)?;
        if input.bytes(8)? != b"BEXNET71" {
            return Err(SnapshotError::UnsupportedVersion);
        }
        let version = input.word()?;
        if !matches!(version, 1..=SNAPSHOT_VERSION) {
            return Err(SnapshotError::UnsupportedVersion);
        }
        let next_namespace = input.word()?;
        let next_interface = input.word()?;
        let next_socket = input.word()?;
        let next_namespace_fd = input.word()?;
        let mut namespaces = BTreeMap::new();
        for _ in 0..input.count(64)? {
            let id = input.word()?;
            let owner_references = input.u32()?;
            let fd_references = input.u32()?;
            let mut interfaces = BTreeMap::new();
            for _ in 0..input.count(64)? {
                let interface = decode_interface(&mut input, version)?;
                if interfaces.insert(interface.id, interface).is_some() {
                    return Err(SnapshotError::Corrupt);
                }
            }
            let mut routes = Vec::new();
            for _ in 0..input.count(256)? {
                routes.push(Route {
                    destination: decode_prefix(&mut input)?,
                    gateway: decode_optional_ip(&mut input)?,
                    interface_id: input.word()?,
                    metric: input.u32()?,
                });
            }
            let mut dns_servers = Vec::new();
            if version >= 2 {
                for _ in 0..input.count(8)? {
                    dns_servers.push(decode_ip(&mut input)?);
                }
            }
            let netfilter = decode_netfilter(&mut input)?;
            if id == 0
                || interfaces.len() > MAX_INTERFACES
                || routes
                    .iter()
                    .any(|route| !interfaces.contains_key(&route.interface_id))
                || namespaces
                    .insert(
                        id,
                        Namespace {
                            id,
                            interfaces,
                            routes,
                            dns_servers,
                            netfilter,
                            owner_references,
                            fd_references,
                        },
                    )
                    .is_some()
            {
                return Err(SnapshotError::Corrupt);
            }
        }
        let mut sockets = BTreeMap::new();
        for _ in 0..input.count(1024)? {
            let id = input.word()?;
            let mut linux_fds = Vec::new();
            for _ in 0..input.count(256)? {
                let fd = input.word()?;
                // Versions 1 and 2 predate per-process descriptor identities;
                // their single-process fd numbers belong to PID 1.
                let fd = if version < 3 {
                    let raw = fd as u32 as i32;
                    if raw < 0 {
                        return Err(SnapshotError::Corrupt);
                    }
                    (1u64 << 32) | raw as u32 as u64
                } else {
                    fd
                };
                if fd as u32 > 255 || fd >> 32 == 0 || linux_fds.contains(&fd) {
                    return Err(SnapshotError::Corrupt);
                }
                linux_fds.push(fd);
            }
            let namespace = input.word()?;
            let domain = decode_socket_domain(input.word()?)?;
            let kind = decode_socket_kind(input.word()?)?;
            let protocol = input.u16()?;
            let net_admin = version >= 2 && input.flag()?;
            let net_raw = version >= 2 && input.flag()?;
            let bound = input.word()?;
            let local = decode_optional_endpoint(&mut input)?;
            let peer = decode_optional_endpoint(&mut input)?;
            let nonblocking = input.flag()?;
            let listening = input.flag()?;
            let shutdown_read = input.flag()?;
            let shutdown_write = input.flag()?;
            let receive_packet_info = version >= 4 && input.flag()?;
            let mut multicast_groups = Vec::new();
            if version >= 4 {
                for _ in 0..input.count(64)? {
                    let group = decode_ip(&mut input)?;
                    if !group.is_multicast() || multicast_groups.contains(&group) {
                        return Err(SnapshotError::Corrupt);
                    }
                    multicast_groups.push(group);
                }
            }
            let netfilter_batch = if version >= 4 && input.flag()? {
                Some(decode_netfilter(&mut input)?)
            } else {
                None
            };
            let backend_control = input.word()?;
            let mut queued_packets = Vec::new();
            for _ in 0..input.count(256)? {
                queued_packets.push(input.bytes(65_535)?.to_vec());
            }
            if id == 0 || !namespaces.contains_key(&namespace) {
                return Err(SnapshotError::Corrupt);
            }
            if bound != 0
                && !namespaces
                    .get(&namespace)
                    .unwrap()
                    .interfaces
                    .contains_key(&bound)
            {
                return Err(SnapshotError::Corrupt);
            }
            let socket = SocketState {
                id,
                linux_fds,
                namespace,
                domain,
                kind,
                protocol,
                net_admin,
                net_raw,
                bound_interface: (bound != 0).then_some(bound),
                local,
                peer,
                nonblocking,
                listening,
                shutdown_read,
                shutdown_write,
                receive_packet_info,
                multicast_groups,
                netfilter_batch,
                backend_control,
                queued_packets,
            };
            if sockets.values().any(|other: &SocketState| {
                socket
                    .linux_fds
                    .iter()
                    .any(|fd| other.linux_fds.contains(fd))
            }) || sockets.insert(id, socket).is_some()
            {
                return Err(SnapshotError::Corrupt);
            }
        }
        let namespace_fds = decode_map(&mut input, 256)?;
        let mut task_namespaces = BTreeMap::new();
        for _ in 0..input.count(4096)? {
            let tid = input.u32()?;
            let namespace = input.word()?;
            if tid == 0
                || !namespaces.contains_key(&namespace)
                || task_namespaces.insert(tid, namespace).is_some()
            {
                return Err(SnapshotError::Corrupt);
            }
        }
        let mut task_capabilities = BTreeMap::new();
        for _ in 0..input.count(4096)? {
            let tid = input.u32()?;
            let capabilities = Capabilities {
                permitted: input.word()?,
                effective: input.word()?,
                inheritable: input.word()?,
            };
            let supported = CAP_NET_ADMIN | CAP_NET_RAW | CAP_SYS_ADMIN;
            if tid == 0
                || capabilities.permitted & !supported != 0
                || capabilities.effective & !capabilities.permitted != 0
                || capabilities.inheritable & !supported != 0
                || task_capabilities.insert(tid, capabilities).is_some()
            {
                return Err(SnapshotError::Corrupt);
            }
        }
        input.finish()?;
        if !namespaces.contains_key(&1)
            || namespace_fds
                .values()
                .any(|namespace| !namespaces.contains_key(namespace))
            || task_namespaces
                .keys()
                .any(|tid| !task_capabilities.contains_key(tid))
            || next_namespace <= namespaces.keys().copied().max().unwrap_or(0)
            || next_interface
                <= namespaces
                    .values()
                    .flat_map(|namespace| namespace.interfaces.keys().copied())
                    .max()
                    .unwrap_or(0)
            || next_socket <= sockets.keys().copied().max().unwrap_or(0)
        {
            return Err(SnapshotError::Corrupt);
        }
        Ok(Self {
            namespaces,
            sockets,
            namespace_fds,
            task_namespaces,
            task_capabilities,
            next_namespace,
            next_interface,
            next_socket,
            next_namespace_fd,
        })
    }

    pub fn migration_handles(&self) -> Vec<u64> {
        let mut handles = Vec::new();
        for interface in self
            .namespaces
            .values()
            .flat_map(|namespace| namespace.interfaces.values())
        {
            match interface.backend {
                InterfaceBackend::Direct {
                    provider, lease, ..
                } => {
                    handles.push(provider);
                    handles.push(lease);
                }
                InterfaceBackend::L2 { device, lease, .. } => {
                    handles.push(device);
                    handles.push(lease);
                }
                _ => {}
            }
        }
        handles.extend(
            self.sockets
                .values()
                .map(|socket| socket.backend_control)
                .filter(|handle| *handle != 0),
        );
        handles.sort_unstable();
        handles.dedup();
        handles
    }
}

fn encode_interface(out: &mut Writer, value: &Interface) -> Result<(), SnapshotError> {
    out.word(value.id);
    out.text(&value.name, 15)?;
    out.word(u64::from(value.mtu));
    out.flag(value.up);
    out.flag(value.promiscuous);
    out.count(value.addresses.len(), 16)?;
    for address in &value.addresses {
        encode_prefix(out, address)?;
    }
    out.word(value.bridge.unwrap_or(0));
    match &value.backend {
        InterfaceBackend::Loopback => out.word(1),
        InterfaceBackend::Direct {
            provider,
            lease,
            table_id,
            host_interface_id,
            policy,
        } => {
            out.word(2);
            out.word(*provider);
            out.word(*lease);
            out.word(u64::from(*table_id));
            out.word(*host_interface_id);
            encode_policy(out, policy)?;
        }
        InterfaceBackend::L2 {
            device,
            lease,
            mac,
            policy,
        } => {
            out.word(3);
            out.word(*device);
            out.word(*lease);
            out.bytes(mac)?;
            encode_policy(out, policy)?;
        }
        InterfaceBackend::Veth {
            peer_namespace,
            peer_interface,
        } => {
            out.word(4);
            out.word(*peer_namespace);
            out.word(*peer_interface);
        }
        InterfaceBackend::Bridge => out.word(5),
    }
    Ok(())
}

fn decode_interface(input: &mut Reader<'_>, version: u64) -> Result<Interface, SnapshotError> {
    let id = input.word()?;
    let name = input.text(15)?;
    let mtu = input.u32()?;
    let up = input.flag()?;
    let promiscuous = input.flag()?;
    let mut addresses = Vec::new();
    for _ in 0..input.count(16)? {
        addresses.push(decode_prefix(input)?);
    }
    let bridge = input.word()?;
    let backend = match input.word()? {
        1 => InterfaceBackend::Loopback,
        2 => InterfaceBackend::Direct {
            provider: input.word()?,
            lease: input.word()?,
            table_id: input.u32()?,
            host_interface_id: if version >= 3 { input.word()? } else { 0 },
            policy: decode_policy(input)?,
        },
        3 => InterfaceBackend::L2 {
            device: input.word()?,
            lease: input.word()?,
            mac: input
                .bytes(6)?
                .try_into()
                .map_err(|_| SnapshotError::Corrupt)?,
            policy: decode_policy(input)?,
        },
        4 => InterfaceBackend::Veth {
            peer_namespace: input.word()?,
            peer_interface: input.word()?,
        },
        5 => InterfaceBackend::Bridge,
        _ => return Err(SnapshotError::Corrupt),
    };
    let value = Interface {
        id,
        name,
        mtu,
        up,
        promiscuous,
        addresses,
        bridge: (bridge != 0).then_some(bridge),
        backend,
    };
    value.validate().map_err(|_| SnapshotError::Corrupt)?;
    Ok(value)
}

fn encode_policy(out: &mut Writer, value: &HostPolicy) -> Result<(), SnapshotError> {
    out.text(&value.profile, 64)?;
    out.text(&value.isolation_group, 64)?;
    out.text(&value.domain, 64)?;
    out.text(&value.physical_selector, 128)?;
    out.word(u64::from(value.vlan_id));
    out.flag(value.allow_raw);
    out.flag(value.allow_promiscuous);
    Ok(())
}

fn decode_policy(input: &mut Reader<'_>) -> Result<HostPolicy, SnapshotError> {
    Ok(HostPolicy {
        profile: input.text(64)?,
        isolation_group: input.text(64)?,
        domain: input.text(64)?,
        physical_selector: input.text(128)?,
        vlan_id: input.u16()?,
        allow_raw: input.flag()?,
        allow_promiscuous: input.flag()?,
    })
}

fn encode_prefix(out: &mut Writer, value: &IpPrefix) -> Result<(), SnapshotError> {
    encode_ip(out, value.address);
    out.word(u64::from(value.prefix_len));
    Ok(())
}

fn decode_prefix(input: &mut Reader<'_>) -> Result<IpPrefix, SnapshotError> {
    IpPrefix::new(decode_ip(input)?, input.u8()?).map_err(|_| SnapshotError::Corrupt)
}

fn encode_ip(out: &mut Writer, value: IpAddr) {
    match value {
        IpAddr::V4(value) => {
            out.word(4);
            out.raw(&value.octets());
        }
        IpAddr::V6(value) => {
            out.word(6);
            out.raw(&value.octets());
        }
    }
}

fn decode_ip(input: &mut Reader<'_>) -> Result<IpAddr, SnapshotError> {
    match input.word()? {
        4 => Ok(IpAddr::V4(Ipv4Addr::from(
            <[u8; 4]>::try_from(input.raw(4)?).unwrap(),
        ))),
        6 => Ok(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(input.raw(16)?).unwrap(),
        ))),
        _ => Err(SnapshotError::Corrupt),
    }
}

fn encode_optional_ip(out: &mut Writer, value: Option<IpAddr>) {
    out.flag(value.is_some());
    if let Some(value) = value {
        encode_ip(out, value);
    }
}

fn decode_optional_ip(input: &mut Reader<'_>) -> Result<Option<IpAddr>, SnapshotError> {
    input.flag()?.then(|| decode_ip(input)).transpose()
}

fn encode_optional_endpoint(out: &mut Writer, value: Option<(IpAddr, u16)>) {
    out.flag(value.is_some());
    if let Some((address, port)) = value {
        encode_ip(out, address);
        out.word(u64::from(port));
    }
}

fn decode_optional_endpoint(
    input: &mut Reader<'_>,
) -> Result<Option<(IpAddr, u16)>, SnapshotError> {
    input
        .flag()?
        .then(|| Ok((decode_ip(input)?, input.u16()?)))
        .transpose()
}

fn socket_domain(value: SocketDomain) -> u64 {
    match value {
        SocketDomain::Inet4 => 1,
        SocketDomain::Inet6 => 2,
        SocketDomain::Packet => 3,
        SocketDomain::RouteNetlink => 4,
        SocketDomain::NetfilterNetlink => 5,
        SocketDomain::SockDiagNetlink => 6,
    }
}

fn decode_socket_domain(value: u64) -> Result<SocketDomain, SnapshotError> {
    match value {
        1 => Ok(SocketDomain::Inet4),
        2 => Ok(SocketDomain::Inet6),
        3 => Ok(SocketDomain::Packet),
        4 => Ok(SocketDomain::RouteNetlink),
        5 => Ok(SocketDomain::NetfilterNetlink),
        6 => Ok(SocketDomain::SockDiagNetlink),
        _ => Err(SnapshotError::Corrupt),
    }
}

fn socket_kind(value: SocketKind) -> u64 {
    match value {
        SocketKind::Stream => 1,
        SocketKind::Datagram => 2,
        SocketKind::Raw => 3,
    }
}

fn decode_socket_kind(value: u64) -> Result<SocketKind, SnapshotError> {
    match value {
        1 => Ok(SocketKind::Stream),
        2 => Ok(SocketKind::Datagram),
        3 => Ok(SocketKind::Raw),
        _ => Err(SnapshotError::Corrupt),
    }
}

fn encode_netfilter(out: &mut Writer, value: &Netfilter) -> Result<(), SnapshotError> {
    out.word(value.generation);
    out.word(verdict(value.default_verdict));
    out.count(value.rules.len(), MAX_FIREWALL_RULES)?;
    for rule in &value.rules {
        encode_optional_word(out, rule.matcher.input_interface);
        encode_optional_word(out, rule.matcher.output_interface);
        encode_optional_prefix(out, rule.matcher.source.as_ref())?;
        encode_optional_prefix(out, rule.matcher.destination.as_ref())?;
        encode_optional_word(out, rule.matcher.protocol.map(u64::from));
        encode_optional_ports(out, rule.matcher.source_ports);
        encode_optional_ports(out, rule.matcher.destination_ports);
        out.count(rule.matcher.states.len(), 4)?;
        for state in &rule.matcher.states {
            out.word(connection_state(*state));
        }
        match rule.action {
            RuleAction::Verdict(value) => {
                out.word(1);
                out.word(verdict(value));
            }
            RuleAction::Snat { address, port } => {
                out.word(2);
                encode_ip(out, address);
                encode_optional_word(out, port.map(u64::from));
            }
            RuleAction::Dnat { address, port } => {
                out.word(3);
                encode_ip(out, address);
                encode_optional_word(out, port.map(u64::from));
            }
            RuleAction::Masquerade => out.word(4),
        }
        out.word(rule.packets);
        out.word(rule.bytes);
    }
    out.count(value.conntrack.len(), MAX_CONNTRACK)?;
    for (flow, translation) in &value.conntrack {
        encode_ip(out, flow.source);
        encode_ip(out, flow.destination);
        out.word(u64::from(flow.protocol));
        out.word(u64::from(flow.source_port));
        out.word(u64::from(flow.destination_port));
        encode_ip(out, translation.source);
        encode_ip(out, translation.destination);
        out.word(u64::from(translation.source_port));
        out.word(u64::from(translation.destination_port));
    }
    Ok(())
}

fn decode_netfilter(input: &mut Reader<'_>) -> Result<Netfilter, SnapshotError> {
    let generation = input.word()?;
    let default_verdict = decode_verdict(input.word()?)?;
    let mut rules = Vec::new();
    for _ in 0..input.count(MAX_FIREWALL_RULES)? {
        let input_interface = decode_optional_word(input)?;
        let output_interface = decode_optional_word(input)?;
        let source = decode_optional_prefix(input)?;
        let destination = decode_optional_prefix(input)?;
        let protocol = decode_optional_word(input)?
            .map(|value| u8::try_from(value).map_err(|_| SnapshotError::Corrupt))
            .transpose()?;
        let source_ports = decode_optional_ports(input)?;
        let destination_ports = decode_optional_ports(input)?;
        let mut states = Vec::new();
        for _ in 0..input.count(4)? {
            states.push(decode_connection_state(input.word()?)?);
        }
        let action = match input.word()? {
            1 => RuleAction::Verdict(decode_verdict(input.word()?)?),
            2 => RuleAction::Snat {
                address: decode_ip(input)?,
                port: decode_optional_word(input)?
                    .map(|value| u16::try_from(value).map_err(|_| SnapshotError::Corrupt))
                    .transpose()?,
            },
            3 => RuleAction::Dnat {
                address: decode_ip(input)?,
                port: decode_optional_word(input)?
                    .map(|value| u16::try_from(value).map_err(|_| SnapshotError::Corrupt))
                    .transpose()?,
            },
            4 => RuleAction::Masquerade,
            _ => return Err(SnapshotError::Corrupt),
        };
        rules.push(Rule {
            matcher: RuleMatch {
                input_interface,
                output_interface,
                source,
                destination,
                protocol,
                source_ports,
                destination_ports,
                states,
            },
            action,
            packets: input.word()?,
            bytes: input.word()?,
        });
    }
    let mut conntrack = BTreeMap::new();
    for _ in 0..input.count(MAX_CONNTRACK)? {
        let flow = Flow {
            source: decode_ip(input)?,
            destination: decode_ip(input)?,
            protocol: input.u8()?,
            source_port: input.u16()?,
            destination_port: input.u16()?,
        };
        let translation = Translation {
            source: decode_ip(input)?,
            destination: decode_ip(input)?,
            source_port: input.u16()?,
            destination_port: input.u16()?,
        };
        if conntrack.insert(flow, translation).is_some() {
            return Err(SnapshotError::Corrupt);
        }
    }
    Ok(Netfilter {
        generation,
        rules,
        conntrack,
        default_verdict,
    })
}

fn verdict(value: Verdict) -> u64 {
    match value {
        Verdict::Accept => 1,
        Verdict::Drop => 2,
        Verdict::Reject => 3,
    }
}

fn decode_verdict(value: u64) -> Result<Verdict, SnapshotError> {
    match value {
        1 => Ok(Verdict::Accept),
        2 => Ok(Verdict::Drop),
        3 => Ok(Verdict::Reject),
        _ => Err(SnapshotError::Corrupt),
    }
}

fn connection_state(value: ConnectionState) -> u64 {
    match value {
        ConnectionState::New => 1,
        ConnectionState::Established => 2,
        ConnectionState::Related => 3,
        ConnectionState::Invalid => 4,
    }
}

fn decode_connection_state(value: u64) -> Result<ConnectionState, SnapshotError> {
    match value {
        1 => Ok(ConnectionState::New),
        2 => Ok(ConnectionState::Established),
        3 => Ok(ConnectionState::Related),
        4 => Ok(ConnectionState::Invalid),
        _ => Err(SnapshotError::Corrupt),
    }
}

fn encode_optional_prefix(out: &mut Writer, value: Option<&IpPrefix>) -> Result<(), SnapshotError> {
    out.flag(value.is_some());
    if let Some(value) = value {
        encode_prefix(out, value)?;
    }
    Ok(())
}

fn decode_optional_prefix(input: &mut Reader<'_>) -> Result<Option<IpPrefix>, SnapshotError> {
    input.flag()?.then(|| decode_prefix(input)).transpose()
}

fn encode_optional_ports(out: &mut Writer, value: Option<(u16, u16)>) {
    out.flag(value.is_some());
    if let Some((start, end)) = value {
        out.word(u64::from(start));
        out.word(u64::from(end));
    }
}

fn decode_optional_ports(input: &mut Reader<'_>) -> Result<Option<(u16, u16)>, SnapshotError> {
    let value = input
        .flag()?
        .then(|| Ok((input.u16()?, input.u16()?)))
        .transpose()?;
    if value.is_some_and(|(start, end)| start > end) {
        return Err(SnapshotError::Corrupt);
    }
    Ok(value)
}

fn encode_optional_word(out: &mut Writer, value: Option<u64>) {
    out.flag(value.is_some());
    if let Some(value) = value {
        out.word(value);
    }
}

fn decode_optional_word(input: &mut Reader<'_>) -> Result<Option<u64>, SnapshotError> {
    input.flag()?.then(|| input.word()).transpose()
}

fn encode_map(
    out: &mut Writer,
    value: &BTreeMap<u64, u64>,
    maximum: usize,
) -> Result<(), SnapshotError> {
    out.count(value.len(), maximum)?;
    for (key, value) in value {
        out.word(*key);
        out.word(*value);
    }
    Ok(())
}

fn decode_map(input: &mut Reader<'_>, maximum: usize) -> Result<BTreeMap<u64, u64>, SnapshotError> {
    let mut value = BTreeMap::new();
    for _ in 0..input.count(maximum)? {
        if value.insert(input.word()?, input.word()?).is_some() {
            return Err(SnapshotError::Corrupt);
        }
    }
    Ok(value)
}

struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }
    fn word(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn flag(&mut self, value: bool) {
        self.word(u64::from(value));
    }
    fn count(&mut self, value: usize, maximum: usize) -> Result<(), SnapshotError> {
        if value > maximum {
            return Err(SnapshotError::TooLarge);
        }
        self.word(value as u64);
        Ok(())
    }
    fn raw(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }
    fn bytes(&mut self, value: &[u8]) -> Result<(), SnapshotError> {
        if value.len() > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::TooLarge);
        }
        self.word(value.len() as u64);
        self.raw(value);
        Ok(())
    }
    fn text(&mut self, value: &str, maximum: usize) -> Result<(), SnapshotError> {
        if value.len() > maximum || value.contains('\0') {
            return Err(SnapshotError::Corrupt);
        }
        self.bytes(value.as_bytes())
    }
    fn finish(self) -> Result<Vec<u8>, SnapshotError> {
        if self.bytes.len() > MAX_SNAPSHOT_BYTES {
            Err(SnapshotError::TooLarge)
        } else {
            Ok(self.bytes)
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, SnapshotError> {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::TooLarge);
        }
        Ok(Self { bytes, at: 0 })
    }
    fn raw(&mut self, length: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self.at.checked_add(length).ok_or(SnapshotError::Corrupt)?;
        let value = self.bytes.get(self.at..end).ok_or(SnapshotError::Corrupt)?;
        self.at = end;
        Ok(value)
    }
    fn word(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        self.word()?.try_into().map_err(|_| SnapshotError::Corrupt)
    }
    fn u16(&mut self) -> Result<u16, SnapshotError> {
        self.word()?.try_into().map_err(|_| SnapshotError::Corrupt)
    }
    fn u8(&mut self) -> Result<u8, SnapshotError> {
        self.word()?.try_into().map_err(|_| SnapshotError::Corrupt)
    }
    fn flag(&mut self) -> Result<bool, SnapshotError> {
        match self.word()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(SnapshotError::Corrupt),
        }
    }
    fn count(&mut self, maximum: usize) -> Result<usize, SnapshotError> {
        let value: usize = self
            .word()?
            .try_into()
            .map_err(|_| SnapshotError::Corrupt)?;
        (value <= maximum)
            .then_some(value)
            .ok_or(SnapshotError::Corrupt)
    }
    fn bytes(&mut self, maximum: usize) -> Result<&'a [u8], SnapshotError> {
        let length = self.count(maximum)?;
        self.raw(length)
    }
    fn text(&mut self, maximum: usize) -> Result<String, SnapshotError> {
        let value = self.bytes(maximum)?;
        let value = core::str::from_utf8(value).map_err(|_| SnapshotError::Corrupt)?;
        if value.contains('\0') {
            return Err(SnapshotError::Corrupt);
        }
        Ok(value.into())
    }
    fn finish(self) -> Result<(), SnapshotError> {
        (self.at == self.bytes.len())
            .then_some(())
            .ok_or(SnapshotError::Corrupt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn multi_vnic_fib_uses_longest_prefix_then_metric_and_binding() {
        let mut state = NetworkState::offline(0);
        let policy = HostPolicy {
            profile: "l2".into(),
            isolation_group: "default".into(),
            domain: String::new(),
            physical_selector: "pci/ethernet".into(),
            vlan_id: 42,
            allow_raw: true,
            allow_promiscuous: false,
        };
        let interface = |name: &str, device| Interface {
            id: 1,
            name: name.into(),
            mtu: 1500,
            up: true,
            promiscuous: false,
            addresses: Vec::new(),
            bridge: None,
            backend: InterfaceBackend::L2 {
                device,
                lease: device + 100,
                mac: [2, 0, 0, 0, 0, device as u8],
                policy: policy.clone(),
            },
        };
        let one = state.attach(1, interface("eth0", 10)).unwrap();
        let two = state.attach(1, interface("eth1", 11)).unwrap();
        state
            .add_route(
                1,
                1,
                Route {
                    destination: IpPrefix::new("0.0.0.0".parse().unwrap(), 0).unwrap(),
                    gateway: None,
                    interface_id: one,
                    metric: 10,
                },
            )
            .unwrap();
        state
            .add_route(
                1,
                1,
                Route {
                    destination: IpPrefix::new("10.2.0.0".parse().unwrap(), 16).unwrap(),
                    gateway: None,
                    interface_id: two,
                    metric: 20,
                },
            )
            .unwrap();
        let socket = state
            .socket(1, SocketDomain::Inet4, SocketKind::Stream, 6, false)
            .unwrap();
        assert_eq!(
            state.selected_interface(socket, "10.2.3.4".parse::<IpAddr>().unwrap()),
            Ok(two)
        );
        state.bind_interface(socket, "eth0").unwrap();
        assert_eq!(
            state.selected_interface(socket, "10.2.3.4".parse::<IpAddr>().unwrap()),
            Ok(one)
        );
    }

    #[test]
    fn namespaces_are_retained_by_sockets_and_namespace_fds() {
        let mut state = NetworkState::offline(0);
        let namespace = state.create_namespace(1).unwrap();
        let fd = state.open_namespace(namespace).unwrap();
        let socket = state
            .socket(1, SocketDomain::Inet6, SocketKind::Datagram, 17, false)
            .unwrap();
        state.exit_task(1);
        state.close_namespace_fd(fd).unwrap();
        assert!(state.namespaces.contains_key(&namespace));
        state.sockets.remove(&socket);
        state
            .namespaces
            .get_mut(&namespace)
            .unwrap()
            .owner_references = 1;
        state.task_namespaces.insert(2, namespace);
        state.exit_task(2);
        assert!(!state.namespaces.contains_key(&namespace));
    }

    #[test]
    fn forked_descriptor_tables_retain_shared_socket_until_last_process_closes() {
        let mut state = NetworkState::offline(0);
        let socket = state
            .socket(1, SocketDomain::Inet4, SocketKind::Datagram, 17, false)
            .unwrap();
        state.sockets.get_mut(&socket).unwrap().backend_control = 99;
        let parent_fd = (1u64 << 32) | 3;
        let parent_ns = (1u64 << 32) | 4;
        state.bind_fd(socket, parent_fd).unwrap();
        state.bind_namespace_fd(parent_ns, 1).unwrap();
        state.clone_process_fds(1, 2).unwrap();
        assert_eq!(state.socket_by_fd((2u64 << 32) | 3), Some(socket));
        assert_eq!(state.close_process_fds(2), Vec::<u64>::new());
        assert_eq!(state.socket_by_fd(parent_fd), Some(socket));
        assert_eq!(state.close_process_fds(1), vec![99]);
        assert!(!state.sockets.contains_key(&socket));
    }

    #[test]
    fn direct_provider_rejects_raw_and_netfilter_replacement_is_atomic() {
        let mut state = NetworkState::offline(0);
        let interface = state
            .attach(
                1,
                Interface {
                    id: 1,
                    name: "eth0".into(),
                    mtu: 1500,
                    up: true,
                    promiscuous: false,
                    addresses: Vec::new(),
                    bridge: None,
                    backend: InterfaceBackend::Direct {
                        provider: 9,
                        lease: 10,
                        table_id: 4,
                        host_interface_id: 7,
                        policy: HostPolicy {
                            profile: "direct".into(),
                            isolation_group: "default".into(),
                            domain: "public".into(),
                            physical_selector: String::new(),
                            vlan_id: 0,
                            allow_raw: false,
                            allow_promiscuous: false,
                        },
                    },
                },
            )
            .unwrap();
        assert_eq!(
            state.socket(1, SocketDomain::Inet4, SocketKind::Raw, 1, false),
            Err(NetError::PermissionDenied)
        );
        assert_eq!(interface, 2);

        let namespace = state.namespaces.get_mut(&1).unwrap();
        let generation = namespace
            .netfilter
            .replace_atomic(0, Vec::new(), Verdict::Drop)
            .unwrap();
        assert_eq!(generation, 1);
        assert_eq!(
            namespace
                .netfilter
                .replace_atomic(0, Vec::new(), Verdict::Accept),
            Err(NetError::InvalidArgument)
        );
        assert_eq!(namespace.netfilter.default_verdict, Verdict::Drop);

        namespace.routes.push(Route {
            destination: IpPrefix::new("0.0.0.0".parse().unwrap(), 0).unwrap(),
            gateway: None,
            interface_id: interface,
            metric: 0,
        });
        namespace
            .netfilter
            .replace_atomic(
                1,
                vec![Rule {
                    matcher: RuleMatch {
                        input_interface: None,
                        output_interface: Some(interface),
                        source: None,
                        destination: None,
                        protocol: Some(17),
                        source_ports: None,
                        destination_ports: None,
                        states: Vec::new(),
                    },
                    action: RuleAction::Dnat {
                        address: "192.0.2.1".parse().unwrap(),
                        port: Some(5353),
                    },
                    packets: 0,
                    bytes: 0,
                }],
                Verdict::Accept,
            )
            .unwrap();
        let socket = state
            .socket(1, SocketDomain::Inet4, SocketKind::Datagram, 17, false)
            .unwrap();
        assert_eq!(
            state.filter_egress(socket, ("198.51.100.1".parse().unwrap(), 53), 17, 40000),
            Err(NetError::Unsupported)
        );
    }

    #[test]
    fn snapshot_header_rejects_corruption() {
        let state = NetworkState::offline(0);
        let mut bytes = state.snapshot_header();
        assert_eq!(
            NetworkState::validate_snapshot_header(&bytes),
            Ok((1, 0, 0, 1))
        );
        bytes[20] ^= 1;
        assert_eq!(
            NetworkState::validate_snapshot_header(&bytes),
            Err(SnapshotError::Corrupt)
        );
    }

    #[test]
    fn full_snapshot_round_trips_namespace_and_capability_state() {
        let mut state = NetworkState::offline(0);
        let namespace = state.create_namespace(1).unwrap();
        let fd = state.open_namespace(namespace).unwrap();
        assert_ne!(fd, 0);
        let encoded = state.snapshot().unwrap();
        assert_eq!(NetworkState::restore(&encoded), Ok(state));
        let mut corrupt = encoded;
        corrupt.truncate(corrupt.len() - 1);
        assert_eq!(NetworkState::restore(&corrupt), Err(SnapshotError::Corrupt));
    }

    #[test]
    fn new_namespace_has_only_routable_loopback_and_can_build_veth_bridge() {
        let mut state = NetworkState::offline(0);
        let namespace = state.create_namespace(1).unwrap();
        let target = state.namespaces.get(&namespace).unwrap();
        assert_eq!(target.interfaces.len(), 1);
        assert_eq!(
            target
                .route("127.3.2.1".parse().unwrap(), None)
                .unwrap()
                .interface_id,
            1
        );
        assert_eq!(
            target
                .route("::1".parse().unwrap(), None)
                .unwrap()
                .interface_id,
            1
        );
        let (left, right) = state
            .create_veth(1, namespace, "veth0", namespace, "veth1")
            .unwrap();
        let bridge = state.create_bridge(1, namespace, "br0").unwrap();
        state.enslave(1, namespace, left, bridge).unwrap();
        state.enslave(1, namespace, right, bridge).unwrap();
        let restored = NetworkState::restore(&state.snapshot().unwrap()).unwrap();
        assert_eq!(restored, state);
    }

    #[test]
    fn firewall_translation_counts_and_rejects_stale_atomic_update() {
        let mut firewall = Netfilter::default();
        let generation = firewall
            .replace_atomic(
                0,
                vec![Rule {
                    matcher: RuleMatch {
                        input_interface: None,
                        output_interface: Some(2),
                        source: None,
                        destination: Some(IpPrefix::new("10.0.0.0".parse().unwrap(), 8).unwrap()),
                        protocol: Some(17),
                        source_ports: None,
                        destination_ports: Some((53, 53)),
                        states: vec![ConnectionState::New],
                    },
                    action: RuleAction::Dnat {
                        address: "192.0.2.53".parse().unwrap(),
                        port: None,
                    },
                    packets: 0,
                    bytes: 0,
                }],
                Verdict::Accept,
            )
            .unwrap();
        let mut packet = Packet {
            input_interface: None,
            output_interface: Some(2),
            source: "192.0.2.1".parse().unwrap(),
            destination: "10.0.0.53".parse().unwrap(),
            protocol: 17,
            source_port: 40000,
            destination_port: 53,
            state: ConnectionState::New,
        };
        assert_eq!(firewall.evaluate(&mut packet, 64), Verdict::Accept);
        assert_eq!(packet.destination, "192.0.2.53".parse::<IpAddr>().unwrap());
        assert_eq!(firewall.conntrack.len(), 2);
        let mut reply = Packet {
            input_interface: Some(2),
            output_interface: None,
            source: "192.0.2.53".parse().unwrap(),
            destination: "192.0.2.1".parse().unwrap(),
            protocol: 17,
            source_port: 53,
            destination_port: 40000,
            state: ConnectionState::New,
        };
        assert_eq!(firewall.evaluate(&mut reply, 64), Verdict::Accept);
        assert_eq!(reply.source, "10.0.0.53".parse::<IpAddr>().unwrap());
        assert_eq!(reply.state, ConnectionState::Established);
        assert_eq!(
            (firewall.rules[0].packets, firewall.rules[0].bytes),
            (1, 64)
        );
        assert_eq!(
            firewall.replace_atomic(0, Vec::new(), Verdict::Drop),
            Err(NetError::InvalidArgument)
        );
        assert_eq!(firewall.generation, generation);
    }

    #[test]
    fn nft_batch_and_legacy_iptables_replace_are_atomic() {
        fn netlink(message_type: u16, body: &[u8]) -> Vec<u8> {
            let mut bytes = vec![0; 16];
            bytes[..4].copy_from_slice(&((16 + body.len()) as u32).to_ne_bytes());
            bytes[4..6].copy_from_slice(&message_type.to_ne_bytes());
            bytes.extend_from_slice(body);
            bytes
        }
        fn attr(kind: u16, value: &[u8]) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&((4 + value.len()) as u16).to_ne_bytes());
            bytes.extend_from_slice(&kind.to_ne_bytes());
            bytes.extend_from_slice(value);
            while bytes.len() & 3 != 0 {
                bytes.push(0);
            }
            bytes
        }

        let mut state = NetworkState::offline(0);
        let control = state
            .socket(
                1,
                SocketDomain::NetfilterNetlink,
                SocketKind::Raw,
                12,
                false,
            )
            .unwrap();
        state.netlink_send(control, &netlink(0x10, &[])).unwrap();
        let mut chain = vec![2, 0, 0, 0];
        chain.extend_from_slice(&attr(5, &0u32.to_be_bytes()));
        state
            .netlink_send(control, &netlink(0x0a03, &chain))
            .unwrap();
        state.netlink_send(control, &netlink(0x11, &[])).unwrap();
        assert_eq!(
            state.namespaces[&1].netfilter.default_verdict,
            Verdict::Drop
        );
        assert_eq!(state.namespaces[&1].netfilter.generation, 1);

        state.netlink_send(control, &netlink(0x10, &[])).unwrap();
        assert_eq!(
            state.netlink_send(control, &netlink(0x0aff, &[2, 0, 0, 0])),
            Err(NetError::Unsupported)
        );
        assert!(state.sockets[&control].netfilter_batch.is_none());
        assert_eq!(state.namespaces[&1].netfilter.generation, 1);

        let mut replacement = vec![0u8; 96 + 152];
        replacement[..6].copy_from_slice(b"filter");
        replacement[36..40].copy_from_slice(&1u32.to_ne_bytes());
        replacement[40..44].copy_from_slice(&152u32.to_ne_bytes());
        let entry = 96;
        replacement[entry + 88..entry + 90].copy_from_slice(&112u16.to_ne_bytes());
        replacement[entry + 90..entry + 92].copy_from_slice(&152u16.to_ne_bytes());
        replacement[entry + 112..entry + 114].copy_from_slice(&40u16.to_ne_bytes());
        replacement[entry + 144..entry + 148].copy_from_slice(&(-1i32).to_ne_bytes());
        state.replace_iptables_ipv4(control, &replacement).unwrap();
        assert_eq!(state.namespaces[&1].netfilter.rules.len(), 1);
        assert_eq!(
            state.namespaces[&1].netfilter.rules[0].action,
            RuleAction::Verdict(Verdict::Drop)
        );
        assert_eq!(state.namespaces[&1].netfilter.generation, 2);

        replacement[entry + 114] = b'X';
        assert_eq!(
            state.replace_iptables_ipv4(control, &replacement),
            Err(NetError::Unsupported)
        );
        assert_eq!(state.namespaces[&1].netfilter.generation, 2);
    }

    #[test]
    fn multicast_and_packet_info_socket_state_survives_snapshot() {
        let mut state = NetworkState::offline(0);
        let socket = state
            .socket(1, SocketDomain::Inet4, SocketKind::Datagram, 17, false)
            .unwrap();
        state.set_receive_packet_info(socket, true).unwrap();
        state
            .update_multicast_membership(socket, "239.1.2.3".parse().unwrap(), Some(1), true)
            .unwrap();
        assert_eq!(NetworkState::restore(&state.snapshot().unwrap()), Ok(state));
    }

    #[test]
    fn sock_diag_dump_is_namespace_scoped_and_reports_endpoints() {
        let mut state = NetworkState::offline(0);
        let tcp = state
            .socket(1, SocketDomain::Inet4, SocketKind::Stream, 6, false)
            .unwrap();
        let value = state.sockets.get_mut(&tcp).unwrap();
        value.local = Some(("127.0.0.1".parse().unwrap(), 41000));
        value.peer = Some(("127.0.0.1".parse().unwrap(), 443));
        let diag = state
            .socket(1, SocketDomain::SockDiagNetlink, SocketKind::Raw, 4, false)
            .unwrap();
        let mut request = vec![0u8; 72];
        request[..4].copy_from_slice(&72u32.to_ne_bytes());
        request[4..6].copy_from_slice(&20u16.to_ne_bytes());
        request[6..8].copy_from_slice(&0x301u16.to_ne_bytes());
        request[8..12].copy_from_slice(&9u32.to_ne_bytes());
        request[16] = 2;
        request[17] = 6;
        request[20..24].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(state.netlink_send(diag, &request), Ok(72));
        let reply = state.netlink_recv(diag).unwrap();
        assert_eq!(u16::from_ne_bytes(reply[4..6].try_into().unwrap()), 20);
        assert_eq!(reply[17], 1);
        assert_eq!(u16::from_be_bytes(reply[20..22].try_into().unwrap()), 41000);
        assert_eq!(u16::from_be_bytes(reply[22..24].try_into().unwrap()), 443);
        assert_eq!(&reply[24..28], &[127, 0, 0, 1]);
    }
}
