use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
    service_binding::BoundServiceEndpoint,
};

use crate::device::{VirtualDevice, append_resources, decode_device, encode_device};
use crate::extensions::{Extension, ExtensionCounters, FailurePolicy};
use crate::graph::DataPlane;
use crate::neighbor::{Neighbor, NeighborState};
use crate::packet::{IpAddress, Packet, PacketDisposition};
use crate::physical::{
    PhysicalDevice, append_resources as append_physical_resources,
    decode_device as decode_physical_device, encode_device as encode_physical_device,
};
use crate::routing::{InterfaceEndpoint, Route, RoutedInterface};
use crate::switch::{Port, PortPolicy, QueuedFrame, VirtualSwitch};

const VERSION: u64 = 4;
const DATA_PLANE_BASE: u64 = 10_000;
const DATA_PLANE_CHUNKS: usize = 240;
const DATA_PLANE_CHUNK_BYTES: usize = 32_600;
const MAX_DATA_PLANE_BYTES: usize = DATA_PLANE_CHUNKS * DATA_PLANE_CHUNK_BYTES;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub physical_devices: BTreeMap<u64, PhysicalDevice>,
    pub virtual_devices: BTreeMap<u64, VirtualDevice>,
    pub switch: VirtualSwitch,
    pub data_plane: DataPlane,
    pub generation: u64,
    migration_image: Vec<u8>,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>) -> Self {
        let mut data_plane = DataPlane::default();
        data_plane
            .extensions
            .install(crate::builtins::firewall(1).expect("bundled firewall must compile"))
            .expect("bundled firewall must install");
        let mut runtime = Self {
            control,
            migration,
            clients: Vec::new(),
            physical_devices: BTreeMap::new(),
            virtual_devices: BTreeMap::new(),
            switch: VirtualSwitch::default(),
            data_plane,
            generation: 0,
            migration_image: Vec::new(),
        };
        runtime
            .refresh_migration_image()
            .expect("initial data plane migration image");
        runtime
    }

    pub fn refresh_migration_image(&mut self) -> Result<(), Error> {
        self.migration_image = encode_data_plane(&self.data_plane)?;
        if self.migration_image.len() > MAX_DATA_PLANE_BYTES {
            return Err(Error::Capacity);
        }
        Ok(())
    }

    pub fn data_plane_keys() -> impl Iterator<Item = u64> {
        (0..DATA_PLANE_CHUNKS).map(|index| DATA_PLANE_BASE + index as u64)
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self::new(Channel(0), None)
    }

    fn keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![0, 1, 2];
        keys.extend(Self::data_plane_keys());
        keys
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            0 => {
                w.word(VERSION);
                w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |channel| channel.0));
                w.word(self.generation);
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.text(&client.protocol);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
            }
            1 => {
                w.word(VERSION);
                w.word(self.physical_devices.len() as u64);
                for (id, device) in &self.physical_devices {
                    w.word(*id);
                    encode_physical_device(&mut w, device);
                }
                w.word(self.virtual_devices.len() as u64);
                for (id, device) in &self.virtual_devices {
                    w.word(*id);
                    encode_device(&mut w, device);
                }
            }
            2 => {
                w.word(VERSION);
                w.word(self.switch.ports.len() as u64);
                for port in self.switch.ports.values() {
                    encode_port(&mut w, port);
                }
            }
            key if (DATA_PLANE_BASE..DATA_PLANE_BASE + DATA_PLANE_CHUNKS as u64).contains(&key) => {
                let index = (key - DATA_PLANE_BASE) as usize;
                let start = index * DATA_PLANE_CHUNK_BYTES;
                let end = self
                    .migration_image
                    .len()
                    .min(start + DATA_PLANE_CHUNK_BYTES);
                w.word(VERSION);
                w.word(self.migration_image.len() as u64);
                w.word(start as u64);
                if start < self.migration_image.len() {
                    w.bytes(&self.migration_image[start..end]);
                } else {
                    w.bytes(&[]);
                }
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = match r.word()? {
            1 => 1,
            2 => 2,
            3 => 3,
            VERSION => VERSION,
            _ => return Err(Error::UnsupportedVersion),
        };
        match key {
            0 => {
                let architecture = r.word()?;
                if architecture != if cfg!(bexos_arch_x86_64) { 2 } else { 1 } {
                    return Err(Error::InvalidData);
                }
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                self.generation = r.word()?;
                self.clients.clear();
                for _ in 0..r.count(64)? {
                    let channel = Channel(r.word()?);
                    let protocol = if version >= 4 {
                        r.text(64)?
                    } else {
                        "VirtualSwitchController"
                    };
                    let mut methods = Vec::new();
                    for _ in 0..r.count(16)? {
                        methods.push(r.word()?);
                    }
                    self.clients.push(BoundServiceEndpoint::new_with_protocol(
                        channel, methods, protocol,
                    ));
                }
            }
            1 => {
                self.physical_devices.clear();
                for _ in 0..r.count(64)? {
                    let id = r.word()?;
                    let device = if version == 1 {
                        PhysicalDevice::connect(r.word()?).map_err(|_| Error::InvalidData)?
                    } else {
                        decode_physical_device(&mut r)?
                    };
                    if self.physical_devices.insert(id, device).is_some() {
                        return Err(Error::InvalidData);
                    }
                }
                self.virtual_devices.clear();
                for _ in 0..r.count(256)? {
                    let id = r.word()?;
                    let device = if version == 1 {
                        VirtualDevice::new(Channel(r.word()?))
                    } else {
                        decode_device(&mut r)?
                    };
                    if self.virtual_devices.insert(id, device).is_some() {
                        return Err(Error::InvalidData);
                    }
                }
            }
            2 => {
                self.switch.ports.clear();
                for _ in 0..r.count(256)? {
                    let port = decode_port(&mut r, version)?;
                    if self
                        .switch
                        .ports
                        .insert(port.policy.port_id, port)
                        .is_some()
                    {
                        return Err(Error::InvalidData);
                    }
                }
            }
            key if (DATA_PLANE_BASE..DATA_PLANE_BASE + DATA_PLANE_CHUNKS as u64).contains(&key) => {
                let total = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let offset = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let expected = (key - DATA_PLANE_BASE) as usize * DATA_PLANE_CHUNK_BYTES;
                let chunk = r.bytes(DATA_PLANE_CHUNK_BYTES)?;
                if total > MAX_DATA_PLANE_BYTES
                    || offset != expected
                    || offset > total
                    || chunk.len() != total.saturating_sub(offset).min(DATA_PLANE_CHUNK_BYTES)
                {
                    return Err(Error::InvalidData);
                }
                if self.migration_image.len() != total {
                    self.migration_image.resize(total, 0);
                }
                self.migration_image[offset..offset + chunk.len()].copy_from_slice(chunk);
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()?;
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self
                .virtual_devices
                .keys()
                .any(|port| !self.switch.ports.contains_key(port))
            || self.switch.ports.values().any(|port| {
                !self
                    .physical_devices
                    .contains_key(&port.policy.physical_interface)
                    || port.policy.rx_capacity == 0
                    || port.policy.rx_capacity > 4096
                    || port.policy.tx_capacity == 0
                    || port.policy.tx_capacity > 4096
                    || port.rx.len().saturating_add(port.delivered_rx.len())
                        > port.policy.rx_capacity
                    || port.staged_tx.len() > port.policy.tx_capacity
            })
        {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn prepare_adoption(&mut self) -> Result<(), Error> {
        self.data_plane = decode_data_plane(&self.migration_image)?;
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut resources = alloc::vec![Resource::Handle(self.control.0)];
        if let Some(migration) = self.migration {
            resources.push(Resource::Handle(migration.0));
        }
        resources.extend(
            self.clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        for device in self.physical_devices.values() {
            append_physical_resources(&mut resources, device);
        }
        for device in self.virtual_devices.values() {
            append_resources(&mut resources, device);
        }
        resources
    }

    fn activated(&mut self, generation: u64) {
        self.generation = generation;
    }
}

fn encode_port(w: &mut Encoder, port: &Port) {
    w.word(port.policy.port_id);
    w.word(port.policy.physical_interface);
    w.bytes(&port.policy.source_mac);
    w.word(port.policy.vlan_id.map_or(0, |value| u64::from(value) + 1));
    w.word(port.policy.tagged as u64);
    w.word(port.policy.rx_capacity as u64);
    w.word(port.policy.tx_capacity as u64);
    w.word(port.committed_generation);
    w.word(port.dropped_rx);
    w.word(port.dropped_tx);
    w.word(port.spoof_rejections);
    encode_queue(w, &port.rx);
    encode_queue(w, &port.delivered_rx);
    encode_queue(w, &port.staged_tx);
}

fn decode_port(r: &mut Decoder<'_>, version: u64) -> Result<Port, Error> {
    let port_id = r.word()?;
    let physical_interface = r.word()?;
    let source_mac = r.bytes(6)?.try_into().map_err(|_| Error::InvalidData)?;
    let encoded_vlan = r.word()?;
    let vlan_id = if encoded_vlan == 0 {
        None
    } else {
        Some(u16::try_from(encoded_vlan - 1).map_err(|_| Error::InvalidData)?)
    };
    let tagged = r.flag()?;
    let rx_capacity = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let tx_capacity = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let committed_generation = r.word()?;
    let dropped_rx = r.word()?;
    let dropped_tx = r.word()?;
    let spoof_rejections = r.word()?;
    Ok(Port {
        policy: PortPolicy {
            port_id,
            physical_interface,
            source_mac,
            vlan_id,
            tagged,
            rx_capacity,
            tx_capacity,
        },
        rx: decode_queue(r, rx_capacity)?,
        delivered_rx: if version >= 3 {
            decode_queue(r, rx_capacity)?
        } else {
            VecDeque::new()
        },
        staged_tx: decode_queue(r, tx_capacity)?,
        committed_generation,
        dropped_rx,
        dropped_tx,
        spoof_rejections,
    })
}

fn encode_queue(w: &mut Encoder, queue: &VecDeque<QueuedFrame>) {
    w.word(queue.len() as u64);
    for frame in queue {
        w.word(frame.generation);
        w.bytes(&frame.bytes);
    }
}

fn decode_queue(r: &mut Decoder<'_>, capacity: usize) -> Result<VecDeque<QueuedFrame>, Error> {
    let mut queue = VecDeque::new();
    for _ in 0..r.count(capacity)? {
        queue.push_back(QueuedFrame {
            generation: r.word()?,
            bytes: r.bytes(crate::switch::MAX_FRAME_SIZE)?.to_vec(),
        });
    }
    Ok(queue)
}

fn encode_data_plane(data: &DataPlane) -> Result<Vec<u8>, Error> {
    let mut w = Encoder::new();
    w.word(3);
    for value in [
        data.counters.batches,
        data.counters.packets,
        data.counters.l2_packets,
        data.counters.routed_packets,
        data.counters.no_route,
        data.counters.ttl_expired,
        data.counters.neighbor_queued,
        data.counters.dropped,
        data.counters.icmp_errors,
        data.counters.icmp_rate_limited,
    ] {
        w.word(value);
    }
    w.word(data.icmp_window_ns);
    w.word(data.icmp_in_window as u64);
    w.word(data.routing.interfaces.len() as u64);
    for interface in data.routing.interfaces.values() {
        w.word(interface.id);
        w.word(interface.table_id as u64);
        w.word(interface.bridge_domain as u64);
        w.word(interface.vlan_id as u64);
        match interface.endpoint {
            InterfaceEndpoint::Physical(id) => {
                w.word(1);
                w.word(id);
            }
            InterfaceEndpoint::Virtual(id) => {
                w.word(2);
                w.word(id);
            }
        }
        w.bytes(&interface.mac);
        w.word(interface.mtu as u64);
        w.word(interface.zone as u64);
        w.word(interface.addresses.len() as u64);
        for (address, prefix) in &interface.addresses {
            encode_ip(&mut w, *address);
            w.word(*prefix as u64);
        }
    }
    let routes = data.routing.tables.values().flatten().collect::<Vec<_>>();
    w.word(routes.len() as u64);
    for route in routes {
        w.word(route.table_id as u64);
        encode_ip(&mut w, route.network);
        w.word(route.prefix_len as u64);
        w.word(route.gateway.is_some() as u64);
        if let Some(gateway) = route.gateway {
            encode_ip(&mut w, gateway);
        }
        w.word(route.interface_id);
        w.word(route.metric as u64);
    }
    let neighbors = data.neighbors.entries().collect::<Vec<_>>();
    w.word(neighbors.len() as u64);
    for neighbor in neighbors {
        w.word(neighbor.interface_id);
        encode_ip(&mut w, neighbor.address);
        w.bytes(&neighbor.mac);
        w.word(match neighbor.state {
            NeighborState::Incomplete => 1,
            NeighborState::Reachable => 2,
            NeighborState::Stale => 3,
        });
        w.word(neighbor.updated_ns);
        w.word(neighbor.probes as u64);
        w.word(neighbor.queued.len() as u64);
        for packet in &neighbor.queued {
            encode_packet(&mut w, packet);
        }
    }
    let extensions = data.extensions.iter().collect::<Vec<_>>();
    w.word(extensions.len() as u64);
    for extension in extensions {
        w.text(&extension.name);
        w.word(extension.hook as u64);
        w.word(match extension.policy {
            FailurePolicy::Closed => 1,
            FailurePolicy::Open => 2,
        });
        w.bytes(&extension.digest);
        w.bytes(&extension.module_bytes);
        w.bytes(&extension.config);
        w.word(extension.generation);
        for value in [
            extension.counters.batches,
            extension.counters.packets,
            extension.counters.passed,
            extension.counters.dropped,
            extension.counters.rewritten,
            extension.counters.redirected,
            extension.counters.traps,
        ] {
            w.word(value);
        }
        w.word(extension.faulted as u64);
        w.bytes(&extension.snapshot());
    }
    let bytes = w.finish();
    (bytes.len() <= MAX_DATA_PLANE_BYTES)
        .then_some(bytes)
        .ok_or(Error::Capacity)
}

fn decode_data_plane(bytes: &[u8]) -> Result<DataPlane, Error> {
    let mut r = Decoder::new(bytes);
    let version = r.word()?;
    if !matches!(version, 1 | 2 | 3) {
        return Err(Error::UnsupportedVersion);
    }
    let mut data = DataPlane::default();
    data.extensions.clear();
    data.counters.batches = r.word()?;
    data.counters.packets = r.word()?;
    data.counters.l2_packets = r.word()?;
    data.counters.routed_packets = r.word()?;
    data.counters.no_route = r.word()?;
    data.counters.ttl_expired = r.word()?;
    data.counters.neighbor_queued = r.word()?;
    data.counters.dropped = r.word()?;
    if version >= 3 {
        data.counters.icmp_errors = r.word()?;
        data.counters.icmp_rate_limited = r.word()?;
        data.icmp_window_ns = r.word()?;
        data.icmp_in_window = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    }
    for _ in 0..r.count(crate::routing::MAX_INTERFACES)? {
        let id = r.word()?;
        let table_id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let bridge_domain = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let vlan_id = if version >= 2 {
            u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?
        } else {
            0
        };
        let endpoint = match r.word()? {
            1 => InterfaceEndpoint::Physical(r.word()?),
            2 => InterfaceEndpoint::Virtual(r.word()?),
            _ => return Err(Error::InvalidData),
        };
        let mac = r.bytes(6)?.try_into().map_err(|_| Error::InvalidData)?;
        let mtu = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let zone = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let mut addresses = Vec::new();
        for _ in 0..r.count(16)? {
            addresses.push((
                decode_ip(&mut r)?,
                u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            ));
        }
        data.routing
            .configure_interface(RoutedInterface {
                id,
                table_id,
                bridge_domain,
                vlan_id,
                endpoint,
                mac,
                mtu,
                zone,
                addresses,
            })
            .map_err(|_| Error::InvalidData)?;
    }
    for _ in 0..r.count(crate::routing::MAX_ROUTES_PER_TABLE * crate::routing::MAX_INTERFACES)? {
        let table_id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let network = decode_ip(&mut r)?;
        let prefix_len = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let gateway = if r.flag()? {
            Some(decode_ip(&mut r)?)
        } else {
            None
        };
        let interface_id = r.word()?;
        let metric = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        data.routing
            .add_route(Route {
                table_id,
                network,
                prefix_len,
                gateway,
                interface_id,
                metric,
            })
            .map_err(|_| Error::InvalidData)?;
    }
    data.neighbors.clear();
    for _ in 0..r.count(crate::neighbor::MAX_NEIGHBORS)? {
        let interface_id = r.word()?;
        let address = decode_ip(&mut r)?;
        let mac = r.bytes(6)?.try_into().map_err(|_| Error::InvalidData)?;
        let state = match r.word()? {
            1 => NeighborState::Incomplete,
            2 => NeighborState::Reachable,
            3 => NeighborState::Stale,
            _ => return Err(Error::InvalidData),
        };
        let updated_ns = r.word()?;
        let probes = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let mut queued = VecDeque::new();
        for _ in 0..r.count(crate::neighbor::MAX_QUEUED_PER_NEIGHBOR)? {
            queued.push_back(decode_packet(&mut r)?);
        }
        data.neighbors
            .restore(Neighbor {
                interface_id,
                address,
                mac,
                state,
                updated_ns,
                probes,
                queued,
            })
            .map_err(|_| Error::InvalidData)?;
    }
    for _ in 0..r.count(8)? {
        let name = r.text(64)?;
        let hook = match r.word()? {
            1 => bexos_network_extension_abi::Hook::Bridge,
            2 => bexos_network_extension_abi::Hook::PreRouting,
            3 => bexos_network_extension_abi::Hook::Firewall,
            4 => bexos_network_extension_abi::Hook::PostRouting,
            _ => return Err(Error::InvalidData),
        };
        let policy = match r.word()? {
            1 => FailurePolicy::Closed,
            2 => FailurePolicy::Open,
            _ => return Err(Error::InvalidData),
        };
        let digest = r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?;
        let module = r.bytes(4 << 20)?;
        let config = r.bytes(bexos_network_extension_abi::MAX_CONFIG_BYTES)?;
        let generation = r.word()?;
        let counters = ExtensionCounters {
            batches: r.word()?,
            packets: r.word()?,
            passed: r.word()?,
            dropped: r.word()?,
            rewritten: r.word()?,
            redirected: r.word()?,
            traps: r.word()?,
        };
        let faulted = r.flag()?;
        let checkpoint = r.bytes(4 << 20)?;
        let mut extension =
            Extension::compile(name, hook, policy, digest, module, config, generation)
                .map_err(|_| Error::InvalidData)?;
        extension
            .restore(checkpoint)
            .map_err(|_| Error::InvalidData)?;
        extension.counters = counters;
        extension.faulted = faulted;
        data.extensions
            .install(extension)
            .map_err(|_| Error::InvalidData)?;
    }
    r.finish()?;
    Ok(data)
}

fn encode_ip(w: &mut Encoder, address: IpAddress) {
    match address {
        IpAddress::V4(value) => {
            w.word(4);
            w.bytes(&value);
        }
        IpAddress::V6(value) => {
            w.word(6);
            w.bytes(&value);
        }
    }
}

fn decode_ip(r: &mut Decoder<'_>) -> Result<IpAddress, Error> {
    match r.word()? {
        4 => Ok(IpAddress::V4(
            r.bytes(4)?.try_into().map_err(|_| Error::InvalidData)?,
        )),
        6 => Ok(IpAddress::V6(
            r.bytes(16)?.try_into().map_err(|_| Error::InvalidData)?,
        )),
        _ => Err(Error::InvalidData),
    }
}

fn encode_packet(w: &mut Encoder, packet: &Packet) {
    w.bytes(&packet.bytes);
    w.word(packet.descriptor.ingress_interface);
    w.word(packet.descriptor.egress_interface);
    w.word(packet.descriptor.table_id as u64);
    w.word(packet.descriptor.source_zone as u64);
    w.word(packet.descriptor.destination_zone as u64);
    w.word(packet.descriptor.direction as u64);
    let (kind, target) = match packet.disposition {
        PacketDisposition::Continue => (1, 0),
        PacketDisposition::Drop => (2, 0),
        PacketDisposition::Deliver(id) => (3, id),
        PacketDisposition::Transmit(id) => (4, id),
    };
    w.word(kind);
    w.word(target);
    w.word(packet.rewritten as u64);
}

fn decode_packet(r: &mut Decoder<'_>) -> Result<Packet, Error> {
    let bytes = r.bytes(crate::switch::MAX_FRAME_SIZE)?.to_vec();
    let ingress = r.word()?;
    let egress = r.word()?;
    let table = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let source_zone = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let destination_zone = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let direction = match r.word()? {
        1 => bexos_network_extension_abi::Direction::PhysicalIngress,
        2 => bexos_network_extension_abi::Direction::VirtualIngress,
        _ => return Err(Error::InvalidData),
    };
    let kind = r.word()?;
    let target = r.word()?;
    let rewritten = r.flag()?;
    let mut packet = Packet::parse(&bytes, ingress, direction, table, source_zone)
        .map_err(|_| Error::InvalidData)?;
    packet.descriptor.egress_interface = egress;
    packet.descriptor.destination_zone = destination_zone;
    packet.disposition = match kind {
        1 => PacketDisposition::Continue,
        2 => PacketDisposition::Drop,
        3 => PacketDisposition::Deliver(target),
        4 => PacketDisposition::Transmit(target),
        _ => return Err(Error::InvalidData),
    };
    packet.rewritten = rewritten;
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use bexos_network_extension_abi::{Direction, Hook};

    use super::*;
    use crate::builtins::nat;

    #[test]
    fn port_policy_queues_and_generation_round_trip() {
        let port = Port {
            policy: PortPolicy {
                port_id: 9,
                physical_interface: 3,
                source_mac: [2, 0, 0, 0, 0, 9],
                vlan_id: Some(44),
                tagged: false,
                rx_capacity: 4,
                tx_capacity: 4,
            },
            rx: VecDeque::from([QueuedFrame {
                generation: 10,
                bytes: alloc::vec![1, 2, 3],
            }]),
            delivered_rx: VecDeque::from([QueuedFrame {
                generation: 11,
                bytes: alloc::vec![3, 2, 1],
            }]),
            staged_tx: VecDeque::from([QueuedFrame {
                generation: 11,
                bytes: alloc::vec![4, 5, 6],
            }]),
            committed_generation: 10,
            dropped_rx: 1,
            dropped_tx: 2,
            spoof_rejections: 3,
        };
        let mut encoder = Encoder::new();
        encode_port(&mut encoder, &port);
        let bytes = encoder.finish();
        let mut decoder = Decoder::new(&bytes);
        let adopted = decode_port(&mut decoder, VERSION).unwrap();
        decoder.finish().unwrap();
        assert_eq!(adopted.policy, port.policy);
        assert_eq!(adopted.rx, port.rx);
        assert_eq!(adopted.delivered_rx, port.delivered_rx);
        assert_eq!(adopted.staged_tx, port.staged_tx);
        assert_eq!(adopted.committed_generation, 10);
        assert_eq!(adopted.spoof_rejections, 3);
    }

    #[test]
    fn rejects_architecture_mismatch() {
        let runtime = Runtime::new(Channel(1), Some(Channel(2)));
        let mut record = runtime.encode_record(0).unwrap().unwrap();
        // Encoder words are little endian and the architecture is the second
        // word. Replace it with a value no supported target can use.
        record[8..16].copy_from_slice(&99u64.to_le_bytes());
        let mut adopted = Runtime::empty();
        assert_eq!(
            adopted.adopt_record(0, Some(&record)),
            Err(Error::InvalidData)
        );
    }

    #[test]
    fn routed_neighbor_and_nat_checkpoint_round_trip() {
        let mut data = DataPlane::default();
        data.routing
            .configure_interface(RoutedInterface {
                id: 7,
                table_id: 3,
                bridge_domain: 11,
                vlan_id: 44,
                endpoint: InterfaceEndpoint::Physical(2),
                mac: [2, 0, 0, 0, 0, 7],
                mtu: 1500,
                zone: 2,
                addresses: alloc::vec![(IpAddress::V4([203, 0, 113, 1]), 24)],
            })
            .unwrap();
        data.routing
            .add_route(Route {
                table_id: 3,
                network: IpAddress::V4([0, 0, 0, 0]),
                prefix_len: 0,
                gateway: Some(IpAddress::V4([203, 0, 113, 254])),
                interface_id: 7,
                metric: 20,
            })
            .unwrap();

        let queued = udp_packet(
            [10, 0, 0, 2],
            [198, 51, 100, 9],
            1234,
            53,
            Direction::VirtualIngress,
        );
        data.neighbors
            .queue(7, IpAddress::V4([203, 0, 113, 254]), queued, 99)
            .unwrap();

        let mut config = alloc::vec![0u8; 48];
        config[..4].copy_from_slice(b"NAT1");
        config[4..8].copy_from_slice(&[203, 0, 113, 5]);
        config[8..10].copy_from_slice(&40000u16.to_le_bytes());
        config[10..12].copy_from_slice(&40010u16.to_le_bytes());
        data.extensions
            .replace_nat_pair(nat(&config, 9).unwrap())
            .unwrap();
        let mut outbound = alloc::vec![udp_packet(
            [10, 0, 0, 2],
            [198, 51, 100, 9],
            1234,
            53,
            Direction::VirtualIngress,
        )];
        data.extensions.process(Hook::PostRouting, &mut outbound);

        let image = encode_data_plane(&data).unwrap();
        let mut adopted = decode_data_plane(&image).unwrap();
        let route = adopted
            .routing
            .lookup(3, IpAddress::V4([192, 0, 2, 1]))
            .unwrap();
        assert_eq!(route.gateway, Some(IpAddress::V4([203, 0, 113, 254])));
        let neighbor = adopted.neighbors.entries().next().unwrap();
        assert_eq!(neighbor.queued.len(), 1);
        assert_eq!(adopted.extensions.iter().count(), 2);
        assert!(
            adopted
                .extensions
                .iter()
                .all(|extension| extension.generation == 9)
        );

        let mut inbound = alloc::vec![udp_packet(
            [198, 51, 100, 9],
            [203, 0, 113, 5],
            53,
            40000,
            Direction::PhysicalIngress,
        )];
        adopted.extensions.process(Hook::PreRouting, &mut inbound);
        assert_eq!(&inbound[0].bytes[30..34], &[10, 0, 0, 2]);
        assert_eq!(
            u16::from_be_bytes(inbound[0].bytes[36..38].try_into().unwrap()),
            1234
        );
    }

    fn udp_packet(
        source: [u8; 4],
        destination: [u8; 4],
        source_port: u16,
        destination_port: u16,
        direction: Direction,
    ) -> Packet {
        let mut bytes = alloc::vec![0u8; 42];
        bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        bytes[14] = 0x45;
        bytes[16..18].copy_from_slice(&28u16.to_be_bytes());
        bytes[22] = 64;
        bytes[23] = 17;
        bytes[26..30].copy_from_slice(&source);
        bytes[30..34].copy_from_slice(&destination);
        bytes[34..36].copy_from_slice(&source_port.to_be_bytes());
        bytes[36..38].copy_from_slice(&destination_port.to_be_bytes());
        bytes[38..40].copy_from_slice(&8u16.to_be_bytes());
        let mut sum = bytes[14..34]
            .chunks(2)
            .map(|word| u32::from(u16::from_be_bytes([word[0], *word.get(1).unwrap_or(&0)])))
            .sum::<u32>();
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        bytes[24..26].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        Packet::parse(&bytes, 1, direction, 3, 1).unwrap()
    }
}
