//! Bounded reserved-edge arbitration. Claiming a stream requires the caller to
//! cancel its original recipient before forwarding the saved Down to the shell.
use crate::{Phase, Pointer};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgePolicy {
    pub edges: u32,
    pub inset: f64,
    pub threshold: f64,
}
impl Default for EdgePolicy {
    fn default() -> Self {
        Self {
            edges: 0,
            inset: 16.,
            threshold: 32.,
        }
    }
}
impl EdgePolicy {
    pub fn validate(self) -> Result<(), Error> {
        if self.edges & !15 != 0
            || !self.inset.is_finite()
            || !(1. ..=64.).contains(&self.inset)
            || !self.threshold.is_finite()
            || !(8. ..=256.).contains(&self.threshold)
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Contact {
    start: Pointer,
    last: Pointer,
    edge: u32,
    claimed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decision {
    Pass,
    Claim { start: Pointer, event: Pointer },
    Owned(Pointer),
}
pub struct Gestures {
    pub policy: EdgePolicy,
    contacts: [Option<Contact>; 64],
}
impl Default for Gestures {
    fn default() -> Self {
        Self {
            policy: Default::default(),
            contacts: [None; 64],
        }
    }
}
impl Gestures {
    pub fn configure(
        &mut self,
        policy: EdgePolicy,
        mut cancel: impl FnMut(Pointer),
    ) -> Result<(), Error> {
        policy.validate()?;
        for contact in &mut self.contacts {
            if let Some(c) = contact.take().filter(|c| c.claimed) {
                cancel(Pointer {
                    phase: Phase::Cancel,
                    ..c.last
                });
            }
        }
        self.policy = policy;
        Ok(())
    }
    pub fn pointer(&mut self, p: Pointer, width: f64, height: f64) -> Decision {
        let slot = self
            .contacts
            .iter()
            .position(|c| c.is_some_and(|c| (c.start.device, c.start.id) == (p.device, p.id)));
        if !p.x.is_finite()
            || !p.y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.
            || height <= 0.
        {
            if let Some(index) = slot {
                if let Some(c) = self.contacts[index].take().filter(|c| c.claimed) {
                    return Decision::Owned(Pointer {
                        phase: Phase::Cancel,
                        ..c.last
                    });
                }
            }
            return Decision::Pass;
        }
        if p.phase == Phase::Down {
            // Device normalization emits Cancel before reusing an active ID.
            if slot.is_some() || p.device == 0 {
                return Decision::Pass;
            }
            let edges = self.policy.edges;
            let inset = self.policy.inset;
            let edge = if edges & 1 != 0 && p.x >= 0. && p.x < inset {
                1
            } else if edges & 2 != 0 && p.x >= width - inset && p.x < width {
                2
            } else if edges & 4 != 0 && p.y >= 0. && p.y < inset {
                4
            } else if edges & 8 != 0 && p.y >= height - inset && p.y < height {
                8
            } else {
                0
            };
            if edge != 0 {
                if let Some(slot) = self.contacts.iter_mut().find(|c| c.is_none()) {
                    *slot = Some(Contact {
                        start: p,
                        last: p,
                        edge,
                        claimed: false,
                    });
                }
            }
            return Decision::Pass;
        }
        let Some(index) = slot else {
            return Decision::Pass;
        };
        let mut c = self.contacts[index].unwrap();
        c.last = p;
        if matches!(p.phase, Phase::Up | Phase::Cancel) {
            self.contacts[index] = None;
            return if c.claimed {
                Decision::Owned(p)
            } else {
                Decision::Pass
            };
        }
        if c.claimed {
            self.contacts[index] = Some(c);
            return Decision::Owned(p);
        }
        let dx = p.x - c.start.x;
        let dy = p.y - c.start.y;
        let (inward, across) = match c.edge {
            1 => (dx, dy.abs()),
            2 => (-dx, dy.abs()),
            4 => (dy, dx.abs()),
            8 => (-dy, dx.abs()),
            _ => unreachable!(),
        };
        if inward >= self.policy.threshold && inward >= across {
            c.claimed = true;
            self.contacts[index] = Some(c);
            Decision::Claim {
                start: c.start,
                event: p,
            }
        } else if across >= self.policy.threshold && across > inward.abs() {
            self.contacts[index] = None;
            Decision::Pass
        } else {
            self.contacts[index] = Some(c);
            Decision::Pass
        }
    }
    pub fn remove_device(&mut self, device: u64, mut cancel: impl FnMut(Pointer)) {
        for contact in &mut self.contacts {
            if contact.is_some_and(|c| c.start.device == device) {
                if let Some(c) = contact.take().filter(|c| c.claimed) {
                    cancel(Pointer {
                        phase: Phase::Cancel,
                        ..c.last
                    });
                }
            }
        }
    }
    pub fn encode(&self, w: &mut Encoder) {
        w.word(1);
        w.word(self.policy.edges as u64);
        w.word(self.policy.inset.to_bits());
        w.word(self.policy.threshold.to_bits());
        w.word(self.contacts.iter().flatten().count() as u64);
        for c in self.contacts.iter().flatten() {
            crate::migration::pointer_write(c.start, w);
            crate::migration::pointer_write(c.last, w);
            w.word(c.edge as u64);
            w.word(c.claimed as u64);
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mut out = Self::default();
        out.policy = EdgePolicy {
            edges: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
            inset: f64::from_bits(r.word()?),
            threshold: f64::from_bits(r.word()?),
        };
        out.policy.validate()?;
        for index in 0..r.count(64)? {
            let c = Contact {
                start: crate::migration::pointer_read(r)?,
                last: crate::migration::pointer_read(r)?,
                edge: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
                claimed: r.flag()?,
            };
            if c.start.device == 0
                || c.start.phase != Phase::Down
                || (c.start.device, c.start.id) != (c.last.device, c.last.id)
                || !matches!(c.edge, 1 | 2 | 4 | 8)
                || out.policy.edges & c.edge == 0
                || matches!(c.last.phase, Phase::Up | Phase::Cancel)
                || out.contacts[..index]
                    .iter()
                    .flatten()
                    .any(|old| (old.start.device, old.start.id) == (c.start.device, c.start.id))
            {
                return Err(Error::InvalidData);
            }
            out.contacts[index] = Some(c);
        }
        Ok(out)
    }
}
