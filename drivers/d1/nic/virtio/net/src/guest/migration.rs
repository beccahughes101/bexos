use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bexos_virtio_hal::migration::*;

use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;

use crate::guest::page_round;
use crate::hal::{self, DmaAllocation, MmioMapping, SharedRange};
use crate::hardware::Hardware;
use crate::server::EthernetServer;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub hardware: Option<Hardware>,
    pub server: Option<EthernetServer>,
    pub fifos: Vec<Channel>,
    pub pending: crate::pending::PendingFrames,
    pub domain: u64,
    pub device_endpoints: Vec<BoundServiceEndpoint>,
    pub power_endpoints: Vec<BoundServiceEndpoint>,
    pub lifecycle: Option<Channel>,
    hardware_checkpoint: Option<Vec<u8>>,
    dma: Vec<DmaAllocation>,
    shared_ranges: Vec<SharedRange>,
    mmio: Vec<MmioMapping>,
}

impl Runtime {
    pub fn new(
        control: Channel,
        migration: Option<Channel>,
        hardware: Hardware,
        server: EthernetServer,
    ) -> Self {
        Self {
            control,
            migration,
            hardware: Some(hardware),
            server: Some(server),
            fifos: Vec::new(),
            pending: Default::default(),
            domain: hal::iommu_domain_handle().unwrap_or(0),
            device_endpoints: Vec::new(),
            power_endpoints: Vec::new(),
            lifecycle: None,
            hardware_checkpoint: None,
            dma: Vec::new(),
            shared_ranges: Vec::new(),
            mmio: Vec::new(),
        }
    }

