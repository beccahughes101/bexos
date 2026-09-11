use crate::hardware::{DmaAllocation, Hardware};
use alloc::{collections::BTreeMap, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_usb_host::{
    migration::QueueState,
    transfer::{BufferRegistration, TransferLimits},
    xhci::CapabilityRegisters,
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
    service_binding::BoundServiceEndpoint,
};
use usb_host_fidl::TransferDirection;

#[derive(Clone, Debug)]
pub struct RegisteredBuffer {
    pub info: BufferRegistration,
    pub handle: u64,
    pub device_address: u64,
    pub size_bytes: u64,
    pub direction: TransferDirection,
    pub in_flight: u32,
}

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub hardware: Option<Hardware>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub bus: Option<Channel>,
    pub bus_events: Option<Channel>,
    pub completions: Vec<Channel>,
    pub buffers: BTreeMap<u32, RegisteredBuffer>,
    pub next_buffer_id: u32,
    pub next_transfer_id: u64,
    pub command: QueueState,
    pub event: QueueState,
    pub queued_transfers: Vec<u64>,
    pub limits: TransferLimits,
}

impl State for Runtime {
    fn empty() -> Self {
        Self::new(Channel(0), None, None)
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
                let h = self.hardware.as_ref().ok_or(Error::BadState)?;
                for value in [
                    h.mmio_handle,
                    h.mmio,
                    h.mmio_size,
                    h.interrupt,
                    h.iommu_domain,
                    h.generation,
                ] {
                    w.word(value);
                }
                encode_dma(&mut w, &h.command_ring);
                encode_dma(&mut w, &h.event_ring);
                encode_dma(&mut w, &h.scratchpad_table);
                w.word(h.caps.caplength as u64);
                w.word(h.caps.max_slots as u64);
                w.word(h.caps.max_interrupters as u64);
                w.word(h.caps.max_ports as u64);
                w.word(h.caps.scratchpads as u64);
                w.word(h.caps.doorbell_offset as u64);
                w.word(h.caps.runtime_offset as u64);
                self.command.encode(&mut w);
                self.event.encode(&mut w);
            }
            1 => {
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
                w.word(self.bus.map_or(0, |c| c.0));
                w.word(self.bus_events.map_or(0, |c| c.0));
                w.word(self.completions.len() as u64);
                for channel in &self.completions {
                    w.word(channel.0);
                }
            }
            2 => {
                w.word(self.next_buffer_id as u64);
                w.word(self.next_transfer_id);
                w.word(self.buffers.len() as u64);
                for (id, buffer) in &self.buffers {
                    w.word(*id as u64);
                    w.word(buffer.handle);
                    w.word(buffer.device_address);
                    w.word(buffer.size_bytes);
                    w.word(buffer.direction as u64);
                    w.word(buffer.in_flight as u64);
                }
            }
            3 => {
                w.word(self.queued_transfers.len() as u64);
                for transfer in &self.queued_transfers {
                    w.word(*transfer);
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
                let mmio_handle = r.word()?;
                let mmio = r.word()?;
                let mmio_size = r.word()?;
                let interrupt = r.word()?;
                let iommu_domain = r.word()?;
                let generation = r.word()?;
                let command_ring = decode_dma(&mut r)?;
                let event_ring = decode_dma(&mut r)?;
                let scratchpad_table = decode_dma(&mut r)?;
                let caps = CapabilityRegisters {
                    caplength: r.word()? as u8,
                    max_slots: r.word()? as u8,
                    max_interrupters: r.word()? as u16,
                    max_ports: r.word()? as u8,
                    scratchpads: r.word()? as u16,
                    doorbell_offset: r.word()? as u32,
                    runtime_offset: r.word()? as u32,
                };
                self.hardware = Some(Hardware::adopt(
                    mmio_handle,
                    mmio,
                    mmio_size,
                    interrupt,
                    iommu_domain,
                    caps,
                    generation,
                    [command_ring, event_ring, scratchpad_table],
                ));
                self.command = QueueState::decode(&mut r)?;
                self.event = QueueState::decode(&mut r)?;
            }
            1 => {
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
                self.bus = nonzero(r.word()?);
                self.bus_events = nonzero(r.word()?);
                self.completions.clear();
                for _ in 0..r.count(64)? {
                    self.completions.push(Channel(r.word()?));
                }
            }
            2 => {
                self.next_buffer_id = r.word()? as u32;
                self.next_transfer_id = r.word()?;
                self.buffers.clear();
                for _ in 0..r.count(1024)? {
                    let id = r.word()? as u32;
                    let handle = r.word()?;
                    let device_address = r.word()?;
                    let size_bytes = r.word()?;
                    let direction = match r.word()? {
                        1 => TransferDirection::Out,
                        2 => TransferDirection::In,
                        _ => return Err(Error::InvalidData),
                    };
                    let in_flight = r.word()? as u32;
                    if self
                        .buffers
                        .insert(
                            id,
                            RegisteredBuffer {
                                info: BufferRegistration {
                                    id,
                                    size_bytes,
                                    direction,
                                },
                                handle,
                                device_address,
                                size_bytes,
                                direction,
                                in_flight,
                            },
                        )
                        .is_some()
                    {
                        return Err(Error::InvalidData);
                    }
                }
            }
            3 => {
                self.queued_transfers.clear();
                for _ in 0..r.count(4096)? {
                    self.queued_transfers.push(r.word()?);
                }
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || !self
                .hardware
                .as_ref()
                .is_some_and(|hardware| hardware.ready_to_migrate())
            || self
                .buffers
                .iter()
                .any(|(id, buffer)| *id != buffer.info.id || buffer.handle == 0)
        {
            Err(Error::BadState)
        } else {
            Ok(())
        }
    }

    fn quiescence_ready(&self) -> bool {
        self.buffers.values().all(|buffer| buffer.in_flight == 0)
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = self
            .hardware
            .as_ref()
            .map_or_else(Vec::new, Hardware::resources);
        out.push(Resource::Handle(self.control.0));
        if let Some(channel) = self.migration {
            out.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.bus {
            out.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.bus_events {
            out.push(Resource::Handle(channel.0));
        }
        out.extend(
            self.clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        out.extend(
            self.completions
                .iter()
                .map(|channel| Resource::Handle(channel.0)),
        );
        for buffer in self.buffers.values() {
            out.push(Resource::Handle(buffer.handle));
        }
        out
    }

    fn activation_markers(&self) -> [u64; 3] {
        let h = self.hardware.as_ref().unwrap();
        [
            h.command_ring.device_address(),
            h.event_ring.device_address(),
            h.scratchpad_table.device_address(),
        ]
    }

    fn activated(&mut self, generation: u64) {
        if let Some(hardware) = self.hardware.as_mut() {
            hardware.generation = generation;
            hardware.activate();
        }
        bexos_userspace::log(&alloc::format!("xhcid: adopted generation={generation}\n"));
    }
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>, hardware: Option<Hardware>) -> Self {
        Self {
            control,
            migration,
            hardware,
            clients: Vec::new(),
            bus: None,
            bus_events: None,
            completions: Vec::new(),
            buffers: BTreeMap::new(),
            next_buffer_id: 1,
            next_transfer_id: 1,
            command: QueueState {
                cycle: true,
                ..Default::default()
            },
            event: QueueState {
                cycle: true,
                ..Default::default()
            },
            queued_transfers: Vec::new(),
            limits: TransferLimits::default(),
        }
    }
}

fn nonzero(handle: u64) -> Option<Channel> {
    (handle != 0).then_some(Channel(handle))
}

fn encode_dma(w: &mut Encoder, allocation: &DmaAllocation) {
    for value in [
        allocation.handle(),
        allocation.va(),
        allocation.device_address(),
        allocation.token(),
        allocation.size(),
    ] {
        w.word(value);
    }
}

fn decode_dma(r: &mut Decoder<'_>) -> Result<DmaAllocation, Error> {
    DmaAllocation::adopt(r.word()?, r.word()?, r.word()?, r.word()?, r.word()?)
        .map_err(|_| Error::InvalidData)
}
