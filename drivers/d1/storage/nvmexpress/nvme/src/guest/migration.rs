use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};
pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub mmio_handle: u64,
    pub mmio: u64,
    pub mmio_size: u64,
    pub hardware: Option<Hardware>,
    pub server: Option<BlockDeviceServer>,
    pub buffers: BTreeMap<u32, (u64, u64)>,
    pub fifos: Vec<Channel>,
    pub block_endpoints: Vec<BoundServiceEndpoint>,
    pub lifecycle: Option<Channel>,
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            mmio_handle: 0,
            mmio: 0,
            mmio_size: 0,
            hardware: None,
            server: None,
            buffers: BTreeMap::new(),
            fifos: Vec::new(),
            block_endpoints: Vec::new(),
            lifecycle: None,
        }
    }
    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2, 3]
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            0 => {
                w.word(1);
                for n in [
                    self.control.0,
                    self.migration.map_or(0, |c| c.0),
                    self.mmio_handle,
                    self.mmio,
                    self.mmio_size,
                    self.lifecycle.map_or(0, |c| c.0),
                ] {
                    w.word(n);
                }
                w.bytes(&self.hardware.as_ref().ok_or(Error::BadState)?.checkpoint());
            }
            1 => {
                w.bytes(&self.server.as_ref().ok_or(Error::BadState)?.checkpoint());
                w.word(self.buffers.len() as u64);
                for (id, (h, va)) in &self.buffers {
                    w.word(*id as u64);
                    w.word(*h);
                    w.word(*va);
                }
            }
            2 => {
                w.word(self.fifos.len() as u64);
                for c in &self.fifos {
                    w.word(c.0);
                }
            }
            3 => {
                w.word(self.block_endpoints.len() as u64);
                for endpoint in &self.block_endpoints {
                    w.word(endpoint.channel.0);
                    w.word(endpoint.allowed_methods.len() as u64);
                    for ordinal in &endpoint.allowed_methods {
                        w.word(*ordinal);
                    }
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
                self.mmio_handle = r.word()?;
                self.mmio = r.word()?;
                self.mmio_size = r.word()?;
                let lifecycle = r.word()?;
                self.lifecycle = (lifecycle != 0).then_some(Channel(lifecycle));
                self.hardware = Some(Hardware::adopt(r.bytes(4096)?)?);
            }
            1 => {
                self.server = Some(BlockDeviceServer::adopt(
                    r.bytes(32704)?,
                    self.hardware.as_ref().ok_or(Error::BadState)?.info,
                )?);
                self.buffers.clear();
                for _ in 0..r.count(1024)? {
                    let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                    if self.buffers.insert(id, (r.word()?, r.word()?)).is_some() {
                        return Err(Error::InvalidData);
                    }
                }
            }
            2 => {
                self.fifos.clear();
                for _ in 0..r.count(1024)? {
                    self.fifos.push(Channel(r.word()?));
                }
            }
            3 => {
                self.block_endpoints.clear();
                for _ in 0..r.count(1024)? {
                    let channel = Channel(r.word()?);
                    let mut allowed = Vec::new();
                    for _ in 0..r.count(16)? {
                        allowed.push(r.word()?);
                    }
                    self.block_endpoints
                        .push(BoundServiceEndpoint::new(channel, allowed));
                }
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || !self.hardware.as_ref().is_some_and(|h| h.ready_to_migrate())
            || !self
                .server
                .as_ref()
                .is_some_and(|s| s.registrations_match(&self.buffers))
        {
            return Err(Error::BadState);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out = self.hardware.as_ref().unwrap().resources();
        out.push(Resource::Handle(self.control.0));
        if let Some(c) = self.migration {
            out.push(Resource::Handle(c.0));
        }
        if let Some(c) = self.lifecycle {
            out.push(Resource::Handle(c.0));
        }
        out.push(Resource::Mapping {
            handle: self.mmio_handle,
            offset: 0,
            va: self.mmio,
            size: self.mmio_size,
            rights: 6,
        });
        for (h, va) in self.buffers.values() {
            out.push(Resource::Mapping {
                handle: *h,
                offset: 0,
                va: *va,
                size: REGISTERED_BUFFER_BYTES,
                rights: 6,
            });
        }
        out.extend(self.fifos.iter().map(|c| Resource::Handle(c.0)));
        out.extend(
            self.block_endpoints
                .iter()
                .map(|endpoint| Resource::Handle(endpoint.channel.0)),
        );
        out
    }
    fn activation_markers(&self) -> [u64; 3] {
        let (a, b, c) = self.hardware.as_ref().unwrap().queue_addresses();
        [a, b, c]
    }
    fn activated(&mut self, generation: u64) {
        self.hardware.as_mut().unwrap().activate();
        log(&alloc::format!("nvme: adopted generation={generation}\n"));
    }
}
