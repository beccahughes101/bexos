use alloc::vec::Vec;

use bexos_network_extension_abi::{Hook, VECTOR_BATCH_SIZE};

use crate::extensions::ExtensionSet;
use crate::neighbor::NeighborTable;
use crate::packet::{ETHERTYPE_ARP, IpAddress, Packet, PacketDisposition};
use crate::routing::{InterfaceEndpoint, RoutingPlane};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GraphCounters {
    pub batches: u64,
    pub packets: u64,
    pub l2_packets: u64,
    pub routed_packets: u64,
    pub no_route: u64,
    pub ttl_expired: u64,
    pub neighbor_queued: u64,
    pub dropped: u64,
    pub icmp_errors: u64,
    pub icmp_rate_limited: u64,
}

/// Stable native graph node identifiers. A vector is regrouped at every
/// branch so each node consumes a dense list of packet indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum NodeId {
    Bridge = 0,
    PreRouting = 1,
    FibLookup = 2,
    Firewall = 3,
    PostRouting = 4,
    ChecksumMtu = 5,
}

const NODE_COUNT: usize = 6;

struct VectorDispatch {
    vectors: [Vec<usize>; NODE_COUNT],
}

impl Default for VectorDispatch {
    fn default() -> Self {
        Self {
            vectors: core::array::from_fn(|_| Vec::with_capacity(VECTOR_BATCH_SIZE)),
        }
    }
}

impl VectorDispatch {
    fn enqueue(&mut self, node: NodeId, packet: usize) {
        debug_assert!(packet < VECTOR_BATCH_SIZE);
        self.vectors[node as usize].push(packet);
    }

    fn take(&mut self, node: NodeId) -> Vec<usize> {
        core::mem::take(&mut self.vectors[node as usize])
    }
}

pub struct DataPlane {
    pub routing: RoutingPlane,
    pub neighbors: NeighborTable,
    pub extensions: ExtensionSet,
    pub counters: GraphCounters,
    pub icmp_window_ns: u64,
    pub icmp_in_window: u32,
}

impl Default for DataPlane {
    fn default() -> Self {
        Self {
            routing: RoutingPlane::default(),
            neighbors: NeighborTable::default(),
            extensions: ExtensionSet::default(),
            counters: GraphCounters::default(),
            icmp_window_ns: 0,
            icmp_in_window: 0,
        }
    }
}

impl DataPlane {
    pub fn process(&mut self, packets: &mut [Packet], now_ns: u64) -> Vec<Packet> {
        let mut additional = Vec::new();
        for vector in packets.chunks_mut(VECTOR_BATCH_SIZE) {
            self.process_vector(vector, now_ns, &mut additional);
        }
        additional
    }

