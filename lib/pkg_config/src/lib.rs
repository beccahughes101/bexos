#![no_std]
extern crate alloc;
use alloc::{
    collections::BTreeSet,
    format,
    string::{String, ToString},
    vec::Vec,
};
use bexos_component_config::wire::{fields, varint};
use bexos_pkg_client::{ArtifactKind, ArtifactQuery, PackageStatus, valid_host, valid_repository};
type Result<T> = core::result::Result<T, PackageStatus>;
#[derive(Clone, Default)]
pub struct Config {
    pub repositories: Vec<Repository>,
    pub consumers: Vec<Consumer>,
    pub mappings: Vec<Mapping>,
    pub max_cache_bytes: u64,
    pub max_payload_bytes: u64,
    pub max_inflight: usize,
    pub max_waiters: usize,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
}
#[derive(Clone, Default)]
pub struct Repository {
    pub host: String,
    pub repository: String,
    pub trusted_root: Vec<u8>,
    pub token_origins: Vec<String>,
    pub redirect_origins: Vec<String>,
    pub tls_roots_der: Vec<Vec<u8>>,
}
impl Repository {
    pub fn id(&self) -> String {
        format!("{}/{}", self.host, self.repository)
    }
}
#[derive(Clone, Default)]
pub struct Consumer {
    pub package: String,
    pub kinds: Vec<u32>,
    pub repositories: Vec<String>,
}
#[derive(Clone)]
pub struct Mapping {
    pub name: String,
    pub query: ArtifactQuery,
    pub hardware: Option<HardwareSelector>,
    pub priority: u32,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HardwareSelector {
    pub pci_segment: Option<u16>,
    pub pci_bus: Option<u8>,
    pub pci_device: Option<u8>,
    pub pci_function: Option<u8>,
    pub pci_vendor_id: Option<u16>,
    pub pci_device_id: Option<u16>,
    pub pci_class: Option<u8>,
    pub pci_subclass: Option<u8>,
    pub pci_prog_if: Option<u8>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareIdentity {
    pub pci_segment: u16,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    pub pci_vendor_id: u16,
    pub pci_device_id: u16,
    pub pci_class: u8,
    pub pci_subclass: u8,
    pub pci_prog_if: u8,
}
impl Config {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(PackageStatus::InvalidArgs);
        }
        let mut config = Self {
            max_cache_bytes: 256 * 1024 * 1024,
            max_payload_bytes: 64 * 1024 * 1024,
            max_inflight: 16,
            max_waiters: 64,
            connect_timeout_ms: 10_000,
            request_timeout_ms: 30_000,
            ..Self::default()
        };
        for field in fields(bytes) {
            let (n, kind, value) = field.map_err(|_| PackageStatus::InvalidArgs)?;
            match (n, kind) {
                (1, 2) => config.repositories.push(repository(value)?),
                (2, 2) => config.consumers.push(consumer(value)?),
                (3, 2) => config.mappings.push(mapping(value)?),
                (4, 0) => config.max_cache_bytes = number(value)?,
                (5, 0) => config.max_payload_bytes = number(value)?,
                (6, 0) => config.max_inflight = number(value)? as usize,
                (7, 0) => config.max_waiters = number(value)? as usize,
                (8, 0) => config.connect_timeout_ms = number(value)?,
                (9, 0) => config.request_timeout_ms = number(value)?,
                _ => return Err(PackageStatus::InvalidArgs),
            }
        }
        if config.repositories.len() > 64
            || config.consumers.len() > 256
            || config.mappings.len() > 1024
            || config.max_payload_bytes == 0
            || config.max_payload_bytes > 256 * 1024 * 1024
            || config.max_cache_bytes < config.max_payload_bytes
            || config.max_cache_bytes > 16 * 1024 * 1024 * 1024
            || !(1..=128).contains(&config.max_inflight)
            || !(1..=64).contains(&config.max_waiters)
            || !(1..=120_000).contains(&config.connect_timeout_ms)
            || !(1..=600_000).contains(&config.request_timeout_ms)
        {
            return Err(PackageStatus::InvalidArgs);
        }
        let mut ids = BTreeSet::new();
        for repo in &config.repositories {
            if !ids.insert(repo.id()) {
                return Err(PackageStatus::InvalidArgs);
            }
        }
        let mut names = BTreeSet::new();
        for map in &config.mappings {
            if !names.insert((map.query.kind as u8, map.name.clone()))
                || config.repository(&map.query).is_none()
            {
                return Err(PackageStatus::InvalidArgs);
            }
        }
        for consumer in &config.consumers {
            if consumer.repositories.iter().any(|id| !ids.contains(id)) {
                return Err(PackageStatus::InvalidArgs);
            }
        }
        Ok(config)
    }
    pub fn repository(&self, query: &ArtifactQuery) -> Option<&Repository> {
        self.repositories
            .iter()
            .find(|repo| repo.host == query.registry_host && repo.repository == query.repository)
    }
    pub fn permits(&self, package: &str, repository: &str, kind: ArtifactKind) -> bool {
        self.consumers.iter().any(|consumer| {
            consumer.package == package
                && consumer.kinds.contains(&(kind as u32))
                && consumer.repositories.iter().any(|id| id == repository)
        })
    }

