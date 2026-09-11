use alloc::string::String;
use alloc::vec::Vec;
use bexos_kernel_core::ipc::Capability;

use crate::manifest::{CapabilityMetadata, Lifecycle, Metadata, ServiceActivation, Visibility};
use crate::registry::{PublishedInterface, RegistryError};

const SNAPSHOT_MAGIC: &[u8; 4] = b"BASP";
const SNAPSHOT_VERSION: u16 = 4;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppdSnapshot {
    pub interfaces: Vec<PublishedInterface>,
}

impl AppdSnapshot {
    pub fn new(mut interfaces: Vec<PublishedInterface>) -> Self {
        interfaces.sort_by(compare_interfaces);
        Self { interfaces }
    }

    pub fn encode(&self) -> Result<Vec<u8>, SnapshotError> {
        let mut out = Vec::new();
        out.extend(SNAPSHOT_MAGIC);
        write_u16(&mut out, SNAPSHOT_VERSION);
        write_u32(&mut out, checked_len(self.interfaces.len())?);

        for interface in &self.interfaces {
            write_string(&mut out, &interface.provider_package)?;
            match &interface.instance_id {
                Some(instance_id) => {
                    out.push(1);
                    write_string(&mut out, instance_id)?;
                }
                None => out.push(0),
            }
            write_string(&mut out, &interface.name)?;
            write_string(&mut out, &interface.protocol)?;
            out.push(lifecycle_to_u8(interface.lifecycle));
            out.push(visibility_to_u8(interface.visibility));
            match &interface.bind_permission {
                Some(permission) => {
                    out.push(1);
                    write_string(&mut out, permission)?;
                }
                None => out.push(0),
            }
            write_u32(&mut out, checked_len(interface.metadata.len())?);
            for metadata in &interface.metadata {
                write_string(&mut out, &metadata.key)?;
                write_string(&mut out, &metadata.value)?;
            }
            write_u32(&mut out, checked_len(interface.capabilities.len())?);
            for capability in &interface.capabilities {
                write_string(&mut out, &capability.capability)?;
                match &capability.permission {
                    Some(permission) => {
                        out.push(1);
                        write_string(&mut out, permission)?;
                    }
                    None => out.push(0),
                }
                write_u32(&mut out, checked_len(capability.method_ordinals.len())?);
                for ordinal in &capability.method_ordinals {
                    write_u64(&mut out, *ordinal);
                }
            }
            write_u64(&mut out, interface.endpoint.object_id);
            write_u32(&mut out, interface.endpoint.rights);
            out.push(activation_to_u8(interface.activation));
            write_u32(&mut out, interface.idle_timeout_ms);
            match &interface.provider_process {
                Some(process) => {
                    out.push(1);
                    write_string(&mut out, process)?;
                }
                None => out.push(0),
            }
            out.push(interface.dormant as u8);
        }

        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let mut cursor = Cursor::new(bytes);
        if cursor.read_bytes(4)? != SNAPSHOT_MAGIC.as_slice() {
            return Err(SnapshotError::BadMagic);
        }
        let version = cursor.read_u16()?;
        if !(2..=SNAPSHOT_VERSION).contains(&version) {
            return Err(SnapshotError::UnsupportedVersion);
        }

        let interface_count = cursor.read_u32()? as usize;
        let mut interfaces = Vec::with_capacity(interface_count);
        for _ in 0..interface_count {
            let provider_package = cursor.read_string()?;
            let instance_id = if version >= 3 {
                match cursor.read_u8()? {
                    0 => None,
                    1 => Some(cursor.read_string()?),
                    _ => return Err(SnapshotError::InvalidPresenceFlag),
                }
            } else {
                None
            };
            let name = cursor.read_string()?;
            let protocol = cursor.read_string()?;
            let lifecycle = lifecycle_from_u8(cursor.read_u8()?)?;
            let visibility = visibility_from_u8(cursor.read_u8()?)?;
            let bind_permission = match cursor.read_u8()? {
                0 => None,
                1 => Some(cursor.read_string()?),
                _ => return Err(SnapshotError::InvalidPresenceFlag),
            };
            let metadata_count = cursor.read_u32()? as usize;
            let mut metadata = Vec::with_capacity(metadata_count);
            for _ in 0..metadata_count {
                metadata.push(Metadata {
                    key: cursor.read_string()?,
                    value: cursor.read_string()?,
                });
            }
            let capability_count = cursor.read_u32()? as usize;
            let mut capabilities = Vec::with_capacity(capability_count);
            for _ in 0..capability_count {
                let capability = cursor.read_string()?;
                let permission = match cursor.read_u8()? {
                    0 => None,
                    1 => Some(cursor.read_string()?),
                    _ => return Err(SnapshotError::InvalidPresenceFlag),
                };
                let ordinal_count = cursor.read_u32()? as usize;
                let mut method_ordinals = Vec::with_capacity(ordinal_count);
                for _ in 0..ordinal_count {
                    method_ordinals.push(cursor.read_u64()?);
                }
                capabilities.push(CapabilityMetadata {
                    capability,
                    permission,
                    method_ordinals,
                });
            }
            let endpoint = Capability {
                object_id: cursor.read_u64()?,
                rights: cursor.read_u32()?,
            };
            let (activation, idle_timeout_ms, provider_process, dormant) = if version >= 4 {
                let activation = activation_from_u8(cursor.read_u8()?)?;
                let idle_timeout_ms = cursor.read_u32()?;
                let provider_process = match cursor.read_u8()? {
                    0 => None,
                    1 => Some(cursor.read_string()?),
                    _ => return Err(SnapshotError::InvalidPresenceFlag),
                };
                let dormant = match cursor.read_u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(SnapshotError::InvalidPresenceFlag),
                };
                (activation, idle_timeout_ms, provider_process, dormant)
            } else {
                (ServiceActivation::Eager, 0, None, false)
            };

            interfaces.push(PublishedInterface {
                provider_package,
                instance_id,
                name,
                protocol,
                lifecycle,
                visibility,
                bind_permission,
                metadata,
                capabilities,
                endpoint,
                activation,
                idle_timeout_ms,
                provider_process,
                dormant,
            });
        }

