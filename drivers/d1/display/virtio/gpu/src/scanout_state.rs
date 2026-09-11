//! Retained scanout ownership. Only logical IDs, VMOs, mappings and DMA tokens
//! cross a transplant; failed candidates never own or release source resources.
use bexos_graphics::{Format, Surface};
use bexos_graphics_runtime::Mapping;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::Resource;
use bexos_virtio_gpu_protocol::transport::Registry;

pub const MAX_BUFFERS: usize = 16;
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;
pub const FORMATS: u32 = (1 << Format::Bgrx as u32) | (1 << Format::Rgbx as u32);
pub struct Imported {
    pub id: u32,
    pub owner: u64,
    pub surface: Surface,
    pub mapping: Mapping,
    pub address: u64,
    pub token: u64,
    pub ready: bool,
    pub retiring: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retirement {
    pub resource: u32,
    pub sequence: u64,
    pub channel: u64,
}
pub struct Scanout {
    pub buffers: Vec<Imported>,
    pub current: u32,
    pub release: Option<Retirement>,
    /// Drained before encoding, never confused with the current buffer lease.
    pub pending: Option<Retirement>,
    pub uncertain: Option<Retirement>,
}
impl Default for Scanout {
    fn default() -> Self {
        Self {
            buffers: Vec::with_capacity(MAX_BUFFERS),
            current: 1,
            release: None,
            pending: None,
            uncertain: None,
        }
    }
}
impl Scanout {
    pub fn bytes(&self) -> u64 {
        self.buffers.iter().map(|b| b.mapping.size).sum()
    }
    pub fn get(&self, owner: u64, id: u32) -> Option<&Imported> {
        self.buffers
            .iter()
            .find(|b| b.id == id && b.owner == owner && b.ready && !b.retiring)
    }
    pub fn busy(&self, id: u32) -> bool {
        id == self.current
            || self.pending.is_some_and(|p| p.resource == id)
            || self.uncertain.is_some_and(|p| p.resource == id)
    }
    /// Called only after the device has acknowledged SET_SCANOUT. The previous
    /// resource can no longer be read by this scanout, even if FLUSH later fails.
    pub fn switched(&mut self, id: u32) -> Option<Retirement> {
        if id == self.current {
            return None;
        }
        let old = self.release.take();
        self.current = id;
        if self.pending.is_some_and(|p| p.resource == id) {
            self.release = self.pending.take();
        }
        old
    }
    pub fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        w.word(1);
        w.word(self.current as u64);
        w.word(self.release.is_some() as u64);
        if let Some(p) = self.release {
            for v in [p.resource as u64, p.sequence, p.channel] {
                w.word(v);
            }
        }
        w.word(self.uncertain.is_some() as u64);
        if let Some(p) = self.uncertain {
            for v in [p.resource as u64, p.sequence, p.channel] {
                w.word(v);
            }
        }
        w.word(self.pending.is_some() as u64);
        if let Some(p) = self.pending {
            for v in [p.resource as u64, p.sequence, p.channel] {
                w.word(v);
            }
        }
        w.word(self.buffers.len() as u64);
        for b in &self.buffers {
            for v in [
                b.id as u64,
                b.owner,
                b.surface.width as u64,
                b.surface.height as u64,
                b.surface.stride as u64,
                b.surface.format as u64,
                b.address,
                b.token,
                b.ready as u64,
                b.retiring as u64,
            ] {
                w.word(v);
            }
            b.mapping.encode(w);
        }
        Ok(())
    }
    pub fn decode(
        r: &mut Decoder<'_>,
        registry: &Registry,
        display: Surface,
    ) -> Result<Self, Error> {
        fn u32word(r: &mut Decoder<'_>) -> Result<u32, Error> {
            r.word()?.try_into().map_err(|_| Error::InvalidData)
        }
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mut s = Self {
            current: u32word(r)?,
            ..Self::default()
        };
        if r.flag()? {
            s.release = Some(Retirement {
                resource: u32word(r)?,
                sequence: r.word()?,
                channel: r.word()?,
            });
        }
        if r.flag()? {
            s.uncertain = Some(Retirement {
                resource: u32word(r)?,
                sequence: r.word()?,
                channel: r.word()?,
            });
        }
        if r.flag()? {
            s.pending = Some(Retirement {
                resource: u32word(r)?,
                sequence: r.word()?,
                channel: r.word()?,
            });
        }
        for _ in 0..r.count(MAX_BUFFERS)? {
            let b = Imported {
                id: u32word(r)?,
                owner: r.word()?,
                surface: Surface {
                    width: u32word(r)?,
                    height: u32word(r)?,
                    stride: u32word(r)?,
                    format: u32word(r)?.try_into().map_err(|_| Error::InvalidData)?,
                },
                address: r.word()?,
                token: r.word()?,
                ready: r.flag()?,
                retiring: r.flag()?,
                mapping: Mapping::decode(r)?,
            };
            let len = import_size(b.surface, display).ok_or(Error::InvalidData)?;
            if b.owner == 0
                || b.token == 0
                || b.address == 0
                || b.address % 4096 != 0
                || b.address.checked_add(len).is_none()
                || b.mapping.rights != 2
                || b.mapping.size != len
                || !registry.was_reserved(b.id)
                || registry.resources.iter().any(|v| v.id == b.id)
                || s.buffers.iter().any(|v| {
                    v.id == b.id
                        || v.token == b.token
                        || v.mapping.handle == b.mapping.handle
                        || v.mapping.address == b.mapping.address
                        || (v.address < b.address + len && b.address < v.address + v.mapping.size)
                })
                || (!b.ready && !b.retiring)
                || s.bytes() + len > MAX_BYTES
            {
                return Err(Error::InvalidData);
            }
            s.buffers.push(b);
        }
        if !(1..=2).contains(&s.current) && !s.buffers.iter().any(|b| b.id == s.current && b.ready)
            || s.release.is_some_and(|p| {
                p.resource != s.current
                    || p.resource < 3
                    || p.sequence == 0
                    || p.channel == 0
                    || s.buffers.iter().any(|b| b.mapping.handle == p.channel)
            })
            || s.uncertain.is_some_and(|p| {
                p.resource == s.current
                    || p.channel == 0
                    || p.sequence == 0
                    || !s.buffers.iter().any(|b| b.id == p.resource && b.ready)
                    || s.buffers.iter().any(|b| b.mapping.handle == p.channel)
                    || s.release.is_some_and(|v| v.channel == p.channel)
            })
            || s.pending.is_some_and(|p| {
                p.resource == s.current
                    || p.sequence == 0
                    || p.channel == 0
                    || !s.buffers.iter().any(|b| b.id == p.resource && b.ready)
                    || s.release.is_some_and(|v| v.channel == p.channel)
                    || s.uncertain.is_some_and(|v| v.channel == p.channel)
            })
            || (s.current >= 3) != s.release.is_some()
        {
            return Err(Error::InvalidData);
        }
        Ok(s)
    }
    pub fn resources(&self) -> Vec<Resource> {
        let mut out = Vec::new();
        for b in &self.buffers {
            out.extend(b.mapping.resources());
            out.push(Resource::Pin(b.token));
        }
        if let Some(p) = self.release {
            out.push(Resource::Handle(p.channel));
        }
        if let Some(p) = self.uncertain {
            out.push(Resource::Handle(p.channel));
        }
        if let Some(p) = self.pending {
            out.push(Resource::Handle(p.channel));
        }
        out
    }
    pub fn activate(&mut self) {
        for b in &mut self.buffers {
            b.mapping.owned = true;
        }
    }
}
pub fn import_size(surface: Surface, display: Surface) -> Option<u64> {
    let len = surface.validate(u64::MAX).ok()? as u64;
    (surface.format.is_opaque()
        && surface.width == display.width
        && surface.height == display.height
        && surface.stride == surface.width.checked_mul(4)?)
    .then_some((len + 4095) & !4095)
}