    fn process_vector(
        &mut self,
        packets: &mut [Packet],
        now_ns: u64,
        additional: &mut Vec<Packet>,
    ) {
        self.counters.batches = self.counters.batches.saturating_add(1);
        self.counters.packets = self.counters.packets.saturating_add(packets.len() as u64);
        let mut dispatch = VectorDispatch::default();

        for (index, packet) in packets.iter_mut().enumerate() {
            let Some(interface) = self
                .routing
                .interfaces
                .get(&packet.descriptor.ingress_interface)
            else {
                packet.disposition = PacketDisposition::Drop;
                self.counters.dropped = self.counters.dropped.saturating_add(1);
                continue;
            };
            let endpoint_matches = matches!(
                (packet.descriptor.direction, interface.endpoint),
                (value, InterfaceEndpoint::Physical(_)) if value == bexos_network_extension_abi::Direction::PhysicalIngress as u8
            ) || matches!(
                (packet.descriptor.direction, interface.endpoint),
                (value, InterfaceEndpoint::Virtual(_)) if value == bexos_network_extension_abi::Direction::VirtualIngress as u8
            );
            let destination_matches =
                packet.destination_mac[0] & 1 != 0 || packet.destination_mac == interface.mac;
            let source_matches = packet.descriptor.direction
                != bexos_network_extension_abi::Direction::VirtualIngress as u8
                || packet.source_mac == interface.mac;
            if !endpoint_matches
                || packet.descriptor.vlan_id != interface.vlan_id
                || !destination_matches
                || !source_matches
            {
                packet.disposition = PacketDisposition::Drop;
                self.counters.dropped = self.counters.dropped.saturating_add(1);
                continue;
            }
            if packet.destination_ip.is_some() {
                dispatch.enqueue(NodeId::PreRouting, index);
            } else {
                dispatch.enqueue(NodeId::Bridge, index);
            }
        }

        let released = learn_neighbors(&mut self.neighbors, packets, now_ns);
        for mut packet in released {
            finish_output(&self.routing, &mut self.neighbors, &mut packet, now_ns);
            additional.push(packet);
        }

        let bridge = dispatch.take(NodeId::Bridge);
        self.counters.l2_packets = self.counters.l2_packets.saturating_add(bridge.len() as u64);
        self.extensions
            .process_indices(Hook::Bridge, packets, &bridge);

        let pre_routing = dispatch.take(NodeId::PreRouting);
        self.extensions
            .process_indices(Hook::PreRouting, packets, &pre_routing);
        refresh_rewrites(packets);
        for index in pre_routing {
            if packets[index].disposition != PacketDisposition::Drop {
                dispatch.enqueue(NodeId::FibLookup, index);
            }
        }

        for index in dispatch.take(NodeId::FibLookup) {
            let packet = &mut packets[index];
            let Some(destination) = packet.destination_ip else {
                packet.disposition = PacketDisposition::Drop;
                continue;
            };
            let interface = if packet.descriptor.egress_interface != 0 {
                self.routing
                    .interfaces
                    .get(&packet.descriptor.egress_interface)
                    .filter(|interface| interface.table_id == packet.descriptor.table_id)
            } else {
                self.routing
                    .lookup(packet.descriptor.table_id, destination)
                    .and_then(|route| self.routing.interfaces.get(&route.interface_id))
            };
            let Some(interface) = interface else {
                self.counters.no_route = self.counters.no_route.saturating_add(1);
                packet.disposition = PacketDisposition::Drop;
                continue;
            };
            packet.descriptor.egress_interface = interface.id;
            packet.descriptor.destination_zone = interface.zone;
            dispatch.enqueue(NodeId::Firewall, index);
        }

        let firewall = dispatch.take(NodeId::Firewall);
        self.extensions
            .process_indices(Hook::Firewall, packets, &firewall);
        for index in firewall {
            if packets[index].disposition != PacketDisposition::Drop {
                dispatch.enqueue(NodeId::PostRouting, index);
            }
        }

        let post_routing = dispatch.take(NodeId::PostRouting);
        self.extensions
            .process_indices(Hook::PostRouting, packets, &post_routing);
        refresh_rewrites(packets);
        for index in post_routing {
            if packets[index].disposition != PacketDisposition::Drop {
                dispatch.enqueue(NodeId::ChecksumMtu, index);
            }
        }

        for index in dispatch.take(NodeId::ChecksumMtu) {
            let packet = &mut packets[index];
            if matches!(packet.disposition, PacketDisposition::Deliver(_)) {
                continue;
            }
            let Some(interface) = self
                .routing
                .interfaces
                .get(&packet.descriptor.egress_interface)
                .filter(|interface| interface.table_id == packet.descriptor.table_id)
                .cloned()
            else {
                packet.disposition = PacketDisposition::Drop;
                continue;
            };
            if packet.decrement_hop_limit().is_err() {
                if self.allow_icmp(now_ns) {
                    if let Some(error) = icmp_error(packet, &self.routing, IcmpError::TimeExceeded)
                    {
                        additional.push(error);
                        self.counters.icmp_errors = self.counters.icmp_errors.saturating_add(1);
                    }
                }
                packet.disposition = PacketDisposition::Drop;
                self.counters.ttl_expired = self.counters.ttl_expired.saturating_add(1);
                continue;
            }
            if packet.bytes.len()
                > interface.mtu as usize + usize::from(packet.descriptor.l3_offset)
            {
                match packet.fragment_ipv4(interface.mtu as usize) {
                    Ok(mut fragments) => {
                        for fragment in &mut fragments {
                            finish_output(&self.routing, &mut self.neighbors, fragment, now_ns);
                        }
                        if fragments
                            .first()
                            .is_some_and(|fragment| fragment.disposition == PacketDisposition::Drop)
                        {
                            let destination = packet.destination_ip.unwrap();
                            let next_hop = self
                                .routing
                                .lookup(packet.descriptor.table_id, destination)
                                .and_then(|route| route.gateway)
                                .unwrap_or(destination);
                            let mut should_probe = false;
                            for fragment in fragments {
                                if let Ok(probe) =
                                    self.neighbors
                                        .queue(interface.id, next_hop, fragment, now_ns)
                                {
                                    should_probe |= probe;
                                    self.counters.neighbor_queued =
                                        self.counters.neighbor_queued.saturating_add(1);
                                }
                            }
                            if should_probe {
                                if let Some(probe) = neighbor_probe(&interface, next_hop) {
                                    additional.push(probe);
                                }
                            }
                            packet.disposition = PacketDisposition::Drop;
                            continue;
                        }
                        if let Some(first) = fragments.first().cloned() {
                            *packet = first;
                            additional.extend(fragments.into_iter().skip(1));
                            self.counters.routed_packets =
                                self.counters.routed_packets.saturating_add(1);
                        }
                    }
                    Err(_) => {
                        if self.allow_icmp(now_ns) {
                            if let Some(error) = icmp_error(
                                packet,
                                &self.routing,
                                IcmpError::PacketTooBig(interface.mtu),
                            ) {
                                additional.push(error);
                                self.counters.icmp_errors =
                                    self.counters.icmp_errors.saturating_add(1);
                            }
                        }
                        packet.disposition = PacketDisposition::Drop;
                        self.counters.dropped = self.counters.dropped.saturating_add(1);
                    }
                }
                continue;
            }
            let next_hop = self
                .routing
                .lookup(packet.descriptor.table_id, packet.destination_ip.unwrap())
                .and_then(|route| route.gateway)
                .unwrap_or(packet.destination_ip.unwrap());
            if let Some(mac) = self.neighbors.resolve(interface.id, next_hop, now_ns) {
                packet.rewrite_ethernet(interface.mac, mac);
                packet.disposition = match interface.endpoint {
                    InterfaceEndpoint::Virtual(port) => PacketDisposition::Deliver(port),
                    InterfaceEndpoint::Physical(physical) => PacketDisposition::Transmit(physical),
                };
            } else {
                let queued = packet.clone();
                if let Ok(should_probe) =
                    self.neighbors.queue(interface.id, next_hop, queued, now_ns)
                {
                    self.counters.neighbor_queued = self.counters.neighbor_queued.saturating_add(1);
                    if should_probe {
                        if let Some(probe) = neighbor_probe(&interface, next_hop) {
                            additional.push(probe);
                        }
                    }
                }
                packet.disposition = PacketDisposition::Drop;
            }
            self.counters.routed_packets = self.counters.routed_packets.saturating_add(1);
        }
    }

