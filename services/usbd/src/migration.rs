use crate::topology::{Device, Interface, Topology};
use alloc::{collections::BTreeMap, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_usb_host::{
    ClassPolicy, InterfaceClass,
    descriptor::{DeviceDescriptor, EndpointDescriptor},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
    service_binding::BoundServiceEndpoint,
};
use usb_host_fidl::UsbSpeed;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub controller: Option<Channel>,
    pub controller_bus: Option<Channel>,
    pub controller_events: Option<Channel>,
    pub registry: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub topology_watchers: Vec<Channel>,
    pub topology: Topology,
    pub policy: ClassPolicy,
    pub claims: BTreeMap<u64, u64>,
    pub queued_requests: Vec<u64>,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>) -> Self {
        Self {
            control,
            migration,
            controller: None,
            controller_bus: None,
            controller_events: None,
            registry: None,
            clients: Vec::new(),
            topology_watchers: Vec::new(),
            topology: Topology::default(),
            policy: ClassPolicy::default(),
            claims: BTreeMap::new(),
            queued_requests: Vec::new(),
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self::new(Channel(0), None)
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2, 3]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            0 => {
                w.word(1);
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |c| c.0));
                w.word(self.controller.map_or(0, |c| c.0));
                w.word(self.controller_bus.map_or(0, |c| c.0));
                w.word(self.controller_events.map_or(0, |c| c.0));
                w.word(self.registry.map_or(0, |c| c.0));
                w.word(self.policy.hubs as u64);
                w.word(self.policy.boot_hid as u64);
                w.word(self.policy.bulk_storage as u64);
            }
            1 => encode_topology(&mut w, &self.topology),
            2 => {
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
                w.word(self.topology_watchers.len() as u64);
                for watcher in &self.topology_watchers {
                    w.word(watcher.0);
                }
            }
            3 => {
                w.word(self.claims.len() as u64);
                for (interface, owner) in &self.claims {
                    w.word(*interface);
                    w.word(*owner);
                }
                w.word(self.queued_requests.len() as u64);
                for request in &self.queued_requests {
                    w.word(*request);
                }
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        match key {
            0 => {
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(r.word()?);
                self.migration = Some(Channel(r.word()?));
                self.controller = nonzero(r.word()?);
                self.controller_bus = nonzero(r.word()?);
                self.controller_events = nonzero(r.word()?);
                self.registry = nonzero(r.word()?);
                self.policy = ClassPolicy {
                    hubs: r.flag()?,
                    boot_hid: r.flag()?,
                    bulk_storage: r.flag()?,
                };
            }
            1 => self.topology = decode_topology(&mut r)?,
            2 => {
                self.clients.clear();
                for _ in 0..r.count(64)? {
                    let channel = Channel(r.word()?);
                    let mut allowed = Vec::new();
                    for _ in 0..r.count(16)? {
                        allowed.push(r.word()?);
                    }
                    self.clients
                        .push(BoundServiceEndpoint::new(channel, allowed));
                }
                self.topology_watchers.clear();
                for _ in 0..r.count(64)? {
                    self.topology_watchers.push(Channel(r.word()?));
                }
            }
            3 => {
                self.claims.clear();
                for _ in 0..r.count(256)? {
                    self.claims.insert(r.word()?, r.word()?);
                }
                self.queued_requests.clear();
                for _ in 0..r.count(4096)? {
                    self.queued_requests.push(r.word()?);
                }
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        for (interface, owner) in &self.claims {
            if *interface == 0 || *owner == 0 {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        for channel in [
            self.migration,
            self.controller,
            self.controller_bus,
            self.controller_events,
            self.registry,
        ]
        .into_iter()
        .flatten()
        {
            out.push(Resource::Handle(channel.0));
        }
        out.extend(
            self.clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        out.extend(
            self.topology_watchers
                .iter()
                .map(|watcher| Resource::Handle(watcher.0)),
        );
        for device in self.topology.devices.values() {
            for interface in &device.interfaces {
                if let Some(channel) = interface.channel {
                    out.push(Resource::Handle(channel));
                }
            }
        }
        out
    }

    fn activated(&mut self, generation: u64) {
        bexos_userspace::log(&alloc::format!("usbd: adopted generation={generation}\n"));
    }
}

fn encode_topology(w: &mut Encoder, topology: &Topology) {
    w.word(1);
    w.word(topology.next_device_id);
    w.word(topology.next_interface_id);
    w.word(topology.generation);
    w.word(topology.devices.len() as u64);
    for device in topology.devices.values() {
        w.word(device.id);
        w.word(device.parent_id);
        w.word(device.port as u64);
        w.word(device.generation);
        w.word(device.speed as u64);
        encode_device_descriptor(w, &device.descriptor);
        w.word(device.configuration_value as u64);
        w.word(device.interfaces.len() as u64);
        for interface in &device.interfaces {
            encode_interface(w, interface);
        }
    }
}

fn decode_topology(r: &mut Decoder<'_>) -> Result<Topology, Error> {
    if r.word()? != 1 {
        return Err(Error::UnsupportedVersion);
    }
    let mut topology = Topology {
        next_device_id: r.word()?,
        next_interface_id: r.word()?,
        generation: r.word()?,
        devices: BTreeMap::new(),
    };
    for _ in 0..r.count(256)? {
        let id = r.word()?;
        let parent_id = r.word()?;
        let port = r.word()? as u8;
        let generation = r.word()?;
        let speed = decode_speed(r.word()?)?;
        let descriptor = decode_device_descriptor(r)?;
        let configuration_value = r.word()? as u8;
        let mut interfaces = Vec::new();
        for _ in 0..r.count(32)? {
            interfaces.push(decode_interface(r, id)?);
        }
        if topology
            .devices
            .insert(
                id,
                Device {
                    id,
                    parent_id,
                    port,
                    generation,
                    speed,
                    descriptor,
                    configuration_value,
                    interfaces,
                },
            )
            .is_some()
        {
            return Err(Error::InvalidData);
        }
    }
    Ok(topology)
}

fn encode_device_descriptor(w: &mut Encoder, d: &DeviceDescriptor) {
    for value in [
        d.usb_bcd,
        u16::from(d.class_code),
        u16::from(d.subclass),
        u16::from(d.protocol),
        u16::from(d.max_packet_size0),
        d.vendor_id,
        d.product_id,
        d.device_bcd,
        u16::from(d.configurations),
    ] {
        w.word(u64::from(value));
    }
}

fn decode_device_descriptor(r: &mut Decoder<'_>) -> Result<DeviceDescriptor, Error> {
    Ok(DeviceDescriptor {
        usb_bcd: r.word()? as u16,
        class_code: r.word()? as u8,
        subclass: r.word()? as u8,
        protocol: r.word()? as u8,
        max_packet_size0: r.word()? as u8,
        vendor_id: r.word()? as u16,
        product_id: r.word()? as u16,
        device_bcd: r.word()? as u16,
        configurations: r.word()? as u8,
    })
}

fn encode_interface(w: &mut Encoder, interface: &Interface) {
    w.word(interface.id);
    w.word(interface.device_id);
    w.word(interface.number as u64);
    w.word(interface.generation);
    w.word(interface.class as u64);
    w.word(interface.class_code as u64);
    w.word(interface.subclass as u64);
    w.word(interface.protocol as u64);
    w.word(interface.speed as u64);
    w.word(interface.owner.unwrap_or(0));
    w.word(interface.channel.unwrap_or(0));
    w.word(interface.endpoints.len() as u64);
    for endpoint in &interface.endpoints {
        w.word(endpoint.address as u64);
        w.word(endpoint.attributes as u64);
        w.word(endpoint.max_packet_size as u64);
        w.word(endpoint.interval as u64);
    }
}

fn decode_interface(r: &mut Decoder<'_>, device_id: u64) -> Result<Interface, Error> {
    let id = r.word()?;
    let stored_device_id = r.word()?;
    if stored_device_id != device_id {
        return Err(Error::InvalidData);
    }
    let number = r.word()? as u8;
    let generation = r.word()?;
    let class = match r.word()? {
        0 => InterfaceClass::Hub,
        1 => InterfaceClass::BootKeyboard,
        2 => InterfaceClass::BootMouse,
        3 => InterfaceClass::MassStorageBulkOnly,
        _ => return Err(Error::InvalidData),
    };
    Ok(Interface {
        id,
        device_id,
        number,
        generation,
        class,
        class_code: r.word()? as u8,
        subclass: r.word()? as u8,
        protocol: r.word()? as u8,
        speed: decode_speed(r.word()?)?,
        owner: nonzero_handle(r.word()?),
        channel: nonzero_handle(r.word()?),
        endpoints: {
            let mut endpoints = Vec::new();
            for _ in 0..r.count(8)? {
                endpoints.push(EndpointDescriptor {
                    address: r.word()? as u8,
                    attributes: r.word()? as u8,
                    max_packet_size: r.word()? as u16,
                    interval: r.word()? as u8,
                });
            }
            endpoints
        },
    })
}

fn decode_speed(value: u64) -> Result<UsbSpeed, Error> {
    match value {
        1 => Ok(UsbSpeed::Low),
        2 => Ok(UsbSpeed::Full),
        3 => Ok(UsbSpeed::High),
        4 => Ok(UsbSpeed::Super),
        _ => Err(Error::InvalidData),
    }
}

fn nonzero(handle: u64) -> Option<Channel> {
    (handle != 0).then_some(Channel(handle))
}

fn nonzero_handle(handle: u64) -> Option<u64> {
    (handle != 0).then_some(handle)
}
