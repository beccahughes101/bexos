use crate::hardware::{Hardware, transport};
use bexos_graphics::{
    Format, Surface,
    ownership::{Ownership, Transfer},
};
use bexos_graphics_runtime::{Mapping, migration::Component};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::Resource;
use bexos_virtio_hal::{self as hal, buffer::DmaBuffer, migration::*};
const GPU_STATE_V2: u64 = 0x4750550000000002;
const GPU_STATE_V3: u64 = 0x4750550000000003;
const GPU_STATE_V4: u64 = 0x4750550000000004;
const GPU_STATE_V6: u64 = 0x4750550000000006;
const GPU_STATE_V7: u64 = 0x4750550000000007;
const GPU_STATE_V5: u64 = 0x4750550000000005;
#[derive(Default)]
pub struct Display {
    pub pending_reply: Option<(u64, u64)>,
    pub scanout: crate::scanout_state::Scanout,
    pub pending_scanout: Option<crate::scanout::Pending>,
    pub pending_gpu: Option<crate::gpu_transport::Pending>,
    pub gpu: crate::gpu_state::GpuState,
    pub compat_version: u8,
    pub hardware: Option<Hardware>,
    pub ownership: Ownership,
    pub domain: u64,
    pub lifecycle: u64,
    pub saved: Option<Saved>,
    pub resumed: bool,
    pub legacy: bool,
}
pub struct Saved {
    surface: Surface,
    ecam: Mapping,
    node: u64,
    buffers: [[u64; 3]; 4],
    index: u16,
    front: u32,
    fence: u64,
    dma: Vec<hal::DmaAllocation>,
    mmio: Vec<hal::MmioMapping>,
    healthy: bool,
    capabilities: bexos_virtio_gpu_protocol::Capabilities,
}
fn ownership_encode(o: Ownership, w: &mut Encoder) {
    w.word(o.owner);
    w.word(o.generation);
    w.word(o.pending.is_some() as u64);
    if let Some(p) = o.pending {
        w.word(p.previous);
        w.word(p.next);
        w.word(p.deadline_us);
    }
}
impl Component for Display {
    fn quiescence_ready(&self) -> bool {
        self.scanout.pending.is_none()
            && self.pending_scanout.is_none()
            && self.pending_reply.is_none()
            && self.pending_gpu.is_none()
            && self
                .hardware
                .as_ref()
                .is_none_or(|h| h.inflight.is_none() && h.presenting.is_none())
    }
    fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        if !self.legacy {
            w.word(match self.compat_version {
                2 => GPU_STATE_V2,
                3 => GPU_STATE_V3,
                4 => GPU_STATE_V4,
                5 => GPU_STATE_V5,
                6 => GPU_STATE_V6,
                _ => GPU_STATE_V7,
            });
        }
        ownership_encode(self.ownership, w);
        w.word(self.domain);
        w.word(self.lifecycle);
        w.word((self.hardware.is_some() || self.saved.is_some()) as u64);
        if let Some(h) = &self.hardware {
            h.ecam.encode(w);
            w.word(h.node);
            for b in [&h.queue, &h.command, &h.frames[0], &h.frames[1]] {
                for v in b.snapshot() {
                    w.word(v);
                }
            }
            w.word(h.index as u64);
            w.word(h.front as u64);
            w.word(h.fence);
            encode_dma(w, &hal::dma_snapshot());
            encode_mmio(w, &hal::mmio_snapshot());
            w.word(h.healthy as u64);
            if !self.legacy {
                h.capabilities.encode(w);
                if self.compat_version != 2 {
                    if self.compat_version == 3 {
                        self.gpu.encode_legacy(w);
                    } else {
                        self.gpu.encode_version(w, self.compat_version == 4);
                    }
                }
            }
        } else if let Some(s) = &self.saved {
            s.ecam.encode(w);
            w.word(s.node);
            for b in s.buffers {
                for v in b {
                    w.word(v);
                }
            }
            w.word(s.index as u64);
            w.word(s.front as u64);
            w.word(s.fence);
            encode_dma(w, &s.dma);
            encode_mmio(w, &s.mmio);
            w.word(s.healthy as u64);
            if !self.legacy {
                s.capabilities.encode(w);
                if self.compat_version != 2 {
                    if self.compat_version == 3 {
                        self.gpu.encode_legacy(w);
                    } else {
                        self.gpu.encode_version(w, self.compat_version == 4);
                    }
                }
            }
        }
        if !self.legacy
            && matches!(self.compat_version, 0 | 6)
            && (self.hardware.is_some() || self.saved.is_some())
        {
            if self.compat_version == 0 {
                let surface = self
                    .hardware
                    .as_ref()
                    .map(|h| h.surface)
                    .or_else(|| self.saved.as_ref().map(|s| s.surface))
                    .ok_or(Error::InvalidData)?;
                for value in [
                    surface.width,
                    surface.height,
                    surface.stride,
                    surface.format as u32,
                ] {
                    w.word(value as u64);
                }
            }
            self.scanout.encode(w)?;
        }
        Ok(())
    }
    fn decode(&mut self, r: &mut Decoder<'_>) -> Result<(), Error> {
        let first = r.word()?;
        let compat_version = if first == GPU_STATE_V2 {
            2
        } else if first == GPU_STATE_V3 {
            3
        } else if first == GPU_STATE_V4 {
            4
        } else if first == GPU_STATE_V5 {
            5
        } else if first == GPU_STATE_V6 {
            6
        } else {
            0
        };
        let legacy = ![
            GPU_STATE_V2,
            GPU_STATE_V3,
            GPU_STATE_V4,
            GPU_STATE_V5,
            GPU_STATE_V6,
            GPU_STATE_V7,
        ]
        .contains(&first);
        if legacy && first & 0xffff_ffff_ffff_ff00 == GPU_STATE_V2 & 0xffff_ffff_ffff_ff00 {
            return Err(Error::UnsupportedVersion);
        }
        let owner = if legacy { first } else { r.word()? };
        let mut ownership = Ownership {
            owner,
            generation: r.word()?,
            pending: None,
        };
        if r.flag()? {
            ownership.pending = Some(Transfer {
                previous: r.word()?,
                next: r.word()?,
                deadline_us: r.word()?,
            });
        }
        let domain = r.word()?;
        let lifecycle = r.word()?;
        if !r.flag()? {
            if ownership.owner != 0 || ownership.pending.is_some() {
                return Err(Error::InvalidData);
            }
            self.domain = domain;
            self.lifecycle = lifecycle;
            self.ownership = ownership;
            self.saved = None;
            self.hardware = None;
            self.legacy = legacy;
            self.compat_version = compat_version;
            self.gpu = Default::default();
            self.scanout = Default::default();
            return Ok(());
        }
        let ecam = Mapping::decode(r)?;
        let node = r.word()?;
        let mut buffers = [[0; 3]; 4];
        for b in &mut buffers {
            for v in b {
                *v = r.word()?;
            }
        }
        let index = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let front = r.word()?;
        let fence = r.word()?;
        let dma = decode_dma(r)?;
        let mmio = decode_mmio(r)?;
        let healthy = r.flag()?;
        let capabilities = if legacy {
            Default::default()
        } else {
            bexos_virtio_gpu_protocol::Capabilities::decode(r)?
        };
        let gpu = if legacy || compat_version == 2 {
            Default::default()
        } else {
            crate::gpu_state::GpuState::decode_version(
                r,
                &dma,
                if compat_version == 0 {
                    5
                } else {
                    compat_version
                },
            )?
        };
        let surface = if !legacy && compat_version == 0 {
            let mut value = || u32::try_from(r.word()?).map_err(|_| Error::InvalidData);
            Surface {
                width: value()?,
                height: value()?,
                stride: value()?,
                format: value()?.try_into().map_err(|_| Error::InvalidData)?,
            }
        } else {
            Surface {
                width: 800,
                height: 600,
                stride: 3200,
                format: Format::Bgra,
            }
        };
        if surface.width > 4096
            || surface.height > 4096
            || surface.stride != surface.width * 4
            || surface.format != Format::Bgra
        {
            return Err(Error::InvalidData);
        }
        let frame_pages = surface
            .validate(u64::MAX)
            .map_err(|_| Error::InvalidData)?
            .div_ceil(4096) as u64;
        let scanout = if !legacy && matches!(compat_version, 0 | 6) {
            crate::scanout_state::Scanout::decode(r, &gpu.registry, surface)?
        } else {
            crate::scanout_state::Scanout {
                current: front as u32 + 1,
                ..Default::default()
            }
        };
        if ecam.size != 0x1000000
            || ecam.rights != 6
            || node > 0xffffff
            || dma.len() != 4 + gpu.memory.len()
            || (healthy && scanout.uncertain.is_some())
            || scanout.buffers.iter().any(|b| {
                dma.iter().any(|d| {
                    d.token == b.token
                        || d.handle == b.mapping.handle
                        || d.vaddr == b.mapping.address
                        || (d.paddr < b.address + b.mapping.size && b.address < d.paddr + d.size)
                })
            })
            || mmio.len() > 64
            || dma.iter().any(|d| {
                !d.active
                    || d.paddr == 0
                    || d.vaddr == 0
                    || d.handle == 0
                    || d.token == 0
                    || d.size == 0
            })
            || mmio
                .iter()
                .any(|m| !m.active || m.paddr == 0 || m.vaddr == 0 || m.handle == 0 || m.size == 0)
            || domain == 0
            || front > 1
            || buffers[0][2] != 3
            || buffers[1][2] != 2
            || buffers[2][2] != frame_pages
            || buffers[3][2] != frame_pages
            || buffers.iter().any(|b| {
                !dma.iter()
                    .any(|d| d.paddr == b[0] && d.vaddr == b[1] && d.size == b[2] * 4096)
            })
            || gpu.memory.iter().any(|m| {
                buffers
                    .iter()
                    .any(|b| b[0] == m.saved[0] || b[1] == m.saved[1])
            })
            || (!gpu.registry.contexts.is_empty()
                && !bexos_virtio_gpu_protocol::transport::supported(&capabilities))
            || ownership
                .pending
                .is_some_and(|p| p.previous == 0 || p.next != ownership.owner)
        {
            return Err(Error::InvalidData);
        }
        self.domain = domain;
        self.lifecycle = lifecycle;
        self.ownership = ownership;
        self.legacy = legacy;
        self.compat_version = compat_version;
        self.gpu = gpu;
        self.scanout = scanout;
        self.saved = Some(Saved {
            surface,
            ecam,
            node,
            buffers,
            index,
            front: front as u32,
            fence,
            dma,
            mmio,
            healthy,
            capabilities,
        });
        Ok(())
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
        if self.domain != 0 {
            out.push(Resource::Handle(self.domain));
        }
        if self.lifecycle != 0 {
            out.push(Resource::Handle(self.lifecycle));
        }
        out.extend(
            self.gpu
                .host
                .iter()
                .filter(|m| m.handle != 0)
                .map(|m| Resource::Handle(m.handle)),
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
        out.extend(self.scanout.resources());
        out
    }
    fn activate(&mut self) {
        self.legacy = false;
        self.compat_version = 0;
        if let Some(mut s) = self.saved.take() {
            hal::set_iommu_domain(self.domain);
            assert!(hal::adopt_dma_snapshot(&s.dma));
            assert!(hal::adopt_mmio_snapshot(&s.mmio));
            let mut buffers = s
                .buffers
                .map(|b| DmaBuffer::adopt(b).expect("retained GPU DMA"));
            for b in &mut buffers {
                b.activate();
            }
            let [queue, command, a, b] = buffers;
            self.gpu.activate();
            self.scanout.activate();
            s.ecam.owned = true;
            let transport = transport(s.node, s.ecam.address).expect("retained GPU PCI transport");
            hal::activate_adopted();
            self.resumed = true;
            bexos_userspace::log(&format!("virtio-gpu: adopted scanout fence={}\n", s.fence));
            self.hardware = Some(Hardware {
                aperture: self.gpu.aperture,
                transport,
                ecam: s.ecam,
                node: s.node,
                queue,
                command,
                frames: [a, b],
                surface: s.surface,
                index: s.index,
                front: s.front,
                scanout_resource: self.scanout.current,
                fence: s.fence,
                healthy: s.healthy,
                capabilities: s.capabilities,
                inflight: None,
                presenting: None,
                // Process-local damage caches may be discarded. Repairing both
                // buffers on first reuse preserves legacy migration compatibility.
                stale: [Some(s.surface.full()); 2],
            });
        }
    }
    fn validate(&self) -> Result<(), Error> {
        if (self.hardware.is_some() || self.saved.is_some()) && self.domain == 0 {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }
}
