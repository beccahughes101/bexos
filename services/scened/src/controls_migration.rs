//! Versioned logical compositor controls. Raster scratch remains process local.
use crate::controls::{Controls, FocusRing, View};
use bexos_graphics::{
    accessibility::{DisplayTransform, valid_rect},
    resolved::Rect,
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, live_migration::Resource, service_binding::BoundServiceEndpoint};
use std::collections::BTreeSet;
impl Controls {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(3);
        w.word(self.generation);
        w.word(self.views.len() as u64);
        for (owner, view) in &self.views {
            for v in [*owner, view.token, view.identity, view.generation] {
                w.word(v);
            }
        }
        w.word(self.committed.len() as u64);
        for (owner, generation) in &self.committed {
            w.word(*owner);
            w.word(*generation);
        }
        w.word(self.stacking.len() as u64);
        for owner in &self.stacking {
            w.word(*owner);
        }
        for clients in [&self.accessibility, &self.shell] {
            w.word(clients.len() as u64);
            for client in clients {
                w.word(client.channel.0);
                w.word(client.allowed_methods.len() as u64);
                for ordinal in &client.allowed_methods {
                    w.word(*ordinal);
                }
            }
        }
        w.word(self.display.scale.to_bits());
        w.word(self.display.origin_x.to_bits());
        w.word(self.display.origin_y.to_bits());
        w.word(self.display.filter as u64);
        w.word(self.ring.is_some() as u64);
        if let Some(r) = self.ring {
            for v in [
                r.view,
                r.generation,
                r.bounds.x.to_bits(),
                r.bounds.y.to_bits(),
                r.bounds.width.to_bits(),
                r.bounds.height.to_bits(),
                r.width as u64,
                r.rgba as u64,
            ] {
                w.word(v);
            }
        }
        w.word(self.embedded.len() as u64);
        for owner in &self.embedded {
            w.word(*owner);
        }
        w.word(self.pending_display.is_some() as u64);
        if let Some(d) = self.pending_display {
            for v in [
                d.scale.to_bits(),
                d.origin_x.to_bits(),
                d.origin_y.to_bits(),
                d.filter as u64,
            ] {
                w.word(v);
            }
        }
        w.word(self.pending_ring.is_some() as u64);
        if let Some(ring) = self.pending_ring {
            w.word(ring.is_some() as u64);
            if let Some(r) = ring {
                for v in [
                    r.view,
                    r.generation,
                    r.bounds.x.to_bits(),
                    r.bounds.y.to_bits(),
                    r.bounds.width.to_bits(),
                    r.bounds.height.to_bits(),
                    r.width as u64,
                    r.rgba as u64,
                ] {
                    w.word(v);
                }
            }
        }
        w.word(self.pending_stacking.is_some() as u64);
        if let Some(stacking) = &self.pending_stacking {
            w.word(stacking.len() as u64);
            for owner in stacking {
                w.word(*owner);
            }
        }
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !(1..=3).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let mut out = Self::default();
        out.generation = r.word()?;
        for _ in 0..r.count(16)? {
            let owner = r.word()?;
            let v = View {
                token: r.word()?,
                identity: r.word()?,
                generation: r.word()?,
            };
            if out.views.insert(owner, v).is_some() {
                return Err(Error::InvalidData);
            }
        }
        for _ in 0..r.count(16)? {
            let owner = r.word()?;
            let sequence = r.word()?;
            if out.committed.insert(owner, sequence).is_some() {
                return Err(Error::InvalidData);
            }
        }
        for _ in 0..r.count(16)? {
            out.stacking.push(r.word()?);
        }
        for clients in [&mut out.accessibility, &mut out.shell] {
            for _ in 0..r.count(2)? {
                let channel = Channel(r.word()?);
                let mut ordinals = Vec::new();
                for _ in 0..r.count(32)? {
                    ordinals.push(r.word()?);
                }
                clients.push(BoundServiceEndpoint::new(channel, ordinals));
            }
        }
        out.display = DisplayTransform {
            scale: f64::from_bits(r.word()?),
            origin_x: f64::from_bits(r.word()?),
            origin_y: f64::from_bits(r.word()?),
            filter: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
        };
        if r.flag()? {
            out.ring = Some(FocusRing {
                view: r.word()?,
                generation: r.word()?,
                bounds: Rect {
                    x: f64::from_bits(r.word()?),
                    y: f64::from_bits(r.word()?),
                    width: f64::from_bits(r.word()?),
                    height: f64::from_bits(r.word()?),
                },
                width: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
                rgba: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
            });
        }
        if version >= 2 {
            for _ in 0..r.count(16)? {
                if !out.embedded.insert(r.word()?) {
                    return Err(Error::InvalidData);
                }
            }
        }
        if version >= 3 {
            if r.flag()? {
                out.pending_display = Some(DisplayTransform {
                    scale: f64::from_bits(r.word()?),
                    origin_x: f64::from_bits(r.word()?),
                    origin_y: f64::from_bits(r.word()?),
                    filter: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
                });
            }
            if r.flag()? {
                out.pending_ring = Some(if r.flag()? {
                    Some(FocusRing {
                        view: r.word()?,
                        generation: r.word()?,
                        bounds: Rect {
                            x: f64::from_bits(r.word()?),
                            y: f64::from_bits(r.word()?),
                            width: f64::from_bits(r.word()?),
                            height: f64::from_bits(r.word()?),
                        },
                        width: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
                        rgba: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
                    })
                } else {
                    None
                });
            }
            if r.flag()? {
                let mut stacking = Vec::new();
                for _ in 0..r.count(16)? {
                    stacking.push(r.word()?);
                }
                out.pending_stacking = Some(stacking);
            }
        }
        r.finish()?;
        out.validate()?;
        Ok(out)
    }
    pub fn validate(&self) -> Result<(), Error> {
        self.display.validate().map_err(|_| Error::InvalidData)?;
        if let Some(display) = self.pending_display {
            display.validate().map_err(|_| Error::InvalidData)?;
        }
        let mut handles = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut stacking = BTreeSet::new();
        if self.views.len() > 16
            || self.committed.len() > 16
            || self.stacking.len() > 16
            || self.embedded.len() > 16
            || self.embedded.contains(&0)
            || self.accessibility.len() > 2
            || self.shell.len() > 2
        {
            return Err(Error::Capacity);
        }
        for owner in &self.stacking {
            if *owner == 0 || !stacking.insert(*owner) {
                return Err(Error::InvalidData);
            }
        }
        if let Some(pending) = &self.pending_stacking {
            if pending.len() > 16 {
                return Err(Error::Capacity);
            }
            stacking.clear();
            for owner in pending {
                if *owner == 0 || !stacking.insert(*owner) {
                    return Err(Error::InvalidData);
                }
            }
        }
        for (owner, v) in &self.views {
            if *owner == 0
                || v.token == 0
                || v.identity == 0
                || !handles.insert(v.token)
                || !identities.insert(v.identity)
                || v.generation != self.committed.get(owner).copied().unwrap_or(0)
            {
                return Err(Error::InvalidData);
            }
        }
        for clients in [&self.accessibility, &self.shell] {
            for c in clients {
                if c.channel.0 == 0 || !handles.insert(c.channel.0) || c.allowed_methods.len() > 32
                {
                    return Err(Error::InvalidData);
                }
            }
        }
        for r in self.ring.into_iter().chain(self.pending_ring.flatten()) {
            if !self.views.contains_key(&r.view)
                || !valid_rect(r.bounds)
                || !(1..=16).contains(&r.width)
                || r.rgba & 255 != 255
            {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }
    pub fn resources(&self) -> Vec<Resource> {
        self.views
            .values()
            .map(|v| v.token)
            .chain(
                self.accessibility
                    .iter()
                    .chain(&self.shell)
                    .map(|c| c.channel.0),
            )
            .map(Resource::Handle)
            .collect()
    }
}