    fn allow_icmp(&mut self, now_ns: u64) -> bool {
        if now_ns.saturating_sub(self.icmp_window_ns) >= 1_000_000_000 {
            self.icmp_window_ns = now_ns;
            self.icmp_in_window = 0;
        }
        if self.icmp_in_window >= 64 {
            self.counters.icmp_rate_limited = self.counters.icmp_rate_limited.saturating_add(1);
            return false;
        }
        self.icmp_in_window += 1;
        true
    }
}

#[derive(Clone, Copy)]
enum IcmpError {
    TimeExceeded,
    PacketTooBig(u32),
}

fn icmp_error(packet: &Packet, routing: &RoutingPlane, error: IcmpError) -> Option<Packet> {
    let ingress = routing
        .interfaces
        .get(&packet.descriptor.ingress_interface)?;
    let source_ip = ingress
        .addresses
        .iter()
        .map(|(address, _)| *address)
        .find(|address| {
            matches!(
                (address, packet.source_ip),
                (IpAddress::V4(_), Some(IpAddress::V4(_)))
                    | (IpAddress::V6(_), Some(IpAddress::V6(_)))
            )
        })?;
    let destination_ip = packet.source_ip?;
    let direction = match ingress.endpoint {
        InterfaceEndpoint::Physical(_) => bexos_network_extension_abi::Direction::PhysicalIngress,
        InterfaceEndpoint::Virtual(_) => bexos_network_extension_abi::Direction::VirtualIngress,
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&packet.source_mac);
    bytes.extend_from_slice(&ingress.mac);
    match (source_ip, destination_ip) {
        (IpAddress::V4(source), IpAddress::V4(destination)) => {
            if packet.descriptor.protocol == 1 {
                let l4 = usize::from(packet.descriptor.l4_offset);
                if packet
                    .bytes
                    .get(l4)
                    .is_some_and(|kind| matches!(*kind, 3 | 4 | 5 | 11 | 12))
                {
                    return None;
                }
            }
            bytes.extend_from_slice(&crate::packet::ETHERTYPE_IPV4.to_be_bytes());
            let l3 = usize::from(packet.descriptor.l3_offset);
            let quote_len = packet.bytes.len().saturating_sub(l3).min(28);
            let total = 20 + 8 + quote_len;
            let ip = bytes.len();
            bytes.resize(ip + total, 0);
            bytes[ip] = 0x45;
            bytes[ip + 2..ip + 4].copy_from_slice(&(total as u16).to_be_bytes());
            bytes[ip + 8] = 64;
            bytes[ip + 9] = 1;
            bytes[ip + 12..ip + 16].copy_from_slice(&source);
            bytes[ip + 16..ip + 20].copy_from_slice(&destination);
            let icmp = ip + 20;
            match error {
                IcmpError::TimeExceeded => bytes[icmp] = 11,
                IcmpError::PacketTooBig(mtu) => {
                    bytes[icmp] = 3;
                    bytes[icmp + 1] = 4;
                    bytes[icmp + 6..icmp + 8]
                        .copy_from_slice(&(mtu.min(u32::from(u16::MAX)) as u16).to_be_bytes());
                }
            }
            bytes[icmp + 8..icmp + 8 + quote_len]
                .copy_from_slice(&packet.bytes[l3..l3 + quote_len]);
            let icmp_checksum = checksum(&bytes[icmp..]);
            bytes[icmp + 2..icmp + 4].copy_from_slice(&icmp_checksum.to_be_bytes());
            let ip_checksum = checksum(&bytes[ip..ip + 20]);
            bytes[ip + 10..ip + 12].copy_from_slice(&ip_checksum.to_be_bytes());
        }
        (IpAddress::V6(source), IpAddress::V6(destination)) => {
            if packet.descriptor.protocol == 58 {
                let l4 = usize::from(packet.descriptor.l4_offset);
                if packet.bytes.get(l4).is_some_and(|kind| *kind < 128) {
                    return None;
                }
            }
            bytes.extend_from_slice(&crate::packet::ETHERTYPE_IPV6.to_be_bytes());
            let l3 = usize::from(packet.descriptor.l3_offset);
            let quote_len = packet.bytes.len().saturating_sub(l3).min(1232);
            let payload_len = 8 + quote_len;
            let ip = bytes.len();
            bytes.resize(ip + 40 + payload_len, 0);
            bytes[ip] = 0x60;
            bytes[ip + 4..ip + 6].copy_from_slice(&(payload_len as u16).to_be_bytes());
            bytes[ip + 6] = 58;
            bytes[ip + 7] = 64;
            bytes[ip + 8..ip + 24].copy_from_slice(&source);
            bytes[ip + 24..ip + 40].copy_from_slice(&destination);
            let icmp = ip + 40;
            match error {
                IcmpError::TimeExceeded => bytes[icmp] = 3,
                IcmpError::PacketTooBig(mtu) => {
                    bytes[icmp] = 2;
                    bytes[icmp + 4..icmp + 8].copy_from_slice(&mtu.to_be_bytes());
                }
            }
            bytes[icmp + 8..icmp + 8 + quote_len]
                .copy_from_slice(&packet.bytes[l3..l3 + quote_len]);
            let mut pseudo = Vec::with_capacity(40 + payload_len);
            pseudo.extend_from_slice(&source);
            pseudo.extend_from_slice(&destination);
            pseudo.extend_from_slice(&(payload_len as u32).to_be_bytes());
            pseudo.extend_from_slice(&[0, 0, 0, 58]);
            pseudo.extend_from_slice(&bytes[icmp..]);
            let checksum = checksum(&pseudo);
            bytes[icmp + 2..icmp + 4].copy_from_slice(&checksum.to_be_bytes());
        }
        _ => return None,
    }
    let mut result = Packet::parse(
        &bytes,
        ingress.id,
        direction,
        ingress.table_id,
        ingress.zone,
    )
    .ok()?;
    result.disposition = match ingress.endpoint {
        InterfaceEndpoint::Physical(id) => PacketDisposition::Transmit(id),
        InterfaceEndpoint::Virtual(id) => PacketDisposition::Deliver(id),
    };
    Some(result)
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = bytes
        .chunks(2)
        .map(|word| u32::from(u16::from_be_bytes([word[0], *word.get(1).unwrap_or(&0)])))
        .sum::<u32>();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn finish_output(
    routing: &RoutingPlane,
    neighbors: &mut NeighborTable,
    packet: &mut Packet,
    now_ns: u64,
) {
    let Some(interface) = routing.interfaces.get(&packet.descriptor.egress_interface) else {
        packet.disposition = PacketDisposition::Drop;
        return;
    };
    let Some(destination) = packet.destination_ip else {
        packet.disposition = PacketDisposition::Drop;
        return;
    };
    let next_hop = routing
        .lookup(packet.descriptor.table_id, destination)
        .and_then(|route| route.gateway)
        .unwrap_or(destination);
    if let Some(mac) = neighbors.resolve(interface.id, next_hop, now_ns) {
        packet.rewrite_ethernet(interface.mac, mac);
        packet.disposition = match interface.endpoint {
            InterfaceEndpoint::Virtual(port) => PacketDisposition::Deliver(port),
            InterfaceEndpoint::Physical(physical) => PacketDisposition::Transmit(physical),
        };
    } else {
        packet.disposition = PacketDisposition::Drop;
    }
}

fn refresh_rewrites(packets: &mut [Packet]) {
    for packet in packets.iter_mut().filter(|packet| packet.rewritten) {
        if packet.refresh_network_metadata().is_err() {
            packet.disposition = PacketDisposition::Drop;
        }
        packet.rewritten = false;
    }
}

fn learn_neighbors(neighbors: &mut NeighborTable, packets: &[Packet], now_ns: u64) -> Vec<Packet> {
    let mut released = Vec::new();
    for packet in packets {
        if packet.disposition == PacketDisposition::Drop {
            continue;
        }
        if let Some(source) = packet.source_ip {
            if let Ok(mut packets) = neighbors.learn(
                packet.descriptor.ingress_interface,
                source,
                packet.source_mac,
                now_ns,
            ) {
                released.append(&mut packets);
            }
        } else if packet.ether_type == ETHERTYPE_ARP {
            let l3 = usize::from(packet.descriptor.l3_offset);
            if packet.bytes.len() >= l3 + 28
                && packet.bytes[l3..l3 + 2] == [0, 1]
                && packet.bytes[l3 + 2..l3 + 4] == [0x08, 0x00]
                && packet.bytes[l3 + 4] == 6
                && packet.bytes[l3 + 5] == 4
            {
                let mac = packet.bytes[l3 + 8..l3 + 14].try_into().unwrap();
                let ip = packet.bytes[l3 + 14..l3 + 18].try_into().unwrap();
                if let Ok(mut packets) = neighbors.learn(
                    packet.descriptor.ingress_interface,
                    IpAddress::V4(ip),
                    mac,
                    now_ns,
                ) {
                    released.append(&mut packets);
                }
            }
        }
    }
    released
}

fn neighbor_probe(
    interface: &crate::routing::RoutedInterface,
    target: IpAddress,
) -> Option<Packet> {
    let direction = match interface.endpoint {
        InterfaceEndpoint::Physical(_) => bexos_network_extension_abi::Direction::PhysicalIngress,
        InterfaceEndpoint::Virtual(_) => bexos_network_extension_abi::Direction::VirtualIngress,
    };
    let mut bytes = Vec::new();
    match target {
        IpAddress::V4(target) => {
            let source = interface
                .addresses
                .iter()
                .find_map(|(address, _)| match address {
                    IpAddress::V4(value) => Some(*value),
                    _ => None,
                })
                .unwrap_or([0; 4]);
            bytes.extend_from_slice(&[0xff; 6]);
            bytes.extend_from_slice(&interface.mac);
            bytes.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());
            bytes.extend_from_slice(&1u16.to_be_bytes());
            bytes.extend_from_slice(&crate::packet::ETHERTYPE_IPV4.to_be_bytes());
            bytes.extend_from_slice(&[6, 4]);
            bytes.extend_from_slice(&1u16.to_be_bytes());
            bytes.extend_from_slice(&interface.mac);
            bytes.extend_from_slice(&source);
            bytes.extend_from_slice(&[0; 6]);
            bytes.extend_from_slice(&target);
        }
        IpAddress::V6(target) => {
            let source = interface
                .addresses
                .iter()
                .find_map(|(address, _)| match address {
                    IpAddress::V6(value) => Some(*value),
                    _ => None,
                })
                .unwrap_or([0; 16]);
            let mut destination = [0u8; 16];
            destination[0] = 0xff;
            destination[1] = 0x02;
            destination[11] = 0x01;
            destination[12] = 0xff;
            destination[13..].copy_from_slice(&target[13..]);
            bytes.extend_from_slice(&[0x33, 0x33, 0xff, target[13], target[14], target[15]]);
            bytes.extend_from_slice(&interface.mac);
            bytes.extend_from_slice(&crate::packet::ETHERTYPE_IPV6.to_be_bytes());
            let ip = bytes.len();
            bytes.resize(ip + 40 + 32, 0);
            bytes[ip] = 0x60;
            bytes[ip + 4..ip + 6].copy_from_slice(&32u16.to_be_bytes());
            bytes[ip + 6] = 58;
            bytes[ip + 7] = 255;
            bytes[ip + 8..ip + 24].copy_from_slice(&source);
            bytes[ip + 24..ip + 40].copy_from_slice(&destination);
            let icmp = ip + 40;
            bytes[icmp] = 135;
            bytes[icmp + 8..icmp + 24].copy_from_slice(&target);
            bytes[icmp + 24] = 1;
            bytes[icmp + 25] = 1;
            bytes[icmp + 26..icmp + 32].copy_from_slice(&interface.mac);
            let mut pseudo = Vec::with_capacity(72);
            pseudo.extend_from_slice(&source);
            pseudo.extend_from_slice(&destination);
            pseudo.extend_from_slice(&32u32.to_be_bytes());
            pseudo.extend_from_slice(&[0, 0, 0, 58]);
            pseudo.extend_from_slice(&bytes[icmp..]);
            let value = checksum(&pseudo);
            bytes[icmp + 2..icmp + 4].copy_from_slice(&value.to_be_bytes());
        }
    }
    let mut packet = Packet::parse(
        &bytes,
        interface.id,
        direction,
        interface.table_id,
        interface.zone,
    )
    .ok()?;
    packet.disposition = match interface.endpoint {
        InterfaceEndpoint::Physical(id) => PacketDisposition::Transmit(id),
        InterfaceEndpoint::Virtual(id) => PacketDisposition::Deliver(id),
    };
    Some(packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::ETHERTYPE_IPV4;
    use crate::routing::{Route, RoutedInterface};
    use alloc::vec;
    use bexos_network_extension_abi::Direction;

    #[test]
    fn dispatch_groups_packets_by_next_node() {
        let mut dispatch = VectorDispatch::default();
        dispatch.enqueue(NodeId::Firewall, 3);
        dispatch.enqueue(NodeId::Bridge, 1);
        dispatch.enqueue(NodeId::Firewall, 7);
        dispatch.enqueue(NodeId::FibLookup, 2);

        assert_eq!(dispatch.take(NodeId::Bridge), vec![1]);
        assert_eq!(dispatch.take(NodeId::FibLookup), vec![2]);
        assert_eq!(dispatch.take(NodeId::Firewall), vec![3, 7]);
        assert!(dispatch.take(NodeId::Firewall).is_empty());
    }

    #[test]
    fn graph_routes_a_vector_by_longest_prefix() {
        let mut data = DataPlane::default();
        data.routing
            .configure_interface(RoutedInterface {
                id: 9,
                table_id: 1,
                bridge_domain: 1,
                endpoint: InterfaceEndpoint::Virtual(7),
                vlan_id: 0,
                mac: [2, 0, 0, 0, 0, 9],
                mtu: 1500,
                zone: 2,
                addresses: Vec::new(),
            })
            .unwrap();
        data.routing
            .add_route(Route {
                table_id: 1,
                network: IpAddress::V4([10, 0, 0, 0]),
                prefix_len: 8,
                gateway: None,
                interface_id: 9,
                metric: 1,
            })
            .unwrap();
        data.neighbors
            .learn(9, IpAddress::V4([10, 1, 2, 3]), [2, 0, 0, 0, 0, 7], 0)
            .unwrap();
        let mut bytes = vec![0u8; 34];
        bytes[0..6].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
        bytes[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        bytes[14] = 0x45;
        bytes[16..18].copy_from_slice(&20u16.to_be_bytes());
        bytes[22] = 64;
        bytes[23] = 17;
        bytes[26..30].copy_from_slice(&[1, 1, 1, 1]);
        bytes[30..34].copy_from_slice(&[10, 1, 2, 3]);
        // Fill the checksum after constructing the header.
        let mut sum = 0u32;
        for pair in bytes[14..34].chunks(2) {
            sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
        }
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        bytes[24..26].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        let packet = Packet::parse(&bytes, 9, Direction::VirtualIngress, 1, 1).unwrap();
        let mut packets = vec![packet];
        let _ = data.process(&mut packets, 1);
        assert_eq!(packets[0].disposition, PacketDisposition::Deliver(7));
    }

    #[test]
    fn graph_chunks_at_256_descriptors() {
        let mut data = DataPlane::default();
        data.routing
            .configure_interface(RoutedInterface {
                id: 1,
                table_id: 1,
                bridge_domain: 1,
                vlan_id: 0,
                endpoint: InterfaceEndpoint::Virtual(1),
                mac: [2, 0, 0, 0, 0, 1],
                mtu: 1500,
                zone: 1,
                addresses: Vec::new(),
            })
            .unwrap();
        let mut frame = vec![0u8; 14];
        frame[..6].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame[12..14].copy_from_slice(&0x88b5u16.to_be_bytes());
        let packet = Packet::parse(&frame, 1, Direction::VirtualIngress, 1, 1).unwrap();
        let mut packets = vec![packet; 257];
        let _ = data.process(&mut packets, 1);
        assert_eq!(data.counters.batches, 2);
        assert_eq!(data.counters.l2_packets, 257);
    }

    #[test]
    fn ttl_errors_are_native_and_rate_limited() {
        let mut data = DataPlane::default();
        data.routing
            .configure_interface(RoutedInterface {
                id: 1,
                table_id: 1,
                bridge_domain: 1,
                vlan_id: 0,
                endpoint: InterfaceEndpoint::Virtual(7),
                mac: [2, 0, 0, 0, 0, 1],
                mtu: 1500,
                zone: 1,
                addresses: vec![(IpAddress::V4([10, 0, 0, 1]), 24)],
            })
            .unwrap();
        data.routing
            .add_route(Route {
                table_id: 1,
                network: IpAddress::V4([0, 0, 0, 0]),
                prefix_len: 0,
                gateway: None,
                interface_id: 1,
                metric: 1,
            })
            .unwrap();
        let mut bytes = vec![0u8; 34];
        bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        bytes[14] = 0x45;
        bytes[16..18].copy_from_slice(&20u16.to_be_bytes());
        bytes[22] = 1;
        bytes[23] = 17;
        bytes[26..30].copy_from_slice(&[10, 0, 0, 2]);
        bytes[30..34].copy_from_slice(&[198, 51, 100, 1]);
        let checksum = checksum(&bytes[14..34]);
        bytes[24..26].copy_from_slice(&checksum.to_be_bytes());
        let packet = Packet::parse(&bytes, 1, Direction::VirtualIngress, 1, 1).unwrap();
        let mut packets = vec![packet; 70];
        let errors = data.process(&mut packets, 1);
        assert_eq!(errors.len(), 64);
        assert!(
            errors
                .iter()
                .all(|packet| packet.disposition == PacketDisposition::Deliver(7))
        );
        assert_eq!(data.counters.icmp_rate_limited, 6);
    }
}
