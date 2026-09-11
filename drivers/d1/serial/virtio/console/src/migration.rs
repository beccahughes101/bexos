use crate::hardware::Hardware;
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use bexos_virtio_hal::{self as hal, DmaAllocation, MmioMapping, SharedRange, migration::*};

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub lifecycle: Option<Channel>,
    pub clients: Vec<Channel>,
    pub hardware: Option<Hardware>,
    checkpoint: Option<Vec<u8>>,
    domain: u64,
    dma: Vec<DmaAllocation>,
    shared: Vec<SharedRange>,
    mmio: Vec<MmioMapping>,
}
impl Runtime {
    pub fn new(
        control: Channel,
        migration: Option<Channel>,
        lifecycle: Option<Channel>,
        hardware: Hardware,
    ) -> Self {
        Self {
            control,
            migration,
            lifecycle,
            hardware: Some(hardware),
            domain: hal::iommu_domain_handle().unwrap_or(0),
            ..Self::empty()
        }
    }
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            lifecycle: None,
            clients: Vec::new(),
            hardware: None,
            checkpoint: None,
            domain: 0,
            dma: Vec::new(),
            shared: Vec::new(),
            mmio: Vec::new(),
        }
    }
    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2]
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            0 => {
                w.word(2);
                w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
                w.word(self.domain);
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |c| c.0));
                w.word(self.lifecycle.map_or(0, |c| c.0));
                w.word(self.clients.len() as u64);
                for c in &self.clients {
                    w.word(c.0);
                }
            }
            1 => {
                let hardware = self.hardware.as_ref().ok_or(Error::BadState)?;
                if !hardware.ready_to_migrate() {
                    return Err(Error::BadState);
                }
                w.bytes(&hardware.checkpoint());
            }
            2 => {
                encode_dma(&mut w, &hal::dma_snapshot());
                encode_shared(&mut w, &hal::shared_range_snapshot());
                encode_mmio(&mut w, &hal::mmio_snapshot());
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
                let architecture = if version == 1 {
                    1
                } else if version == 2 {
                    r.word()?
                } else {
                    return Err(Error::UnsupportedVersion);
                };
                if architecture != if cfg!(bexos_arch_x86_64) { 2 } else { 1 } {
                    return Err(Error::UnsupportedVersion);
                }
                self.domain = r.word()?;
                self.control = Channel(r.word()?);
                let m = r.word()?;
                self.migration = (m != 0).then_some(Channel(m));
                let l = r.word()?;
                self.lifecycle = (l != 0).then_some(Channel(l));
                self.clients.clear();
                for _ in 0..r.count(64)? {
                    self.clients.push(Channel(r.word()?));
                }
            }
            1 => self.checkpoint = Some(r.bytes(65536)?.to_vec()),
            2 => {
                self.dma = decode_dma(&mut r)?;
                self.shared = decode_shared(&mut r)?;
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
            || !hal::adopt_shared_range_snapshot(&self.shared)
            || !hal::adopt_mmio_snapshot(&self.mmio)
        {
            return Err(Error::InvalidData);
        }
        self.hardware = Some(Hardware::adopt(
            self.checkpoint.as_deref().ok_or(Error::BadState)?,
        )?);
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        if self.domain == 0
            || self.control.0 == 0
            || self.migration.is_none()
            || (self.hardware.is_none() && self.checkpoint.is_none())
            || self.clients.iter().any(|c| c.0 == 0)
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out: Vec<_> = core::iter::once(self.domain)
            .chain(core::iter::once(self.control.0))
            .chain(self.migration.map(|c| c.0))
            .chain(self.lifecycle.map(|c| c.0))
            .chain(self.clients.iter().map(|c| c.0))
            .map(Resource::Handle)
            .collect();
        if let Some(hardware) = &self.hardware {
            out.extend(hardware.resources());
        }
        for d in hal::dma_snapshot() {
            out.push(Resource::Handle(d.handle));
            out.push(Resource::Mapping {
                handle: d.handle,
                offset: 0,
                va: d.vaddr,
                size: d.size,
                rights: 6,
            });
            out.push(Resource::Pin(d.token));
        }
        for d in hal::shared_range_snapshot() {
            out.push(Resource::Handle(d.vmo));
            out.push(Resource::Mapping {
                handle: d.vmo,
                offset: 0,
                va: d.vaddr,
                size: d.size,
                rights: 6,
            });
            out.push(Resource::Pin(d.token));
        }
        for d in hal::mmio_snapshot() {
            out.push(Resource::Handle(d.handle));
            out.push(Resource::Mapping {
                handle: d.handle,
                offset: 0,
                va: d.vaddr,
                size: d.size,
                rights: 6,
            });
        }
        out
    }
    fn activated(&mut self, generation: u64) {
        hal::activate_adopted();
        if let Some(hardware) = &mut self.hardware {
            hardware.activate();
        }
        bexos_userspace::log(&alloc::format!(
            "virtio-console: adopted generation={generation}; serial queues and DMA retained\n"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_record_preserves_domain_and_private_clients() {
        let mut source = Runtime::empty();
        source.domain = 17;
        source.control = Channel(18);
        source.migration = Some(Channel(19));
        source.lifecycle = Some(Channel(20));
        source.clients = alloc::vec![Channel(21), Channel(22)];
        let record = source.encode_record(0).unwrap().unwrap();
        let mut target = Runtime::empty();
        target.adopt_record(0, Some(&record)).unwrap();
        assert_eq!(target.domain, 17);
        assert_eq!(target.control.0, 18);
        assert_eq!(target.migration.unwrap().0, 19);
        assert_eq!(target.lifecycle.unwrap().0, 20);
        assert_eq!(
            target.clients.iter().map(|c| c.0).collect::<Vec<_>>(),
            [21, 22]
        );
        for end in 0..record.len() {
            assert!(
                Runtime::empty()
                    .adopt_record(0, Some(&record[..end]))
                    .is_err()
            );
        }
        let mut trailing = record.clone();
        trailing.push(0);
        assert!(Runtime::empty().adopt_record(0, Some(&trailing)).is_err());
        let mut foreign = record.clone();
        foreign[8..16]
            .copy_from_slice(&(if cfg!(bexos_arch_x86_64) { 1u64 } else { 2 }).to_le_bytes());
        assert!(Runtime::empty().adopt_record(0, Some(&foreign)).is_err());
        let mut legacy = record.clone();
        legacy[..8].copy_from_slice(&1u64.to_le_bytes());
        legacy.drain(8..16);
        assert_eq!(
            Runtime::empty().adopt_record(0, Some(&legacy)).is_ok(),
            !cfg!(bexos_arch_x86_64)
        );
    }

    #[test]
    fn missing_hardware_and_capabilities_reject_adoption() {
        let mut target = Runtime::empty();
        assert!(target.validate().is_err());
        target.control = Channel(1);
        target.migration = Some(Channel(2));
        target.checkpoint = Some(alloc::vec![]);
        assert!(target.validate().is_err());
        target.domain = 3;
        target.clients.push(Channel(0));
        assert!(target.validate().is_err());
    }
}
