//! Logical Venus ownership and retained shared memory; no renderer pointers.
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_virtio_gpu_protocol::aperture::Aperture;
use bexos_virtio_gpu_protocol::transport::Registry;
use bexos_virtio_hal::{DmaAllocation, buffer::DmaBuffer};

pub struct HostMemory {
    pub id: u32,
    pub visible: bool,
    pub offset: u64,
    pub handle: u64,
    pub mapped: bool,
    pub retiring: bool,
    pub owned: bool,
}
impl Drop for HostMemory {
    fn drop(&mut self) {
        if self.owned && self.handle != 0 {
            let _ = bexos_userspace::Memory::close(self.handle);
        }
    }
}

pub struct SharedMemory {
    pub id: u32,
    pub buffer: Option<DmaBuffer>,
    pub saved: [u64; 3],
    pub retiring: bool,
}
impl SharedMemory {
    pub fn snapshot(&self) -> [u64; 3] {
        self.buffer.as_ref().map_or(self.saved, DmaBuffer::snapshot)
    }
}
#[derive(Default)]
pub struct GpuState {
    pub registry: Registry,
    pub memory: Vec<SharedMemory>,
    pub aperture: Aperture,
    pub host: Vec<HostMemory>,
}
impl GpuState {
    pub fn encode(&self, w: &mut Encoder) {
        self.encode_version(w, false);
    }
    pub fn encode_version(&self, w: &mut Encoder, v4: bool) {
        self.encode_legacy(w);
        w.word(self.aperture.base);
        w.word(self.aperture.size);
        w.word(self.host.len() as u64);
        for m in &self.host {
            w.word(m.id as u64);
            w.word(m.offset);
            w.word(m.handle);
            w.word(m.mapped as u64);
            w.word(m.retiring as u64);
            if !v4 {
                w.word(m.visible as u64);
            }
        }
    }
    pub fn encode_legacy(&self, w: &mut Encoder) {
        self.registry.encode(w);
        w.word(self.memory.len() as u64);
        for m in &self.memory {
            w.word(m.id as u64);
            for value in m.snapshot() {
                w.word(value);
            }
            w.word(m.retiring as u64);
        }
    }
    pub fn decode(r: &mut Decoder<'_>, dma: &[DmaAllocation]) -> Result<Self, Error> {
        Self::decode_version(r, dma, 5)
    }
    pub fn decode_version(
        r: &mut Decoder<'_>,
        dma: &[DmaAllocation],
        version: u8,
    ) -> Result<Self, Error> {
        if !(3..=5).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let registry = Registry::decode(r)?;
        let mut memory = Vec::new();
        for _ in 0..r.count(bexos_virtio_gpu_protocol::transport::MAX_RESOURCES)? {
            let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let saved = [r.word()?, r.word()?, r.word()?];
            let retiring = r.flag()?;
            if !registry
                .resources
                .iter()
                .any(|v| v.id == id && v.size / 4096 == saved[2])
                || memory.iter().any(|v: &SharedMemory| {
                    v.id == id || v.saved[0] == saved[0] || v.saved[1] == saved[1]
                })
                || !dma.iter().any(|d| {
                    d.paddr == saved[0] && d.vaddr == saved[1] && d.size / 4096 == saved[2]
                })
            {
                return Err(Error::InvalidData);
            }
            memory.push(SharedMemory {
                id,
                buffer: None,
                saved,
                retiring,
            });
        }
        let mut aperture = Aperture::default();
        let mut host = Vec::new();
        if version >= 4 {
            let base = r.word()?;
            let size = r.word()?;
            if base != 0 || size != 0 {
                aperture = Aperture::new(base, size, 0, size).map_err(|_| Error::InvalidData)?;
            }
            let mut spans = [(0, 0); 64];
            for _ in 0..r.count(64)? {
                let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let offset = r.word()?;
                let handle = r.word()?;
                let mapped = r.flag()?;
                let retiring = r.flag()?;
                let visible = if version >= 5 { r.flag()? } else { true };
                let resource = registry
                    .resources
                    .iter()
                    .find(|v| v.id == id)
                    .ok_or(Error::InvalidData)?;
                if memory.iter().any(|v| v.id == id)
                    || host
                        .iter()
                        .any(|v: &HostMemory| v.id == id || (handle != 0 && v.handle == handle))
                    || offset % 4096 != 0
                    || (visible
                        && offset
                            .checked_add(resource.size)
                            .is_none_or(|end| end > aperture.size))
                    || (!visible && (offset != 0 || handle != 0 || mapped))
                    || (handle != 0 && !mapped)
                {
                    return Err(Error::InvalidData);
                }
                for &(start, size) in &spans[..host.len()] {
                    if visible
                        && size != 0
                        && offset < start + size
                        && start < offset + resource.size
                    {
                        return Err(Error::InvalidData);
                    }
                }
                spans[host.len()] = (offset, if visible { resource.size } else { 0 });
                host.push(HostMemory {
                    id,
                    visible,
                    offset,
                    handle,
                    mapped,
                    retiring,
                    owned: false,
                });
            }
        }
        if memory.len() + host.len() != registry.resources.len() {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            registry,
            memory,
            aperture,
            host,
        })
    }
    pub fn activate(&mut self) {
        for m in &mut self.host {
            m.owned = true;
        }
        for m in &mut self.memory {
            if m.buffer.is_none() {
                let mut buffer = DmaBuffer::adopt(m.saved).expect("retained Venus shared memory");
                buffer.activate();
                m.buffer = Some(buffer);
            }
        }
    }
}
