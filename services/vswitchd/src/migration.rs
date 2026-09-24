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
use crate::physical::{
    PhysicalDevice, append_resources as append_physical_resources,
    decode_device as decode_physical_device, encode_device as encode_physical_device,
};
use crate::switch::{Port, PortPolicy, QueuedFrame, VirtualSwitch};

const VERSION: u64 = 3;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub physical_devices: BTreeMap<u64, PhysicalDevice>,
    pub virtual_devices: BTreeMap<u64, VirtualDevice>,
    pub switch: VirtualSwitch,
    pub generation: u64,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>) -> Self {
        Self {
            control,
            migration,
            clients: Vec::new(),
            physical_devices: BTreeMap::new(),
            virtual_devices: BTreeMap::new(),
            switch: VirtualSwitch::default(),
            generation: 0,
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self::new(Channel(0), None)
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2]
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
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = match r.word()? {
            1 => 1,
            2 => 2,
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
                    let mut methods = Vec::new();
                    for _ in 0..r.count(16)? {
                        methods.push(r.word()?);
                    }
                    self.clients.push(BoundServiceEndpoint::new_with_protocol(
                        channel,
                        methods,
                        "VirtualSwitchController",
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
