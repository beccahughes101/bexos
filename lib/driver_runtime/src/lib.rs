#![no_std]
extern crate alloc;

use alloc::{string::String, vec::Vec};
use bexos_migration::{
    Error as MigrationError,
    codec::{Decoder, Encoder},
};

pub const MAX_INSTANCES_PER_HOST: u32 = 64;
pub const MIGRATION_RECORD_VERSION: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ResourceKind {
    Mmio,
    Interrupt,
    DmaPool,
    IommuDomain,
    RegisterProxy,
    BusControl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resource {
    pub kind: ResourceKind,
    pub resource_id: u64,
    pub base: u64,
    pub length: u64,
    pub flags: u64,
    pub handle: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceRequirement {
    pub kind: ResourceKind,
    pub minimum: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindContext {
    pub node_id: u64,
    pub parent_node_id: Option<u64>,
    pub topological_path: String,
    pub resources: Vec<Resource>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColocationPolicy {
    Isolated,
    Colocated,
    HostShared,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostPolicy {
    pub package_id: String,
    pub process_name: String,
    pub signed_package_digest: [u8; 32],
    pub colocation: ColocationPolicy,
    pub max_instances: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodePhase {
    Binding,
    Ready,
    Quiesced,
    Restoring,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedNode {
    pub node_id: u64,
    pub parent_node_id: Option<u64>,
    pub phase: NodePhase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostMembership {
    pub host_id: u64,
    pub policy: HostPolicy,
    nodes: Vec<HostedNode>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    InvalidContext,
    MissingResource(ResourceKind),
    DuplicateResource,
    Capacity,
    PolicyMismatch,
    ParentNotReady,
    DuplicateNode,
    UnknownNode,
    InvalidTransition,
}

impl BindContext {
    pub fn validate(&self, requirements: &[ResourceRequirement]) -> Result<(), RuntimeError> {
        if self.node_id == 0
            || self.topological_path.is_empty()
            || self.topological_path.len() > 256
            || self.resources.len() > 16
            || self.resources.iter().any(|resource| resource.handle == 0)
        {
            return Err(RuntimeError::InvalidContext);
        }
        for (index, resource) in self.resources.iter().enumerate() {
            if self.resources[..index]
                .iter()
                .any(|prior| prior.resource_id == resource.resource_id)
            {
                return Err(RuntimeError::DuplicateResource);
            }
        }
        for requirement in requirements {
            if requirement.minimum == 0
                || self
                    .resources
                    .iter()
                    .filter(|resource| resource.kind == requirement.kind)
                    .count()
                    < requirement.minimum as usize
            {
                return Err(RuntimeError::MissingResource(requirement.kind));
            }
        }
        Ok(())
    }
}

impl HostPolicy {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.package_id.is_empty()
            || self.process_name.is_empty()
            || !(1..=MAX_INSTANCES_PER_HOST).contains(&self.max_instances)
            || (self.colocation == ColocationPolicy::Isolated && self.max_instances != 1)
        {
            Err(RuntimeError::PolicyMismatch)
        } else {
            Ok(())
        }
    }
}

impl HostMembership {
    pub fn new(host_id: u64, policy: HostPolicy) -> Result<Self, RuntimeError> {
        policy.validate()?;
        Ok(Self {
            host_id,
            policy,
            nodes: Vec::new(),
        })
    }

    pub fn nodes(&self) -> &[HostedNode] {
        &self.nodes
    }

    pub fn can_host(
        &self,
        candidate: &HostPolicy,
        node_id: u64,
        parent_node_id: Option<u64>,
    ) -> Result<(), RuntimeError> {
        candidate.validate()?;
        if candidate.package_id != self.policy.package_id
            || candidate.process_name != self.policy.process_name
            || candidate.signed_package_digest != self.policy.signed_package_digest
        {
            return Err(RuntimeError::PolicyMismatch);
        }
        if self.nodes.iter().any(|node| node.node_id == node_id) {
            return Err(RuntimeError::DuplicateNode);
        }
        match candidate.colocation {
            ColocationPolicy::Isolated => {
                if self.nodes.is_empty() {
                    Ok(())
                } else {
                    Err(RuntimeError::Capacity)
                }
            }
            ColocationPolicy::Colocated => {
                if self.policy.colocation != ColocationPolicy::Colocated
                    || candidate.max_instances != self.policy.max_instances
                {
                    return Err(RuntimeError::PolicyMismatch);
                }
                if self.nodes.len() >= candidate.max_instances as usize {
                    Err(RuntimeError::Capacity)
                } else {
                    Ok(())
                }
            }
            ColocationPolicy::HostShared => {
                let parent = parent_node_id.ok_or(RuntimeError::ParentNotReady)?;
                if self.nodes.len() >= candidate.max_instances as usize {
                    return Err(RuntimeError::Capacity);
                }
                if self
                    .nodes
                    .iter()
                    .any(|node| node.node_id == parent && node.phase == NodePhase::Ready)
                {
                    Ok(())
                } else {
                    Err(RuntimeError::ParentNotReady)
                }
            }
        }
    }

    pub fn bind(
        &mut self,
        candidate: &HostPolicy,
        node_id: u64,
        parent_node_id: Option<u64>,
    ) -> Result<(), RuntimeError> {
        self.can_host(candidate, node_id, parent_node_id)?;
        self.nodes.push(HostedNode {
            node_id,
            parent_node_id,
            phase: NodePhase::Binding,
        });
        Ok(())
    }

    pub fn set_ready(&mut self, node_id: u64) -> Result<(), RuntimeError> {
        let node = self
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
            .ok_or(RuntimeError::UnknownNode)?;
        if !matches!(node.phase, NodePhase::Binding | NodePhase::Restoring) {
            return Err(RuntimeError::InvalidTransition);
        }
        node.phase = NodePhase::Ready;
        Ok(())
    }

    /// Host failures and planned replacement quiesce every member before any
    /// one node is restored, which makes colocated recovery atomic.
    pub fn quiesce_all(&mut self) {
        for node in &mut self.nodes {
            node.phase = NodePhase::Quiesced;
        }
    }

    pub fn begin_restore_all(&mut self) -> Result<(), RuntimeError> {
        if self
            .nodes
            .iter()
            .any(|node| node.phase != NodePhase::Quiesced)
        {
            return Err(RuntimeError::InvalidTransition);
        }
        for node in &mut self.nodes {
            node.phase = NodePhase::Restoring;
        }
        Ok(())
    }

    pub fn remove(&mut self, node_id: u64) -> Result<HostedNode, RuntimeError> {
        let index = self
            .nodes
            .iter()
            .position(|node| node.node_id == node_id)
            .ok_or(RuntimeError::UnknownNode)?;
        if self
            .nodes
            .iter()
            .any(|node| node.parent_node_id == Some(node_id))
        {
            return Err(RuntimeError::InvalidTransition);
        }
        Ok(self.nodes.remove(index))
    }

    pub fn encode_migration_record(&self, generation: u64) -> Vec<u8> {
        let mut writer = Encoder::new();
        writer.word(MIGRATION_RECORD_VERSION);
        writer.word(generation);
        writer.word(self.host_id);
        writer.text(&self.policy.package_id);
        writer.text(&self.policy.process_name);
        writer.bytes(&self.policy.signed_package_digest);
        writer.word(match self.policy.colocation {
            ColocationPolicy::Isolated => 1,
            ColocationPolicy::Colocated => 2,
            ColocationPolicy::HostShared => 3,
        });
        writer.word(self.policy.max_instances as u64);
        writer.word(self.nodes.len() as u64);
        for node in &self.nodes {
            writer.word(node.node_id);
            writer.word(node.parent_node_id.is_some() as u64);
            writer.word(node.parent_node_id.unwrap_or(0));
            writer.word(match node.phase {
                NodePhase::Binding => 1,
                NodePhase::Ready => 2,
                NodePhase::Quiesced => 3,
                NodePhase::Restoring => 4,
            });
        }
        writer.finish()
    }

    pub fn decode_migration_record(bytes: &[u8]) -> Result<(u64, Self), MigrationError> {
        let mut reader = Decoder::new(bytes);
        if reader.word()? != MIGRATION_RECORD_VERSION {
            return Err(MigrationError::UnsupportedVersion);
        }
        let generation = reader.word()?;
        let host_id = reader.word()?;
        let package_id = String::from(reader.text(128)?);
        let process_name = String::from(reader.text(128)?);
        let signed_package_digest = reader
            .bytes(32)?
            .try_into()
            .map_err(|_| MigrationError::InvalidData)?;
        let colocation = match reader.word()? {
            1 => ColocationPolicy::Isolated,
            2 => ColocationPolicy::Colocated,
            3 => ColocationPolicy::HostShared,
            _ => return Err(MigrationError::InvalidData),
        };
        let max_instances =
            u32::try_from(reader.word()?).map_err(|_| MigrationError::InvalidData)?;
        let policy = HostPolicy {
            package_id,
            process_name,
            signed_package_digest,
            colocation,
            max_instances,
        };
        policy.validate().map_err(|_| MigrationError::InvalidData)?;
        let mut nodes = Vec::new();
        for _ in 0..reader.count(MAX_INSTANCES_PER_HOST as usize)? {
            let node_id = reader.word()?;
            let has_parent = reader.flag()?;
            let parent = reader.word()?;
            let phase = match reader.word()? {
                1 => NodePhase::Binding,
                2 => NodePhase::Ready,
                3 => NodePhase::Quiesced,
                4 => NodePhase::Restoring,
                _ => return Err(MigrationError::InvalidData),
            };
            if node_id == 0
                || nodes
                    .iter()
                    .any(|node: &HostedNode| node.node_id == node_id)
            {
                return Err(MigrationError::InvalidData);
            }
            nodes.push(HostedNode {
                node_id,
                parent_node_id: has_parent.then_some(parent),
                phase,
            });
        }
        reader.finish()?;
        if nodes.len() > policy.max_instances as usize {
            return Err(MigrationError::InvalidData);
        }
        Ok((
            generation,
            Self {
                host_id,
                policy,
                nodes,
            },
        ))
    }
}