    /// Resolution order is exact BDF identity, vendor/device, then class.
    /// Priority only orders mappings within the same specificity tier.
    pub fn driver_mapping(&self, identity: HardwareIdentity) -> Option<&Mapping> {
        self.mappings
            .iter()
            .filter(|mapping| mapping.query.kind == ArtifactKind::Driver)
            .filter_map(|mapping| {
                let selector = mapping.hardware.as_ref()?;
                selector.matches(identity).then_some((
                    selector.specificity(),
                    mapping.priority,
                    mapping,
                ))
            })
            .max_by(
                |(specificity_a, priority_a, mapping_a), (specificity_b, priority_b, mapping_b)| {
                    specificity_a
                        .cmp(specificity_b)
                        .then_with(|| priority_a.cmp(priority_b))
                        .then_with(|| mapping_b.name.cmp(&mapping_a.name))
                },
            )
            .map(|(_, _, mapping)| mapping)
    }
}

impl HardwareSelector {
    fn valid(&self) -> bool {
        let has_bdf = self.pci_segment.is_some()
            || self.pci_bus.is_some()
            || self.pci_device.is_some()
            || self.pci_function.is_some();
        let full_bdf = self.pci_segment.is_some()
            && self.pci_bus.is_some()
            && self.pci_device.is_some()
            && self.pci_function.is_some();
        let has_id = self.pci_vendor_id.is_some() || self.pci_device_id.is_some();
        let full_id = self.pci_vendor_id.is_some() && self.pci_device_id.is_some();
        let has_class =
            self.pci_class.is_some() || self.pci_subclass.is_some() || self.pci_prog_if.is_some();
        (!has_bdf || full_bdf)
            && (!has_id || full_id)
            && (full_bdf || full_id || has_class)
            && self.pci_device.is_none_or(|value| value <= 31)
            && self.pci_function.is_none_or(|value| value <= 7)
    }

    fn matches(&self, identity: HardwareIdentity) -> bool {
        self.pci_segment
            .is_none_or(|value| value == identity.pci_segment)
            && self.pci_bus.is_none_or(|value| value == identity.pci_bus)
            && self
                .pci_device
                .is_none_or(|value| value == identity.pci_device)
            && self
                .pci_function
                .is_none_or(|value| value == identity.pci_function)
            && self
                .pci_vendor_id
                .is_none_or(|value| value == identity.pci_vendor_id)
            && self
                .pci_device_id
                .is_none_or(|value| value == identity.pci_device_id)
            && self
                .pci_class
                .is_none_or(|value| value == identity.pci_class)
            && self
                .pci_subclass
                .is_none_or(|value| value == identity.pci_subclass)
            && self
                .pci_prog_if
                .is_none_or(|value| value == identity.pci_prog_if)
    }

