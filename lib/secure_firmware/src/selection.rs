//! Canonical x86 boot-selection identities. These records are authoritative
//! only when read through the root-only authenticated Trusty journal.
use crate::{Component, MAX_IMAGE_BYTES};

pub const IDENTITY_BYTES: usize = 56;
pub const REQUEST_BYTES: usize = 256;
pub const STATE_BYTES: usize = 512;
pub const MAX_ATTEMPTS: u32 = 2;
const MAGIC: &[u8; 8] = b"BEXBS002";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    A,
    B,
}
impl Slot {
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
    pub const fn number(self) -> u32 {
        match self {
            Self::A => 1,
            Self::B => 2,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    pub slot: Slot,
    pub generation: u64,
    pub digest: [u8; 32],
    pub length: u64,
}
impl Identity {
    pub const INITIAL: Self = Self {
        slot: Slot::A,
        generation: 1,
        digest: [0; 32],
        length: 0,
    };
    pub fn valid(self, initial: bool) -> bool {
        (initial && self == Self::INITIAL)
            || (self.generation > 1
                && self.digest != [0; 32]
                && (64..=MAX_IMAGE_BYTES as u64).contains(&self.length))
    }
    pub fn encode(self) -> [u8; IDENTITY_BYTES] {
        let mut b = [0; IDENTITY_BYTES];
        b[..4].copy_from_slice(&self.slot.number().to_le_bytes());
        b[8..16].copy_from_slice(&self.generation.to_le_bytes());
        b[16..48].copy_from_slice(&self.digest);
        b[48..56].copy_from_slice(&self.length.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8], initial: bool) -> Result<Self, Invalid> {
        if b.len() != IDENTITY_BYTES || b[4..8] != [0; 4] {
            return Err(Invalid);
        }
        let slot = match word(b, 0) {
            1 => Slot::A,
            2 => Slot::B,
            _ => return Err(Invalid),
        };
        let value = Self {
            slot,
            generation: wide(b, 8),
            digest: b[16..48].try_into().unwrap(),
            length: wide(b, 48),
        };
        if value.valid(initial) {
            Ok(value)
        } else {
            Err(Invalid)
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invalid;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Idle,
    Pending,
    Trial,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Stage,
    Attempt,
    Commit,
    Abort,
}
impl Operation {
    pub const fn number(self) -> u32 {
        match self {
            Self::Stage => 2,
            Self::Attempt => 3,
            Self::Commit => 4,
            Self::Abort => 5,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub revision: u64,
    pub committed: [Identity; 2],
    pub phase: Phase,
    pub pending: Option<(Component, Identity)>,
    pub attempts: u32,
    last_request: [u8; REQUEST_BYTES],
}
pub const fn component_number(c: Component) -> u32 {
    match c {
        Component::Trusty => 1,
        Component::Hypervisor => 2,
    }
}
pub fn component(n: u32) -> Result<Component, Invalid> {
    match n {
        1 => Ok(Component::Trusty),
        2 => Ok(Component::Hypervisor),
        _ => Err(Invalid),
    }
}
pub fn query_request() -> [u8; REQUEST_BYTES] {
    let mut r = [0; REQUEST_BYTES];
    r[..8].copy_from_slice(MAGIC);
    r[8] = 1;
    r[12] = 2;
    r
}
pub fn request(
    revision: u64,
    op: Operation,
    c: Component,
    image: Identity,
) -> Result<[u8; REQUEST_BYTES], Invalid> {
    if revision == 0 || revision == u64::MAX || !image.valid(false) {
        return Err(Invalid);
    }
    let mut r = query_request();
    r[8..12].copy_from_slice(&op.number().to_le_bytes());
    r[16..24].copy_from_slice(&revision.to_le_bytes());
    r[24..28].copy_from_slice(&component_number(c).to_le_bytes());
    r[32..88].copy_from_slice(&image.encode());
    Ok(r)
}
pub fn validate_mutation_request(r: &[u8; REQUEST_BYTES]) -> Result<(), Invalid> {
    let op = match word(r, 8) {
        2 => Operation::Stage,
        3 => Operation::Attempt,
        4 => Operation::Commit,
        5 => Operation::Abort,
        _ => return Err(Invalid),
    };
    if request(
        wide(r, 16),
        op,
        component(word(r, 24))?,
        Identity::decode(&r[32..88], false)?,
    )? != *r
    {
        return Err(Invalid);
    }
    Ok(())
}
impl State {
    /// Authenticated last transition for reboot-visible product status.
    pub fn last_change(&self) -> Option<(Operation, Component, Identity)> {
        if self.revision == 1 {
            return None;
        }
        let op = match word(&self.last_request, 8) {
            2 => Operation::Stage,
            3 => Operation::Attempt,
            4 => Operation::Commit,
            5 => Operation::Abort,
            _ => return None,
        };
        Some((
            op,
            component(word(&self.last_request, 24)).ok()?,
            Identity::decode(&self.last_request[32..88], false).ok()?,
        ))
    }
    pub fn decode(b: &[u8]) -> Result<Self, Invalid> {
        if b.len() != STATE_BYTES
            || &b[..8] != MAGIC
            || word(b, 8) != 0
            || word(b, 12) != 2
            || wide(b, 16) == 0
            || b[204..256] != [0; 52]
        {
            return Err(Invalid);
        }
        let phase = match word(b, 24) {
            0 => Phase::Idle,
            1 => Phase::Pending,
            2 => Phase::Trial,
            _ => return Err(Invalid),
        };
        let committed = [
            Identity::decode(&b[32..88], true)?,
            Identity::decode(&b[88..144], true)?,
        ];
        let attempts = word(b, 200);
        let pending = if phase == Phase::Idle {
            if word(b, 28) != 0 || b[144..204] != [0; 60] {
                return Err(Invalid);
            }
            None
        } else {
            let c = component(word(b, 28))?;
            let image = Identity::decode(&b[144..200], false)?;
            let active = committed[(component_number(c) - 1) as usize];
            if image.slot == active.slot
                || image.generation <= active.generation
                || attempts > MAX_ATTEMPTS
                || (phase == Phase::Pending && attempts != 0)
                || (phase == Phase::Trial && attempts == 0)
            {
                return Err(Invalid);
            }
            Some((c, image))
        };
        let revision = wide(b, 16);
        if revision == 1 {
            if b[256..] != [0; REQUEST_BYTES]
                || phase != Phase::Idle
                || committed != [Identity::INITIAL; 2]
            {
                return Err(Invalid);
            }
        } else {
            let r = &b[256..];
            let op = match word(r, 8) {
                2 => Operation::Stage,
                3 => Operation::Attempt,
                4 => Operation::Commit,
                5 => Operation::Abort,
                _ => return Err(Invalid),
            };
            if request(
                revision - 1,
                op,
                component(word(r, 24))?,
                Identity::decode(&r[32..88], false)?,
            )? != r
            {
                return Err(Invalid);
            }
            // A canonical request is not enough: the final state must also
            // describe its successful result. Otherwise a damaged record can
            // falsely acknowledge commitment while retaining another image.
            let c = component(word(r, 24))?;
            let image = Identity::decode(&r[32..88], false)?;
            let active = committed[(component_number(c) - 1) as usize];
            let consistent = match op {
                Operation::Stage => phase == Phase::Pending && pending == Some((c, image)),
                Operation::Attempt => phase == Phase::Trial && pending == Some((c, image)),
                Operation::Commit => phase == Phase::Idle && active == image,
                Operation::Abort => {
                    phase == Phase::Idle
                        && active.generation < image.generation
                        && active.slot != image.slot
                }
            };
            if !consistent {
                return Err(Invalid);
            }
        }
        Ok(Self {
            revision,
            committed,
            phase,
            pending,
            attempts,
            last_request: b[256..].try_into().unwrap(),
        })
    }
    pub fn committed(&self, c: Component) -> Identity {
        self.committed[(component_number(c) - 1) as usize]
    }
    /// The root still has to perform durable installation, trial execution,
    /// write fencing and health verification before submitting each operation.
    pub fn request(
        &self,
        op: Operation,
        c: Component,
        image: Identity,
    ) -> Result<[u8; REQUEST_BYTES], Invalid> {
        match op {
            Operation::Stage
                if self.pending.is_none()
                    && image.slot != self.committed(c).slot
                    && image.generation > self.committed(c).generation => {}
            Operation::Attempt
                if self.pending == Some((c, image)) && self.attempts < MAX_ATTEMPTS => {}
            Operation::Commit if self.pending == Some((c, image)) && self.phase == Phase::Trial => {
            }
            Operation::Abort if self.pending == Some((c, image)) => {}
            _ => return Err(Invalid),
        }
        request(self.revision, op, c, image)
    }
    /// Exact request identity is required to resolve a lost mutation reply.
    pub fn acknowledges(&self, r: &[u8; REQUEST_BYTES]) -> bool {
        self.last_request == *r && wide(r, 16).checked_add(1) == Some(self.revision)
    }
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn wide(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
