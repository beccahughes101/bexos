use crate::{block::BlockServer, transport::TransportState};
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

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub lifecycle: Option<Channel>,
    pub interface: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub fifos: Vec<Channel>,
    pub node: u64,
    pub transport: TransportState,
    pub block: BlockServer,
    pub queued: Vec<u64>,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>) -> Self {
        Self {
            control,
            migration,
            lifecycle: None,
            interface: None,
            clients: Vec::new(),
            fifos: Vec::new(),
            node: 0,
            transport: TransportState::default(),
            block: BlockServer::new(),
            queued: Vec::new(),
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
                w.word(1);
                for handle in [
                    self.control.0,
                    self.migration.map_or(0, |c| c.0),
                    self.lifecycle.map_or(0, |c| c.0),
                    self.interface.map_or(0, |c| c.0),
                    self.node,
                ] {
                    w.word(handle);
                }
                w.word(self.transport.lun as u64);
                w.word(self.transport.block_size as u64);
                w.word(self.transport.block_count);
                w.word(self.transport.tag as u64);
                w.word(self.transport.last_sense.key as u64);
                w.word(self.transport.last_sense.asc as u64);
                w.word(self.transport.last_sense.ascq as u64);
                w.word(self.transport.bulk_in_stalled as u64);
                w.word(self.transport.bulk_out_stalled as u64);
                w.word(self.transport.reset_count);
                w.word(self.transport.completed_write_watermark);
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
                w.word(self.fifos.len() as u64);
                for fifo in &self.fifos {
                    w.word(fifo.0);
                }
            }
            2 => {
                w.word(self.block.next_buffer_id as u64);
                w.word(self.block.in_flight as u64);
                w.word(self.block.buffers.len() as u64);
                for (id, buffer) in &self.block.buffers {
                    w.word(*id as u64);
                    w.word(buffer.handle);
                    w.word(buffer.mapped);
                    w.word(buffer.size_bytes);
                }
                w.word(self.queued.len() as u64);
                for request in &self.queued {
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
                self.lifecycle = nonzero(r.word()?);
                self.interface = nonzero(r.word()?);
                self.node = r.word()?;
                self.transport.lun = r.word()? as u8;
                self.transport.block_size = r.word()? as u32;
                self.transport.block_count = r.word()?;
                self.transport.tag = r.word()? as u32;
                self.transport.last_sense.key = r.word()? as u8;
                self.transport.last_sense.asc = r.word()? as u8;
                self.transport.last_sense.ascq = r.word()? as u8;
                self.transport.bulk_in_stalled = r.flag()?;
                self.transport.bulk_out_stalled = r.flag()?;
                self.transport.reset_count = r.word()?;
                self.transport.completed_write_watermark = r.word()?;
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
                self.fifos.clear();
                for _ in 0..r.count(64)? {
                    self.fifos.push(Channel(r.word()?));
                }
            }
            2 => {
                self.block.next_buffer_id = r.word()? as u32;
                self.block.in_flight = r.word()? as u32;
                self.block.buffers.clear();
                for _ in 0..r.count(1024)? {
                    let id = r.word()? as u32;
                    self.block.buffers.insert(
                        id,
                        crate::block::Buffer {
                            handle: r.word()?,
                            mapped: r.word()?,
                            size_bytes: r.word()?,
                        },
                    );
                }
                self.queued.clear();
                for _ in 0..r.count(4096)? {
                    self.queued.push(r.word()?);
                }
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || self.transport.block_size == 0
            || self.block.buffers.values().any(|buffer| buffer.handle == 0)
        {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn quiescence_ready(&self) -> bool {
        self.block.in_flight == 0
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        for channel in [self.migration, self.lifecycle, self.interface]
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
        out.extend(self.fifos.iter().map(|fifo| Resource::Handle(fifo.0)));
        for buffer in self.block.buffers.values() {
            out.push(Resource::Handle(buffer.handle));
            out.push(Resource::Mapping {
                handle: buffer.handle,
                offset: 0,
                va: buffer.mapped,
                size: buffer.size_bytes,
                rights: 6,
            });
        }
        out
    }

    fn activated(&mut self, generation: u64) {
        bexos_userspace::log(&alloc::format!(
            "usb-bot: adopted generation={generation}\n"
        ));
    }
}

fn nonzero(handle: u64) -> Option<Channel> {
    (handle != 0).then_some(Channel(handle))
}
