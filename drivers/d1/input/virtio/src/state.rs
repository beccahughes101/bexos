use crate::hardware::{ECAM_SIZE, Hardware, transport};
use bexos_graphics_runtime::{Mapping, migration::Component};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, live_migration::Resource};
use bexos_virtio_hal::{self as hal, buffer::DmaBuffer, migration::*};
#[derive(Default)]
pub struct Input {
    pub hardware: Option<Hardware>,
    pub saved: Option<Saved>,
    pub domain: u64,
    pub lifecycle: u64,
    pub sink: Option<Channel>,
    pub sequence: u64,
    pub reset: bool,
    pub reports: u64,
    pub resumed: bool,
}
pub struct Saved {
    ecam: Mapping,
    node: u64,
    buffers: [[u64; 3]; 3],
    available: u16,
    used: u16,
    healthy: bool,
    axes: [i32; 8],
    dma: Vec<hal::DmaAllocation>,
    mmio: Vec<hal::MmioMapping>,
}
impl Component for Input {
    fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        w.word(1);
        for v in [
            self.domain,
            self.lifecycle,
            self.sink.map_or(0, |c| c.0),
            self.sequence,
            self.reset as u64,
            self.reports,
        ] {
            w.word(v)
        }
        w.word((self.hardware.is_some() || self.saved.is_some()) as u64);
        if let Some(h) = &self.hardware {
            h.ecam.encode(w);
            w.word(h.node);
            for b in [&h.queues[0], &h.queues[1], &h.reports] {
                for v in b.snapshot() {
                    w.word(v)
                }
            }
            w.word(h.available as u64);
            w.word(h.used as u64);
            w.word(h.healthy as u64);
            for v in h.axes {
                w.word(v as i64 as u64)
            }
            encode_dma(w, &hal::dma_snapshot());
            encode_mmio(w, &hal::mmio_snapshot());
        } else if let Some(s) = &self.saved {
            s.ecam.encode(w);
            w.word(s.node);
            for b in s.buffers {
                for v in b {
                    w.word(v)
                }
            }
            w.word(s.available as u64);
            w.word(s.used as u64);
            w.word(s.healthy as u64);
            for v in s.axes {
                w.word(v as i64 as u64)
            }
            encode_dma(w, &s.dma);
            encode_mmio(w, &s.mmio);
        }
        Ok(())
    }
    fn decode(&mut self, r: &mut Decoder<'_>) -> Result<(), Error> {
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        self.domain = r.word()?;
        self.lifecycle = r.word()?;
        self.sink = match r.word()? {
            0 => None,
            h => Some(Channel(h)),
        };
        self.sequence = r.word()?;
        self.reset = r.flag()?;
        self.reports = r.word()?;
        self.saved = if r.flag()? {
            let ecam = Mapping::decode(r)?;
            let node = r.word()?;
            let mut buffers = [[0; 3]; 3];
            for b in &mut buffers {
                for v in b {
                    *v = r.word()?
                }
            }
            let available: u16 = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
            let used: u16 = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
            let healthy = r.flag()?;
            let mut axes = [0; 8];
            for a in &mut axes {
                *a = i32::try_from(r.word()? as i64).map_err(|_| Error::InvalidData)?;
            }
            let dma = decode_dma(r)?;
            let mmio = decode_mmio(r)?;
            if ecam.size != ECAM_SIZE
                || ecam.rights != 6
                || node > 0xffffff
                || dma.len() != 3
                || mmio.len() > 64
                || buffers.iter().enumerate().any(|(i, b)| {
                    b[2] != if i == 2 { 1 } else { 3 }
                        || !dma.iter().any(|d| {
                            d.active
                                && d.paddr == b[0]
                                && d.vaddr == b[1]
                                && d.size == b[2] * 4096
                                && d.handle != 0
                                && d.token != 0
                        })
                })
            {
                return Err(Error::InvalidData);
            }
            if available.wrapping_sub(used) != 64 {
                return Err(Error::InvalidData);
            }
            Some(Saved {
                ecam,
                node,
                buffers,
                available,
                used,
                healthy,
                axes,
                dma,
                mmio,
            })
        } else {
            None
        };
        self.validate()
    }
    fn validate(&self) -> Result<(), Error> {
        if (self.hardware.is_some() || self.saved.is_some()) && self.domain == 0 {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }
    fn resources(&self) -> Vec<Resource> {
        let (mut out, dma, mmio) = if let Some(h) = &self.hardware {
            (
                h.ecam.resources(),
                hal::dma_snapshot(),
                hal::mmio_snapshot(),
            )
        } else if let Some(s) = &self.saved {
            (s.ecam.resources(), s.dma.clone(), s.mmio.clone())
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        out.extend(
            [self.domain, self.lifecycle, self.sink.map_or(0, |c| c.0)]
                .into_iter()
                .filter(|h| *h != 0)
                .map(Resource::Handle),
        );
        for d in dma {
            out.push(Resource::Handle(d.handle));
            out.push(Resource::Pin(d.token));
            out.push(Resource::Mapping {
                handle: d.handle,
                offset: 0,
                va: d.vaddr,
                size: d.size,
                rights: 6,
            });
        }
        for m in mmio {
            out.push(Resource::Handle(m.handle));
            out.push(Resource::Mapping {
                handle: m.handle,
                offset: 0,
                va: m.vaddr,
                size: m.size,
                rights: 6,
            });
        }
        out
    }
    fn activate(&mut self) {
        if let Some(mut s) = self.saved.take() {
            hal::set_iommu_domain(self.domain);
            assert!(hal::adopt_dma_snapshot(&s.dma));
            assert!(hal::adopt_mmio_snapshot(&s.mmio));
            let mut buffers = s
                .buffers
                .map(|b| DmaBuffer::adopt(b).expect("retained input DMA"));
            for b in &mut buffers {
                b.activate()
            }
            let [a, b, reports] = buffers;
            s.ecam.owned = true;
            let transport = transport(s.node, s.ecam.address).expect("retained input PCI");
            hal::activate_adopted();
            self.hardware = Some(Hardware {
                transport,
                ecam: s.ecam,
                node: s.node,
                queues: [a, b],
                reports,
                available: s.available,
                used: s.used,
                healthy: s.healthy,
                axes: s.axes,
            });
        }
        self.resumed = true;
        bexos_userspace::log("virtio-input: queues and report stream adopted\n");
    }
}