    pub fn empty_for_test() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            hardware: None,
            server: None,
            fifos: Vec::new(),
            pending: Default::default(),
            domain: 0,
            device_endpoints: Vec::new(),
            power_endpoints: Vec::new(),
            lifecycle: None,
            hardware_checkpoint: None,
            dma: Vec::new(),
            shared_ranges: Vec::new(),
            mmio: Vec::new(),
        }
    }

    pub fn set_snapshots_for_test(
        &mut self,
        hardware_checkpoint: Vec<u8>,
        dma: Vec<DmaAllocation>,
        shared_ranges: Vec<SharedRange>,
        mmio: Vec<MmioMapping>,
    ) {
        self.hardware_checkpoint = Some(hardware_checkpoint);
        self.dma = dma;
        self.shared_ranges = shared_ranges;
        self.mmio = mmio;
    }

    pub fn drain_for_quiesce(&mut self) -> bool {
        self.hardware
            .as_ref()
            .is_some_and(Hardware::ready_to_migrate)
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self::empty_for_test()
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2, 3]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            0 => {
                w.word(3);
                w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
                w.word(self.domain);
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |channel| channel.0));
                w.word(self.lifecycle.map_or(0, |channel| channel.0));
                w.word(self.device_endpoints.len() as u64);
                for endpoint in &self.device_endpoints {
                    encode_endpoint(&mut w, endpoint);
                }
                w.word(self.power_endpoints.len() as u64);
                for endpoint in &self.power_endpoints {
                    encode_endpoint(&mut w, endpoint);
                }
                w.word(self.fifos.len() as u64);
                for fifo in &self.fifos {
                    w.word(fifo.0);
                }
                self.pending.encode(&mut w)?;
            }
            1 => w.bytes(&self.server.as_ref().ok_or(Error::BadState)?.checkpoint()),
            2 => {
                let checkpoint = self
                    .hardware
                    .as_ref()
                    .map(Hardware::checkpoint)
                    .or_else(|| self.hardware_checkpoint.clone())
                    .ok_or(Error::BadState)?;
                w.bytes(&checkpoint);
            }
            3 => {
                let dma = if self.hardware.is_some() {
                    hal::dma_snapshot()
                } else {
                    self.dma.clone()
                };
                let shared_ranges = if self.hardware.is_some() {
                    hal::shared_range_snapshot()
                } else {
                    self.shared_ranges.clone()
                };
                let mmio = if self.hardware.is_some() {
                    hal::mmio_snapshot()
                } else {
                    self.mmio.clone()
                };
                encode_dma(&mut w, &dma);
                encode_shared(&mut w, &shared_ranges);
                encode_mmio(&mut w, &mmio);
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        match key {
            0 => {
                let version = r.word()?;
                if version != 2 && version != 3 {
                    return Err(Error::UnsupportedVersion);
                }
                let architecture = if version == 2 { 1 } else { r.word()? };
                if architecture != if cfg!(bexos_arch_x86_64) { 2 } else { 1 } {
                    return Err(Error::InvalidData);
                }
                self.domain = if version == 3 { r.word()? } else { 0 };
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                let lifecycle = r.word()?;
                self.lifecycle = (lifecycle != 0).then_some(Channel(lifecycle));
                self.device_endpoints.clear();
                for _ in 0..r.count(1024)? {
                    self.device_endpoints.push(decode_endpoint(&mut r)?);
                }
                self.power_endpoints.clear();
                for _ in 0..r.count(1024)? {
                    self.power_endpoints.push(decode_endpoint(&mut r)?);
                }
                self.fifos.clear();
                for _ in 0..r.count(1024)? {
                    self.fifos.push(Channel(r.word()?));
                }
                self.pending = if version == 3 {
                    crate::pending::PendingFrames::decode(&mut r)?
                } else {
                    Default::default()
                };
            }
            1 => {
                self.server = Some(EthernetServer::adopt(r.bytes(32704)?)?);
            }
            2 => {
                self.hardware_checkpoint = Some(r.bytes(65536)?.to_vec());
            }
            3 => {
                self.dma = decode_dma(&mut r)?;
                self.shared_ranges = decode_shared(&mut r)?;
                self.mmio = decode_mmio(&mut r)?;
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }

    fn finish_adoption(&mut self) -> Result<(), Error> {
        self.validate()?;
        hal::set_iommu_domain(self.domain);
        if !hal::adopt_dma_snapshot(&self.dma)
            || !hal::adopt_shared_range_snapshot(&self.shared_ranges)
            || !hal::adopt_mmio_snapshot(&self.mmio)
        {
            return Err(Error::InvalidData);
        }
        let checkpoint = self.hardware_checkpoint.take().ok_or(Error::BadState)?;
        self.hardware = Some(Hardware::adopt(&checkpoint)?);
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || !self
                .server
                .as_ref()
                .is_some_and(EthernetServer::registrations_match)
        {
            return Err(Error::InvalidData);
        }
        if self.hardware.is_none() && self.hardware_checkpoint.is_none() {
            return Err(Error::InvalidData);
        }
        for (fifo, entry) in self
            .pending
            .rx
            .iter()
            .chain(self.pending.tx.iter())
            .chain(self.pending.transmitting.iter())
        {
            if !self.fifos.iter().any(|known| known.0 == fifo.0)
                || !self
                    .server
                    .as_ref()
                    .unwrap()
                    .buffers()
                    .get(&entry.vmo_id)
                    .is_some_and(|buffer| {
                        (entry.offset as u64)
                            .checked_add(entry.length as u64)
                            .is_some_and(|end| end <= buffer.size as u64)
                    })
            {
                return Err(Error::InvalidData);
            }
        }
        let mut buffers = BTreeMap::new();
        for buffer in self.server.as_ref().unwrap().buffers().values() {
            if buffer.handle == 0
                || buffer.vaddr == 0
                || buffer.paddr == 0
                || buffer.token == 0
                || page_round(buffer.size as u64).is_none()
                || buffers.insert(buffer.handle, ()).is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out: Vec<_> = core::iter::once(self.control.0)
            .chain(self.migration.map(|channel| channel.0))
            .chain(self.lifecycle.map(|channel| channel.0))
            .chain(
                self.device_endpoints
                    .iter()
                    .map(|endpoint| endpoint.channel.0),
            )
            .chain(
                self.power_endpoints
                    .iter()
                    .map(|endpoint| endpoint.channel.0),
            )
            .chain(self.fifos.iter().map(|channel| channel.0))
            .map(Resource::Handle)
            .collect();
        if let Some(hardware) = &self.hardware {
            out.extend(hardware.resources());
        }
        if self.domain != 0 {
            out.push(Resource::Handle(self.domain));
        }
        let dma = if self.hardware.is_some() {
            hal::dma_snapshot()
        } else {
            self.dma.clone()
        };
        let shared_ranges = if self.hardware.is_some() {
            hal::shared_range_snapshot()
        } else {
            self.shared_ranges.clone()
        };
        let mmio = if self.hardware.is_some() {
            hal::mmio_snapshot()
        } else {
            self.mmio.clone()
        };
        for allocation in &dma {
            out.push(Resource::Handle(allocation.handle));
            out.push(Resource::Mapping {
                handle: allocation.handle,
                offset: 0,
                va: allocation.vaddr,
                size: allocation.size,
                rights: 6,
            });
            out.push(Resource::Pin(allocation.token));
        }
        for range in &shared_ranges {
            out.push(Resource::Handle(range.vmo));
            out.push(Resource::Mapping {
                handle: range.vmo,
                offset: 0,
                va: range.vaddr,
                size: range.size,
                rights: 6,
            });
            out.push(Resource::Pin(range.token));
        }
        for mapping in &mmio {
            out.push(Resource::Handle(mapping.handle));
            out.push(Resource::Mapping {
                handle: mapping.handle,
                offset: 0,
                va: mapping.vaddr,
                size: mapping.size,
                rights: 6,
            });
        }
        out
    }

    fn activation_markers(&self) -> [u64; 3] {
        let Some(first) = self.dma.first() else {
            return [0; 3];
        };
        [
            first.paddr,
            self.dma.get(1).map_or(0, |d| d.paddr),
            self.mmio.first().map_or(0, |m| m.paddr),
        ]
    }

    fn activated(&mut self, generation: u64) {
        hal::activate_adopted();
        if let Some(hardware) = &mut self.hardware {
            hardware.activate();
        }
        bexos_userspace::log(&alloc::format!(
            "virtio-net: adopted generation={generation}; nic queues retained without reset\n"
        ));
    }
}

fn encode_endpoint(w: &mut Encoder, endpoint: &BoundServiceEndpoint) {
    w.word(endpoint.channel.0);
    w.word(endpoint.allowed_methods.len() as u64);
    for ordinal in &endpoint.allowed_methods {
        w.word(*ordinal);
    }
}

fn decode_endpoint(r: &mut Decoder<'_>) -> Result<BoundServiceEndpoint, Error> {
    let channel = Channel(r.word()?);
    let mut allowed_methods = Vec::new();
    for _ in 0..r.count(64)? {
        allowed_methods.push(r.word()?);
    }
    Ok(BoundServiceEndpoint::new(channel, allowed_methods))
}
