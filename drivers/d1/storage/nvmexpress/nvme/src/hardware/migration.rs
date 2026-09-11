use super::*;
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::Resource;
impl Dma {
    fn encode(&self, w: &mut Encoder) {
        for n in [
            self.handle(),
            self.va(),
            self.pa(),
            self.token(),
            self.size(),
        ] {
            w.word(n);
        }
    }
    fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        Self::adopt(r.word()?, r.word()?, r.word()?, r.word()?, r.word()?)
            .map_err(|_| Error::InvalidData)
    }
    fn resources(&self, out: &mut Vec<Resource>) {
        out.push(Resource::Mapping {
            handle: self.handle(),
            offset: 0,
            va: self.va(),
            size: self.size(),
            rights: 6,
        });
        out.push(Resource::Pin(self.token()));
    }
}
impl Queue {
    fn encode(&self, w: &mut Encoder) {
        self.sq.encode(w);
        self.cq.encode(w);
        for n in [self.tail, self.head, self.phase, self.cid, self.id] {
            w.word(n as u64);
        }
    }
    fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let sq = Dma::decode(r)?;
        let cq = Dma::decode(r)?;
        let mut word = || u16::try_from(r.word()?).map_err(|_| Error::InvalidData);
        let q = Self {
            sq,
            cq,
            tail: word()?,
            head: word()?,
            phase: word()?,
            cid: word()?,
            id: word()?,
        };
        if q.tail >= DEPTH || q.head >= DEPTH || q.phase > 1 || q.id > 1 {
            return Err(Error::InvalidData);
        }
        Ok(q)
    }
}
impl Hardware {
    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(3);
        w.word(self.mmio.base());
        w.word(self.stride);
        w.word(self.iommu_domain);
        self.admin.encode(&mut w);
        self.io.encode(&mut w);
        self.payload.encode(&mut w);
        self.prp_list.encode(&mut w);
        for n in [
            self.info.namespace_id as u64,
            self.info.block_size as u64,
            self.info.block_count,
            self.info.max_transfer_blocks as u64,
            self.healthy as u64,
        ] {
            w.word(n);
        }
        w.finish()
    }
    pub fn adopt(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if version != 2 && version != 3 {
            return Err(Error::UnsupportedVersion);
        }
        let hw = Self {
            mmio: MappedMmio::new(r.word()?),
            stride: r.word()?,
            iommu_domain: if version >= 3 { r.word()? } else { 0 },
            admin: Queue::decode(&mut r)?,
            io: Queue::decode(&mut r)?,
            payload: Dma::decode(&mut r)?,
            prp_list: Dma::decode(&mut r)?,
            info: NamespaceInfo {
                namespace_id: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                block_size: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                block_count: r.word()?,
                max_transfer_blocks: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            },
            healthy: r.flag()?,
        };
        r.finish()?;
        if hw.admin.id != 0
            || hw.io.id != 1
            || hw.mmio.base() % 4096 != 0
            || !hw.stride.is_power_of_two()
            || hw.stride < 4
            || hw.iommu_domain == 0
            || hw.info.block_size != 512
            || hw.info.block_count == 0
            || hw.admin.sq.size() != PAGE_BYTES as u64
            || hw.admin.cq.size() != PAGE_BYTES as u64
            || hw.io.sq.size() != PAGE_BYTES as u64
            || hw.io.cq.size() != PAGE_BYTES as u64
            || hw.payload.size() != MAX_TRANSFER_BYTES as u64
            || hw.prp_list.size() != PAGE_BYTES as u64
            || !hw.healthy
        {
            return Err(Error::InvalidData);
        }
        Ok(hw)
    }
    pub fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.iommu_domain)];
        for d in [
            &self.admin.sq,
            &self.admin.cq,
            &self.io.sq,
            &self.io.cq,
            &self.payload,
            &self.prp_list,
        ] {
            d.resources(&mut out);
        }
        out
    }
    pub fn ready_to_migrate(&self) -> bool {
        self.healthy
    }
    pub fn activate(&mut self) {
        for d in [
            &mut self.admin.sq,
            &mut self.admin.cq,
            &mut self.io.sq,
            &mut self.io.cq,
            &mut self.payload,
            &mut self.prp_list,
        ] {
            d.activate();
        }
        bexos_userspace::log(&alloc::format!(
            "nvme: queues adopted without reset; sq_pa={:#x} cq_pa={:#x} payload_pa={:#x}\n",
            self.io.sq.pa(),
            self.io.cq.pa(),
            self.payload.pa()
        ));
    }
}
