use alloc::{vec, vec::Vec};
use bexos_d1_pci::{ECAM_SIZE, MAX_BUSES, RootBusConfig, RootPort};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
    service_control::ControlState,
};
pub struct Pending {
    pub node: u64,
    pub added: bool,
    pub control: Option<Channel>,
    pub interrupt: Option<u64>,
}
pub struct DeviceControl {
    pub node: u64,
    pub channel: Channel,
    pub interrupt: u64,
}
pub struct Runtime {
    pub control: ControlState,
    pub registry: Channel,
    pub cursor: u64,
    pub root: RootBusConfig,
    pub next_poll_ms: u64,
    pub known: Vec<u64>,
    pub ports: Vec<RootPort>,
    pub pending: Option<Pending>,
    pub power: Vec<Channel>,
    pub device_controls: Vec<DeviceControl>,
    pub extended: bool,
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: ControlState::empty(),
            registry: Channel(0),
            cursor: 0,
            root: RootBusConfig::qemu_virt(),
            next_poll_ms: 0,
            known: Vec::new(),
            ports: Vec::new(),
            pending: None,
            power: Vec::new(),
            device_controls: Vec::new(),
            extended: false,
        }
    }
    fn keys(&self) -> Vec<u64> {
        if self.extended { vec![0, 1] } else { vec![0] }
    }
    fn quiescence_ready(&self) -> bool {
        self.pending.is_none()
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key == 0 {
            return self.control.encode_record(0);
        }
        if key != 1 || !self.extended {
            return Ok(None);
        }
        let mut w = Encoder::new();
        w.word(5);
        w.word(self.registry.0);
        w.word(self.cursor);
        w.word(self.root.mmio_base);
        w.word(self.root.mmio_limit);
        w.word(self.next_poll_ms);
        w.word(self.known.len() as u64);
        for node in &self.known {
            w.word(*node);
        }
        w.word(self.pending.is_some() as u64);
        if let Some(p) = &self.pending {
            w.word(p.node);
            w.word(p.added as u64);
            w.word(p.control.as_ref().map_or(0, |channel| channel.0));
            w.word(p.interrupt.unwrap_or(0));
        }
        w.word(self.power.len() as u64);
        for c in &self.power {
            w.word(c.0);
        }
        w.word(self.device_controls.len() as u64);
        for control in &self.device_controls {
            w.word(control.node);
            w.word(control.channel.0);
            w.word(control.interrupt);
        }
        w.word(self.ports.len() as u64);
        for port in &self.ports {
            for value in [
                bexos_d1_pci::node_id(port.address),
                port.secondary as u64,
                port.base,
                port.limit,
                port.cursor,
            ] {
                w.word(value);
            }
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key == 0 {
            return self.control.adopt_record(0, bytes);
        }
        if key != 1 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = r.word()?;
        if !(1..=5).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let registry = Channel(r.word()?);
        let cursor = r.word()?;
        let root = if version >= 4 {
            RootBusConfig::new(r.word()?, r.word()?).map_err(|_| Error::InvalidData)?
        } else {
            RootBusConfig::qemu_virt()
        };
        let next_poll_ms = r.word()?;
        let mut known = Vec::new();
        for _ in 0..r.count(16)? {
            let node = r.word()?;
            if !valid_node(node) || known.contains(&node) {
                return Err(Error::InvalidData);
            }
            known.push(node);
        }
        let pending = if r.flag()? {
            Some(Pending {
                node: r.word()?,
                added: r.flag()?,
                control: if version >= 3 {
                    let handle = r.word()?;
                    (handle != 0).then_some(Channel(handle))
                } else {
                    None
                },
                interrupt: if version >= 5 {
                    let handle = r.word()?;
                    (handle != 0).then_some(handle)
                } else {
                    None
                },
            })
        } else {
            None
        };
        let mut power = Vec::new();
        for _ in 0..r.count(8)? {
            let c = Channel(r.word()?);
            if c.0 == 0 || power.iter().any(|p: &Channel| p.0 == c.0) {
                return Err(Error::InvalidData);
            }
            power.push(c);
        }
        let mut device_controls = Vec::new();
        if version >= 3 {
            for _ in 0..r.count(256)? {
                let node = r.word()?;
                let channel = Channel(r.word()?);
                let interrupt = if version >= 5 { r.word()? } else { 0 };
                if !valid_node(node)
                    || channel.0 == 0
                    || (version >= 5 && interrupt == 0)
                    || device_controls.iter().any(|control: &DeviceControl| {
                        control.node == node
                            || control.channel.0 == channel.0
                            || (interrupt != 0 && control.interrupt == interrupt)
                    })
                {
                    return Err(Error::InvalidData);
                }
                device_controls.push(DeviceControl {
                    node,
                    channel,
                    interrupt,
                });
            }
        }
        let limits = root;
        let mut ports: Vec<RootPort> = Vec::new();
        if version >= 2 {
            for _ in 0..r.count(MAX_BUSES as usize - 1)? {
                let node = r.word()?;
                if node > 0x1f07 || !valid_node(node) {
                    return Err(Error::InvalidData);
                }
                let port = RootPort {
                    address: bexos_d1_pci::address_from_node(node).ok_or(Error::InvalidData)?,
                    secondary: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
                    base: r.word()?,
                    limit: r.word()?,
                    cursor: r.word()?,
                };
                port.validate(limits).map_err(|_| Error::InvalidData)?;
                if port.limit > cursor
                    || ports.iter().any(|p| {
                        p.address == port.address
                            || p.secondary == port.secondary
                            || (p.base < port.limit && port.base < p.limit)
                    })
                {
                    return Err(Error::InvalidData);
                }
                ports.push(port);
            }
            if version < 4 && ports.is_empty() {
                return Err(Error::InvalidData);
            }
        }
        r.finish()?;
        if (registry.0 != 0 && (cursor < limits.mmio_base || cursor > limits.mmio_limit))
            || (registry.0 == 0 && (cursor != 0 || !known.is_empty() || pending.is_some()))
        {
            return Err(Error::InvalidData);
        }
        if pending
            .as_ref()
            .is_some_and(|p| !valid_node(p.node) || p.added == known.contains(&p.node))
        {
            return Err(Error::InvalidData);
        }
        self.registry = registry;
        self.cursor = cursor;
        self.root = root;
        self.next_poll_ms = next_poll_ms;
        self.known = known;
        self.ports = ports;
        self.pending = pending;
        self.power = power;
        self.device_controls = device_controls;
        self.extended = true;
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        self.control.validate()?;
        if self.registry.0 != 0
            && self
                .control
                .mapping
                .is_none_or(|(_, _, size)| size != 0x10_0000 && size != ECAM_SIZE)
        {
            return Err(Error::InvalidData);
        }
        if !self.ports.is_empty()
            && self
                .control
                .mapping
                .is_none_or(|(_, _, size)| size != ECAM_SIZE)
        {
            return Err(Error::InvalidData);
        }
        if self
            .known
            .iter()
            .chain(self.pending.iter().map(|p| &p.node))
            .any(|node| {
                *node >> 16 != 0 && !self.ports.iter().any(|p| p.secondary as u64 == *node >> 16)
            })
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut resources = self.control.resources();
        if self.registry.0 != 0 {
            resources.push(Resource::Handle(self.registry.0));
        }
        resources.extend(self.power.iter().map(|c| Resource::Handle(c.0)));
        resources.extend(
            self.device_controls
                .iter()
                .flat_map(|control| {
                    [control.channel.0, control.interrupt]
                        .into_iter()
                        .filter(|handle| *handle != 0)
                        .map(Resource::Handle)
                }),
        );
        if let Some(control) = self.pending.as_ref().and_then(|pending| pending.control) {
            resources.push(Resource::Handle(control.0));
        }
        if let Some(interrupt) = self.pending.as_ref().and_then(|pending| pending.interrupt) {
            resources.push(Resource::Handle(interrupt));
        }
        resources
    }
    fn activated(&mut self, generation: u64) {
        self.control.activated(generation);
        self.extended = true;
        bexos_userspace::log("pci: allocator, discovery and registry endpoint adopted\n");
    }
}

fn valid_node(node: u64) -> bool {
    node >> 16 < MAX_BUSES as u64 && node & 0xffff <= 0x1f07 && node & 0xff <= 7
}