    fn specificity(&self) -> (u8, u8) {
        let exact = self.pci_segment.is_some()
            && self.pci_bus.is_some()
            && self.pci_device.is_some()
            && self.pci_function.is_some();
        let device = self.pci_vendor_id.is_some() && self.pci_device_id.is_some();
        let tier = if exact {
            3
        } else if device {
            2
        } else {
            1
        };
        let fields = [
            self.pci_segment.is_some(),
            self.pci_bus.is_some(),
            self.pci_device.is_some(),
            self.pci_function.is_some(),
            self.pci_vendor_id.is_some(),
            self.pci_device_id.is_some(),
            self.pci_class.is_some(),
            self.pci_subclass.is_some(),
            self.pci_prog_if.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count() as u8;
        (tier, fields)
    }
}
fn number(bytes: &[u8]) -> Result<u64> {
    varint(bytes)
        .map(|v| v.0)
        .map_err(|_| PackageStatus::InvalidArgs)
}
fn text(bytes: &[u8], limit: usize) -> Result<String> {
    if bytes.len() > limit {
        return Err(PackageStatus::InvalidArgs);
    }
    core::str::from_utf8(bytes)
        .map(ToString::to_string)
        .map_err(|_| PackageStatus::InvalidArgs)
}
fn repository(bytes: &[u8]) -> Result<Repository> {
    let mut repo = Repository::default();
    for field in fields(bytes) {
        let (n, kind, value) = field.map_err(|_| PackageStatus::InvalidArgs)?;
        if kind != 2 {
            return Err(PackageStatus::InvalidArgs);
        }
        match n {
            1 => repo.host = text(value, 128)?,
            2 => repo.repository = text(value, 128)?,
            3 => repo.trusted_root = value.to_vec(),
            4 => repo.token_origins.push(text(value, 256)?),
            5 => repo.redirect_origins.push(text(value, 256)?),
            6 if value.len() <= 4096 => repo.tls_roots_der.push(value.to_vec()),
            _ => return Err(PackageStatus::InvalidArgs),
        }
    }
    if !valid_host(&repo.host)
        || !valid_repository(&repo.repository)
        || repo.trusted_root.is_empty()
        || repo.trusted_root.len() > 1024 * 1024
        || repo.tls_roots_der.len() > 16
    {
        return Err(PackageStatus::InvalidArgs);
    }
    for origin in repo.token_origins.iter().chain(&repo.redirect_origins) {
        if !origin.strip_prefix("https://").is_some_and(valid_host) {
            return Err(PackageStatus::InvalidArgs);
        }
    }
    Ok(repo)
}
fn consumer(bytes: &[u8]) -> Result<Consumer> {
    let mut consumer = Consumer::default();
    for field in fields(bytes) {
        let (n, kind, mut value) = field.map_err(|_| PackageStatus::InvalidArgs)?;
        match (n, kind) {
            (1, 2) => consumer.package = text(value, 128)?,
            (2, 0) => consumer
                .kinds
                .push(u32::try_from(number(value)?).map_err(|_| PackageStatus::InvalidArgs)?),
            (2, 2) => {
                while !value.is_empty() {
                    let (number, used) = varint(value).map_err(|_| PackageStatus::InvalidArgs)?;
                    consumer
                        .kinds
                        .push(u32::try_from(number).map_err(|_| PackageStatus::InvalidArgs)?);
                    value = &value[used..];
                }
            }
            (3, 2) => consumer.repositories.push(text(value, 257)?),
            _ => return Err(PackageStatus::InvalidArgs),
        }
    }
    if consumer.package.is_empty() || consumer.kinds.iter().any(|kind| !(1..=5).contains(kind)) {
        return Err(PackageStatus::InvalidArgs);
    }
    Ok(consumer)
}
fn mapping(bytes: &[u8]) -> Result<Mapping> {
    let mut map = Mapping {
        name: String::new(),
        query: ArtifactQuery {
            registry_host: String::new(),
            repository: String::new(),
            tag: String::new(),
            expected_digest: None,
            kind: ArtifactKind::Application,
        },
        hardware: None,
        priority: 0,
    };
    for field in fields(bytes) {
        let (n, kind, value) = field.map_err(|_| PackageStatus::InvalidArgs)?;
        match (n, kind) {
            (1, 2) => map.name = text(value, 128)?,
            (2, 2) => map.query.registry_host = text(value, 128)?,
            (3, 2) => map.query.repository = text(value, 128)?,
            (4, 2) => map.query.tag = text(value, 64)?,
            (5, 0) => {
                map.query.kind = match number(value)? {
                    1 => ArtifactKind::Application,
                    2 => ArtifactKind::Driver,
                    3 => ArtifactKind::Font,
                    4 => ArtifactKind::Firmware,
                    5 => ArtifactKind::NetworkExtension,
                    6 => ArtifactKind::Container,
                    _ => return Err(PackageStatus::InvalidArgs),
                }
            }
            (6, 2) => map.hardware = Some(hardware_selector(value)?),
            (7, 0) => {
                map.priority =
                    u32::try_from(number(value)?).map_err(|_| PackageStatus::InvalidArgs)?
            }
            _ => return Err(PackageStatus::InvalidArgs),
        }
    }
    map.query.validate()?;
    if map.name.is_empty()
        || map.priority > 1_000_000
        || map
            .hardware
            .as_ref()
            .is_some_and(|selector| !selector.valid())
        || (map.hardware.is_some() && map.query.kind != ArtifactKind::Driver)
    {
        return Err(PackageStatus::InvalidArgs);
    }
    Ok(map)
}

fn hardware_selector(bytes: &[u8]) -> Result<HardwareSelector> {
    let mut selector = HardwareSelector::default();
    for field in fields(bytes) {
        let (n, kind, value) = field.map_err(|_| PackageStatus::InvalidArgs)?;
        if kind != 0 {
            return Err(PackageStatus::InvalidArgs);
        }
        let value = number(value)?;
        match n {
            1 => {
                selector.pci_segment =
                    Some(u16::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            2 => {
                selector.pci_bus =
                    Some(u8::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            3 => {
                selector.pci_device =
                    Some(u8::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            4 => {
                selector.pci_function =
                    Some(u8::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            5 => {
                selector.pci_vendor_id =
                    Some(u16::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            6 => {
                selector.pci_device_id =
                    Some(u16::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            7 => {
                selector.pci_class =
                    Some(u8::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            8 => {
                selector.pci_subclass =
                    Some(u8::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            9 => {
                selector.pci_prog_if =
                    Some(u8::try_from(value).map_err(|_| PackageStatus::InvalidArgs)?)
            }
            _ => return Err(PackageStatus::InvalidArgs),
        }
    }
    if selector.valid() {
        Ok(selector)
    } else {
        Err(PackageStatus::InvalidArgs)
    }
}
