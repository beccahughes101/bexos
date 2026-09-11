use crate::{Error, word};
use alloc::vec::Vec;
use bexos_migration::{
    Error as MigrationError,
    codec::{Decoder, Encoder},
};
pub const MAX_SCANOUTS: usize = 16;
pub const MAX_CAPSETS: usize = 16;
pub const VENUS_CAPSET: u32 = 4;
pub const FEATURE_VIRGL: u64 = 1;
pub const FEATURE_BLOB: u64 = 1 << 3;
pub const FEATURE_CONTEXT_INIT: u64 = 1 << 4;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub scanout: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
    pub flags: u32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capset {
    pub id: u32,
    pub version: u32,
    pub size: u32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub offered: u64,
    pub negotiated: u64,
    pub modes: Vec<Mode>,
    pub capsets: Vec<Capset>,
}
impl Capabilities {
    /// Primary scanout extent supported by the compositor's bounded buffers.
    /// VirtIO's mode record has no refresh timing; this is never a VSYNC source.
    pub fn primary_extent(&self) -> Option<(u32, u32)> {
        self.modes
            .iter()
            .find(|m| {
                m.scanout == 0
                    && m.enabled
                    && (1..=4096).contains(&m.width)
                    && (1..=4096).contains(&m.height)
            })
            .map(|m| (m.width, m.height))
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.negotiated & !self.offered != 0
            || self.modes.len() > MAX_SCANOUTS
            || self.capsets.len() > MAX_CAPSETS
        {
            return Err(Error::Invalid);
        }
        for (i, m) in self.modes.iter().enumerate() {
            if m.scanout >= MAX_SCANOUTS as u32
                || self.modes[..i].iter().any(|v| v.scanout == m.scanout)
                || m.width > 16384
                || m.height > 16384
                || m.x.checked_add(m.width).is_none()
                || m.y.checked_add(m.height).is_none()
                || (m.enabled && (m.width == 0 || m.height == 0))
            {
                return Err(Error::Invalid);
            }
        }
        for (i, c) in self.capsets.iter().enumerate() {
            if c.id == 0
                || c.size == 0
                || c.size > 1024 * 1024
                || self.capsets[..i].iter().any(|v| v.id == c.id)
            {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub fn parse_modes(&mut self, reply: &[u8], count: u32) -> Result<(), Error> {
        if count > MAX_SCANOUTS as u32
            || reply.len() != 24 + MAX_SCANOUTS * 24
            || word(reply, 0)? != 0x1101
        {
            return Err(Error::Invalid);
        }
        let mut modes = Vec::with_capacity(count as usize);
        for scanout in 0..count {
            let at = 24 + scanout as usize * 24;
            let enabled = word(reply, at + 16)?;
            if enabled > 1 {
                return Err(Error::Invalid);
            }
            modes.push(Mode {
                scanout,
                x: word(reply, at)?,
                y: word(reply, at + 4)?,
                width: word(reply, at + 8)?,
                height: word(reply, at + 12)?,
                enabled: enabled != 0,
                flags: word(reply, at + 20)?,
            });
        }
        let mut candidate = self.clone();
        candidate.modes = modes;
        candidate.validate()?;
        self.modes = candidate.modes;
        Ok(())
    }
    pub fn add_capset(&mut self, reply: &[u8]) -> Result<(), Error> {
        if self.capsets.len() >= MAX_CAPSETS {
            return Err(Error::Capacity);
        }
        if reply.len() != 40 || word(reply, 0)? != 0x1102 {
            return Err(Error::Invalid);
        }
        let c = Capset {
            id: word(reply, 24)?,
            version: word(reply, 28)?,
            size: word(reply, 32)?,
        };
        if c.id == 0
            || c.size == 0
            || c.size > 1024 * 1024
            || self.capsets.iter().any(|v| v.id == c.id)
        {
            return Err(Error::Invalid);
        }
        self.capsets.push(c);
        Ok(())
    }
    pub fn venus_offered(&self) -> bool {
        let bits = FEATURE_VIRGL | FEATURE_BLOB | FEATURE_CONTEXT_INIT;
        self.offered & bits == bits && self.capsets.iter().any(|c| c.id == VENUS_CAPSET)
    }
    pub fn encode(&self, w: &mut Encoder) {
        w.word(1);
        w.word(self.offered);
        w.word(self.negotiated);
        w.word(self.modes.len() as u64);
        for m in &self.modes {
            for v in [
                m.scanout,
                m.x,
                m.y,
                m.width,
                m.height,
                m.enabled as u32,
                m.flags,
            ] {
                w.word(v as u64);
            }
        }
        w.word(self.capsets.len() as u64);
        for c in &self.capsets {
            w.word(c.id as u64);
            w.word(c.version as u64);
            w.word(c.size as u64);
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, MigrationError> {
        if r.word()? != 1 {
            return Err(MigrationError::UnsupportedVersion);
        }
        let mut out = Self {
            offered: r.word()?,
            negotiated: r.word()?,
            ..Default::default()
        };
        fn u32word(r: &mut Decoder<'_>) -> Result<u32, MigrationError> {
            r.word()?
                .try_into()
                .map_err(|_| MigrationError::InvalidData)
        }
        for _ in 0..r.count(MAX_SCANOUTS)? {
            out.modes.push(Mode {
                scanout: u32word(r)?,
                x: u32word(r)?,
                y: u32word(r)?,
                width: u32word(r)?,
                height: u32word(r)?,
                enabled: r.flag()?,
                flags: u32word(r)?,
            });
        }
        for _ in 0..r.count(MAX_CAPSETS)? {
            out.capsets.push(Capset {
                id: u32word(r)?,
                version: u32word(r)?,
                size: u32word(r)?,
            });
        }
        out.validate().map_err(|_| MigrationError::InvalidData)?;
        Ok(out)
    }
}
