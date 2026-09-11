//! Admission and identity rules independent of process-local GPU mappings.
use crate::{Capabilities, Error, FEATURE_BLOB, FEATURE_CONTEXT_INIT, FEATURE_VIRGL};
use alloc::vec::Vec;
use bexos_migration::{
    Error as MigrationError,
    codec::{Decoder, Encoder},
};

pub const MAX_CONTEXTS: usize = 16;
pub const MAX_RESOURCES: usize = 64;
pub const MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const VENUS_FEATURES: u64 = FEATURE_VIRGL | FEATURE_BLOB | FEATURE_CONTEXT_INIT;
pub const ACCESS_PLATFORM: u64 = 1 << 33;

pub fn negotiated_features(offered: u64) -> u64 {
    (1 << 32)
        | (offered & ACCESS_PLATFORM)
        | if offered & VENUS_FEATURES == VENUS_FEATURES {
            VENUS_FEATURES
        } else {
            0
        }
}
pub fn supported(caps: &Capabilities) -> bool {
    caps.venus_offered() && caps.negotiated & VENUS_FEATURES == VENUS_FEATURES
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub id: u32,
    pub owner: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub id: u32,
    pub context: u32,
    pub size: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registry {
    pub contexts: Vec<Context>,
    pub resources: Vec<Resource>,
    next_context: u32,
    next_resource: u32,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            contexts: Vec::with_capacity(MAX_CONTEXTS),
            resources: Vec::with_capacity(MAX_RESOURCES),
            next_context: 1,
            next_resource: 3,
        }
    }
}
impl Registry {
    pub fn context(&self, owner: u64, id: u32) -> Result<(), Error> {
        self.contexts
            .iter()
            .any(|c| c.id == id && c.owner == owner)
            .then_some(())
            .ok_or(Error::Invalid)
    }
    pub fn create_context(&mut self, owner: u64) -> Result<u32, Error> {
        if owner == 0 {
            return Err(Error::Invalid);
        }
        if self.contexts.len() == MAX_CONTEXTS {
            return Err(Error::Capacity);
        }
        let next = self.next_context.checked_add(1).ok_or(Error::Capacity)?;
        let id = self.next_context;
        self.contexts.push(Context { id, owner });
        self.next_context = next;
        Ok(id)
    }
    pub fn remove_context(&mut self, owner: u64, id: u32) -> Result<(), Error> {
        self.context(owner, id)?;
        if self.resources.iter().any(|r| r.context == id) {
            return Err(Error::Invalid);
        }
        self.contexts.retain(|c| c.id != id);
        Ok(())
    }
    pub fn create_resource(&mut self, owner: u64, context: u32, size: u64) -> Result<u32, Error> {
        self.context(owner, context)?;
        if size == 0 || size > MAX_RESOURCE_BYTES || size % 4096 != 0 {
            return Err(Error::Invalid);
        }
        if self.resources.len() == MAX_RESOURCES
            || self.resources.iter().map(|r| r.size).sum::<u64>() + size > MAX_TOTAL_BYTES
        {
            return Err(Error::Capacity);
        }
        let id = self.reserve_resource_id()?;
        self.resources.push(Resource { id, context, size });
        Ok(id)
    }
    /// A single monotonic namespace is shared with imported 2D scanout resources.
    /// Reserving an ID does not publish a device resource or consume Venus quota.
    pub fn reserve_resource_id(&mut self) -> Result<u32, Error> {
        let next = self.next_resource.checked_add(1).ok_or(Error::Capacity)?;
        let id = self.next_resource;
        self.next_resource = next;
        Ok(id)
    }
    pub fn was_reserved(&self, id: u32) -> bool {
        (3..self.next_resource).contains(&id)
    }
    pub fn resource(&self, owner: u64, context: u32, id: u32) -> Result<Resource, Error> {
        self.context(owner, context)?;
        self.resources
            .iter()
            .find(|r| r.context == context && r.id == id)
            .copied()
            .ok_or(Error::Invalid)
    }
    pub fn remove_resource(&mut self, owner: u64, context: u32, id: u32) -> Result<(), Error> {
        self.resource(owner, context, id)?;
        self.resources.retain(|r| r.id != id);
        Ok(())
    }
    pub fn encode(&self, w: &mut Encoder) {
        w.word(1);
        w.word(self.next_context as u64);
        w.word(self.next_resource as u64);
        w.word(self.contexts.len() as u64);
        for c in &self.contexts {
            w.word(c.id as u64);
            w.word(c.owner);
        }
        w.word(self.resources.len() as u64);
        for r in &self.resources {
            w.word(r.id as u64);
            w.word(r.context as u64);
            w.word(r.size);
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, MigrationError> {
        if r.word()? != 1 {
            return Err(MigrationError::UnsupportedVersion);
        }
        fn id(r: &mut Decoder<'_>) -> Result<u32, MigrationError> {
            r.word()?
                .try_into()
                .map_err(|_| MigrationError::InvalidData)
        }
        let mut out = Self {
            next_context: id(r)?,
            next_resource: id(r)?,
            ..Self::default()
        };
        if out.next_context == 0 || out.next_resource < 3 {
            return Err(MigrationError::InvalidData);
        }
        for _ in 0..r.count(MAX_CONTEXTS)? {
            let c = Context {
                id: id(r)?,
                owner: r.word()?,
            };
            if c.id == 0
                || c.id >= out.next_context
                || c.owner == 0
                || out.contexts.iter().any(|v| v.id == c.id)
            {
                return Err(MigrationError::InvalidData);
            }
            out.contexts.push(c);
        }
        let mut bytes = 0u64;
        for _ in 0..r.count(MAX_RESOURCES)? {
            let resource = Resource {
                id: id(r)?,
                context: id(r)?,
                size: r.word()?,
            };
            if resource.id < 3
                || resource.id >= out.next_resource
                || resource.size == 0
                || resource.size > MAX_RESOURCE_BYTES
                || resource.size % 4096 != 0
                || !out.contexts.iter().any(|c| c.id == resource.context)
                || out.resources.iter().any(|v| v.id == resource.id)
            {
                return Err(MigrationError::InvalidData);
            }
            bytes += resource.size;
            if bytes > MAX_TOTAL_BYTES {
                return Err(MigrationError::InvalidData);
            }
            out.resources.push(resource);
        }
        Ok(out)
    }
}
