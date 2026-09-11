use alloc::{vec, vec::Vec};
use bexos_d1_pci::{ECAM_SIZE, MAX_BUSES, RootPort};
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
}
pub struct Runtime {
    pub control: ControlState,
    pub registry: Channel,
    pub cursor: u64,
    pub next_poll_ms: u64,
    pub known: Vec<u64>,
    pub ports: Vec<RootPort>,
    pub pending: Option<Pending>,
    pub power: Vec<Channel>,
    pub extended: bool,
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: ControlState::empty(),
            registry: Channel(0),
            cursor: 0,
            next_poll_ms: 0,
            known: Vec::new(),
            ports: Vec::new(),
            pending: None,
            power: Vec::new(),
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
        w.word(if self.ports.is_empty() { 1 } else { 2 });
        w.word(self.registry.0);
        w.word(self.cursor);
        w.word(self.next_poll_ms);
        w.word(self.known.len() as u64);
        for node in &self.known {
            w.word(*node);
        }
        w.word(self.pending.is_some() as u64);
        if let Some(p) = &self.pending {
            w.word(p.node);
            w.word(p.added as u64);
        }
        w.word(self.power.len() as u64);
        for c in &self.power {
            w.word(c.0);
        }
        if !self.ports.is_empty() {
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
        if !(1..=2).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let registry = Channel(r.word()?);
        let cursor = r.word()?;
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
        let limits = bexos_d1_pci::RootBusConfig::qemu_virt();
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
            if ports.is_empty() {
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
        self.next_poll_ms = next_poll_ms;
        self.known = known;
        self.ports = ports;
        self.pending = pending;
        self.power = power;
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