        if !cursor.is_empty() {
            return Err(SnapshotError::TrailingBytes);
        }

        Ok(Self::new(interfaces))
    }

    pub fn validate_registry_shape(&self) -> Result<(), RegistryError> {
        let mut seen: Vec<(&str, Option<&str>, &str)> = Vec::new();
        let mut singleton_names: Vec<(&str, &str)> = Vec::new();
        for interface in &self.interfaces {
            if interface.name.is_empty() {
                return Err(RegistryError::EmptyName);
            }
            if interface.protocol.is_empty() {
                return Err(RegistryError::EmptyProtocol);
            }
            if seen.iter().any(|(provider, instance_id, name)| {
                *provider == interface.provider_package
                    && *instance_id == interface.instance_id.as_deref()
                    && *name == interface.name
            }) {
                return Err(RegistryError::DuplicateInterface {
                    provider_package: interface.provider_package.clone(),
                    name: interface.name.clone(),
                });
            }
            if interface.lifecycle == Lifecycle::Singleton {
                if let Some((existing_provider, _)) = singleton_names
                    .iter()
                    .find(|(_, name)| *name == interface.name)
                {
                    return Err(RegistryError::DuplicateSingleton {
                        name: interface.name.clone(),
                        existing_provider_package: String::from(*existing_provider),
                        provider_package: interface.provider_package.clone(),
                    });
                }
                singleton_names.push((&interface.provider_package, &interface.name));
            }
            seen.push((
                &interface.provider_package,
                interface.instance_id.as_deref(),
                &interface.name,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapshotError {
    BadMagic,
    UnsupportedVersion,
    UnexpectedEof,
    InvalidUtf8,
    InvalidLifecycle(u8),
    InvalidVisibility(u8),
    InvalidActivation(u8),
    InvalidPresenceFlag,
    ValueTooLarge,
    TrailingBytes,
}

fn activation_to_u8(activation: ServiceActivation) -> u8 {
    match activation {
        ServiceActivation::Eager => 0,
        ServiceActivation::Lazy => 1,
        ServiceActivation::Unspecified => 255,
    }
}

fn activation_from_u8(value: u8) -> Result<ServiceActivation, SnapshotError> {
    match value {
        0 => Ok(ServiceActivation::Eager),
        1 => Ok(ServiceActivation::Lazy),
        _ => Err(SnapshotError::InvalidActivation(value)),
    }
}

fn compare_interfaces(a: &PublishedInterface, b: &PublishedInterface) -> core::cmp::Ordering {
    a.provider_package
        .cmp(&b.provider_package)
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.protocol.cmp(&b.protocol))
}

fn checked_len(len: usize) -> Result<u32, SnapshotError> {
    len.try_into().map_err(|_| SnapshotError::ValueTooLarge)
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), SnapshotError> {
    write_u32(out, checked_len(value.len())?);
    out.extend(value.as_bytes());
    Ok(())
}

fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend(value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend(value.to_le_bytes());
}

fn lifecycle_to_u8(lifecycle: Lifecycle) -> u8 {
    match lifecycle {
        Lifecycle::Unspecified => 0,
        Lifecycle::Singleton => 1,
        Lifecycle::UserScopedSingleton => 2,
        Lifecycle::MultipleInstance => 3,
    }
}

fn lifecycle_from_u8(value: u8) -> Result<Lifecycle, SnapshotError> {
    match value {
        0 => Ok(Lifecycle::Unspecified),
        1 => Ok(Lifecycle::Singleton),
        2 => Ok(Lifecycle::UserScopedSingleton),
        3 => Ok(Lifecycle::MultipleInstance),
        _ => Err(SnapshotError::InvalidLifecycle(value)),
    }
}

fn visibility_to_u8(visibility: Visibility) -> u8 {
    match visibility {
        Visibility::Unspecified => 0,
        Visibility::Public => 1,
        Visibility::DomainShared => 2,
        Visibility::Private => 3,
    }
}

fn visibility_from_u8(value: u8) -> Result<Visibility, SnapshotError> {
    match value {
        0 => Ok(Visibility::Unspecified),
        1 => Ok(Visibility::Public),
        2 => Ok(Visibility::DomainShared),
        3 => Ok(Visibility::Private),
        _ => Err(SnapshotError::InvalidVisibility(value)),
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }

    fn read_u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, SnapshotError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, SnapshotError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, SnapshotError> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn read_string(&mut self) -> Result<String, SnapshotError> {
        let len = self.read_u32()? as usize;
        let bytes = self.read_bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| SnapshotError::InvalidUtf8)
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(SnapshotError::UnexpectedEof)?;
        if end > self.bytes.len() {
            return Err(SnapshotError::UnexpectedEof);
        }

        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
}
