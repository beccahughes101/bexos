#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
mod checkpoint;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bexos_app_archive::{OpenArchive, TrustedKey};
pub use bexos_package_version::{
    HealthCheckStatus, MultiVersionPolicy, PackageSelector, SemVer, VersionMatchError,
    parse_package_selector, select_unique_match, versioned_package_key,
};

pub use bexos_app_manifest::{Architecture, ManifestArchitecture};

pub const PACKAGE_MANIFEST_PATH: &str = "package.bexmanifest";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallSource {
    Bootfs,
    SystemImage,
    Debugd,
    Oci,
}

impl InstallSource {
    #[cfg(feature = "redb_backend")]
    fn to_u8(self) -> u8 {
        match self {
            Self::Bootfs => 1,
            Self::SystemImage => 2,
            Self::Debugd => 3,
            Self::Oci => 4,
        }
    }

    #[cfg(feature = "redb_backend")]
    fn from_u8(value: u8) -> Result<Self, RegistryError> {
        match value {
            1 => Ok(Self::Bootfs),
            2 => Ok(Self::SystemImage),
            3 => Ok(Self::Debugd),
            4 => Ok(Self::Oci),
            _ => Err(RegistryError::CorruptRecord),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    Installed,
    Launching,
    Running,
    Stopped,
}

impl LifecycleState {
    #[cfg(feature = "redb_backend")]
    fn to_u8(self) -> u8 {
        match self {
            Self::Installed => 1,
            Self::Launching => 2,
            Self::Running => 3,
            Self::Stopped => 4,
        }
    }

    #[cfg(feature = "redb_backend")]
    fn from_u8(value: u8) -> Result<Self, RegistryError> {
        match value {
            1 => Ok(Self::Installed),
            2 => Ok(Self::Launching),
            3 => Ok(Self::Running),
            4 => Ok(Self::Stopped),
            _ => Err(RegistryError::CorruptRecord),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppRecord {
    pub package_id: String,
    pub display_name: String,
    pub manifest_bytes: Vec<u8>,
    pub version: SemVer,
    pub multi_version_policy: MultiVersionPolicy,
    pub min_bexos_abi_version: u32,
    pub archive_content_root: [u8; 32],
    pub signer_key_id: [u8; 32],
    pub verified_signer: Option<VerifiedSignerMetadata>,
    pub install_source: InstallSource,
    pub lifecycle_state: LifecycleState,
    pub protected: bool,
    pub archive_path: String,
    pub package_instance_id: u64,
    pub accepted_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSignerMetadata {
    pub root_anchor_id: String,
    pub leaf_certificate_fingerprint: [u8; 32],
    pub signature_algorithm: u8,
    pub granted_trust_tier: u8,
}

impl AppRecord {
    /// Signed records are retained verbatim, but incompatible native content is
    /// unavailable to selectors, dependencies, launch and replacement.
    pub fn validate_architecture(&self) -> Result<(), RegistryError> {
        ManifestArchitecture::decode(&self.manifest_bytes)
            .and_then(|manifest| manifest.validate(Some(Architecture::current_guest())))
            .map(|_| ())
            .map_err(RegistryError::Architecture)
    }
    pub fn architecture_diagnostic(&self) -> Option<&'static str> {
        self.validate_architecture().err().map(|_| "native package architecture is missing or incompatible; rebuild and reinstall for this device")
    }

    pub fn package_key(&self) -> String {
        versioned_package_key(&self.package_id, &self.version)
    }

    pub fn archive_id(&self) -> String {
        self.archive_path
            .strip_prefix("pkg/")
            .and_then(|path| path.strip_suffix(".bex"))
            .unwrap_or(&self.package_key())
            .to_string()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActivePinRecord {
    pub package_id: String,
    pub pinned_version: SemVer,
    pub content_blake3: [u8; 32],
    pub is_critical_boot_app: bool,
    pub health_check_status: HealthCheckStatus,
    pub rollback_target_version: Option<SemVer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallRequest<'a> {
    pub archive_bytes: &'a [u8],
    pub trusted_keys: &'a [TrustedKey<'a>],
    pub verified_signer: Option<VerifiedSignerMetadata>,
    pub source: InstallSource,
    pub protected: bool,
    pub archive_path: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    Archive,
    MissingManifest,
    ManifestDecode,
    PackageMismatch,
    EmptyPackage,
    InvalidPackage,
    NotFound,
    ProtectedPackage,
    CorruptRecord,
    Storage,
    AmbiguousPackage,
    Architecture(bexos_app_manifest::Error),
}

fn selector_error_to_registry(error: VersionMatchError) -> RegistryError {
    match error {
        VersionMatchError::InvalidSelector => RegistryError::InvalidPackage,
        VersionMatchError::NotFound => RegistryError::NotFound,
        VersionMatchError::Ambiguous => RegistryError::AmbiguousPackage,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryAppRegistry {
    records: Vec<AppRecord>,
    active_pins: Vec<ActivePinRecord>,
}

impl MemoryAppRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn install_bundle(
        &mut self,
        request: InstallRequest<'_>,
    ) -> Result<AppRecord, RegistryError> {
        let record = record_from_bundle(request)?;
        self.upsert_checked(record.clone())?;
        Ok(record)
    }

    pub fn import_boot_bundle(
        &mut self,
        archive_bytes: &[u8],
        trusted_keys: &[TrustedKey<'_>],
        archive_path: &str,
    ) -> Result<AppRecord, RegistryError> {
        self.install_bundle(InstallRequest {
            archive_bytes,
            trusted_keys,
            source: InstallSource::Bootfs,
            protected: true,
            archive_path,
            verified_signer: Some(product_trust_metadata()),
        })
    }

    pub fn import_manifest_cache(
        &mut self,
        manifest_bytes: &[u8],
        source: InstallSource,
        protected: bool,
        archive_path: &str,
    ) -> Result<AppRecord, RegistryError> {
        let (package_id, display_name, version, multi_version_policy, min_bexos_abi_version) =
            decode_manifest_header(manifest_bytes)?;
        validate_package_id(&package_id)?;
        let archive_content_root = [0; 32];
        let package_instance_id =
            installation_instance_id(manifest_bytes, [0; 32], [0; 32], archive_path);
        let record = AppRecord {
            package_id,
            display_name,
            manifest_bytes: manifest_bytes.to_vec(),
            version,
            multi_version_policy,
            min_bexos_abi_version,
            archive_content_root,
            signer_key_id: [0; 32],
            verified_signer: if protected {
                Some(product_trust_metadata())
            } else {
                None
            },
            install_source: source,
            lifecycle_state: LifecycleState::Installed,
            protected,
            archive_path: archive_path.to_string(),
            package_instance_id,
            accepted_generation: 0,
        };
        self.upsert_checked(record.clone())?;
        Ok(record)
    }

    pub fn uninstall_package(&mut self, package_id: &str) -> Result<AppRecord, RegistryError> {
        let record = self.record_for_removal(package_id)?.clone();
        let index = self
            .find_version_index(&record.package_id, &record.version)
            .ok_or(RegistryError::NotFound)?;
        if self.records[index].protected {
            return Err(RegistryError::ProtectedPackage);
        }
        if self
            .active_pin(&record.package_id)
            .is_some_and(|pin| pin.pinned_version == record.version)
        {
            self.active_pins
                .retain(|pin| pin.package_id != record.package_id);
        }
        Ok(self.records.remove(index))
    }

    pub fn list_packages(&self) -> &[AppRecord] {
        &self.records
    }

    pub fn active_pins(&self) -> &[ActivePinRecord] {
        &self.active_pins
    }

    pub fn replace_active_pins(&mut self, pins: Vec<ActivePinRecord>) -> Result<(), RegistryError> {
        validate_active_pins(&self.records, &pins)?;
        self.active_pins = pins;
        self.active_pins
            .sort_by(|left, right| left.package_id.cmp(&right.package_id));
        Ok(())
    }

    pub fn load_manifest(&self, package_id: &str) -> Result<&[u8], RegistryError> {
        Ok(&self.record(package_id)?.manifest_bytes)
    }

    pub fn mark_lifecycle(
        &mut self,
        package_id: &str,
        state: LifecycleState,
    ) -> Result<(), RegistryError> {
        let record = self.record(package_id)?.clone();
        let index = self
            .find_version_index(&record.package_id, &record.version)
            .ok_or(RegistryError::NotFound)?;
        self.records[index].lifecycle_state = state;
        Ok(())
    }

    pub fn record(&self, package_id: &str) -> Result<&AppRecord, RegistryError> {
        let record = self.record_for_removal(package_id)?;
        record.validate_architecture()?;
        Ok(record)
    }

    /// Resolve removal metadata without making an incompatible installation
    /// available for launch. Signed manifests remain unchanged on disk.
    pub fn record_for_removal(&self, package_id: &str) -> Result<&AppRecord, RegistryError> {
        let selector =
            parse_package_selector(package_id, None).map_err(selector_error_to_registry)?;
        if selector.version_requirement.is_none() {
            if let Some(pin) = self.active_pin(&selector.package) {
                let index = self
                    .find_version_index(&pin.package_id, &pin.pinned_version)
                    .ok_or(RegistryError::NotFound)?;
                return Ok(&self.records[index]);
            }
        }
        let record = select_unique_match(&self.records, &selector, |record| {
            (record.package_id.as_str(), &record.version)
        })
        .map_err(selector_error_to_registry)?;
        Ok(record)
    }

    #[cfg(feature = "redb_backend")]
    pub fn install_checkpoint_record(&mut self, record: AppRecord) {
        self.install_decoded(record);
    }

    fn upsert(&mut self, record: AppRecord) {
        if let Some(index) = self.find_version_index(&record.package_id, &record.version) {
            self.records[index] = record;
        } else {
            self.records.push(record);
            self.records.sort_by(|left, right| {
                left.package_id
                    .cmp(&right.package_id)
                    .then_with(|| left.version.cmp(&right.version))
            });
        }
    }

    fn upsert_checked(&mut self, record: AppRecord) -> Result<(), RegistryError> {
        if let Some(index) = self.find_version_index(&record.package_id, &record.version) {
            if self.records[index].protected && !record.protected {
                return Err(RegistryError::ProtectedPackage);
            }
        }
        if record.multi_version_policy == MultiVersionPolicy::SingleActiveOnly {
            self.pin_active_from_record(&record);
        }
        self.upsert(record);
        Ok(())
    }

    fn find_version_index(&self, package_id: &str, version: &SemVer) -> Option<usize> {
        self.records
            .iter()
            .position(|record| record.package_id == package_id && record.version == *version)
    }

    fn record_version(
        &self,
        package_id: &str,
        version: &SemVer,
    ) -> Result<&AppRecord, RegistryError> {
        let record = self
            .find_version_index(package_id, version)
            .map(|index| &self.records[index])
            .ok_or(RegistryError::NotFound)?;
        record.validate_architecture()?;
        Ok(record)
    }

    pub fn active_pin(&self, package_id: &str) -> Option<&ActivePinRecord> {
        self.active_pins
            .iter()
            .find(|pin| pin.package_id == package_id)
    }

    pub fn pin_active_version(
        &mut self,
        package_id: &str,
        version: SemVer,
    ) -> Result<(), RegistryError> {
        let record = self.record_version(package_id, &version)?.clone();
        self.pin_active_from_record(&record);
        Ok(())
    }

    pub fn clear_rollback_target(&mut self, package_id: &str) -> Result<(), RegistryError> {
        let Some(pin) = self
            .active_pins
            .iter_mut()
            .find(|pin| pin.package_id == package_id)
        else {
            return Err(RegistryError::NotFound);
        };
        pin.rollback_target_version = None;
        Ok(())
    }

    pub fn mark_pin_health(
        &mut self,
        package_id: &str,
        status: HealthCheckStatus,
    ) -> Result<(), RegistryError> {
        let Some(pin) = self
            .active_pins
            .iter_mut()
            .find(|pin| pin.package_id == package_id)
        else {
            return Err(RegistryError::NotFound);
        };
        pin.health_check_status = status;
        Ok(())
    }

    pub fn rollback_to_previous(&mut self, package_id: &str) -> Result<SemVer, RegistryError> {
        let index = self
            .active_pins
            .iter()
            .position(|pin| pin.package_id == package_id)
            .ok_or(RegistryError::NotFound)?;
        let rollback = self.active_pins[index]
            .rollback_target_version
            .clone()
            .ok_or(RegistryError::NotFound)?;
        let record = self.record_version(package_id, &rollback)?.clone();
        self.active_pins[index].pinned_version = rollback.clone();
        self.active_pins[index].content_blake3 = record.archive_content_root;
        self.active_pins[index].health_check_status = HealthCheckStatus::Healthy;
        self.active_pins[index].rollback_target_version = None;
        Ok(rollback)
    }

    pub fn remove_version(
        &mut self,
        package_id: &str,
        version: &SemVer,
    ) -> Result<AppRecord, RegistryError> {
        let index = self
            .find_version_index(package_id, version)
            .ok_or(RegistryError::NotFound)?;
        if self.records[index].protected {
            return Err(RegistryError::ProtectedPackage);
        }
        if self
            .active_pin(package_id)
            .is_some_and(|pin| pin.pinned_version == *version)
        {
            return Err(RegistryError::ProtectedPackage);
        }
        for pin in &mut self.active_pins {
            if pin.package_id == package_id
                && pin
                    .rollback_target_version
                    .as_ref()
                    .is_some_and(|rollback| rollback == version)
            {
                pin.rollback_target_version = None;
            }
        }
        Ok(self.records.remove(index))
    }

    pub fn prune_inactive_candidates(
        &self,
        package_id: &str,
        keep_last_n: u32,
    ) -> Result<Vec<AppRecord>, RegistryError> {
        validate_package_id(package_id)?;
        let active = self
            .active_pin(package_id)
            .map(|pin| pin.pinned_version.clone());
        let protected_rollback = self.active_pin(package_id).and_then(|pin| {
            (pin.health_check_status == HealthCheckStatus::Probation)
                .then(|| pin.rollback_target_version.clone())
                .flatten()
        });
        let mut inactive = self
            .records
            .iter()
            .filter(|record| record.package_id == package_id)
            .filter(|record| Some(&record.version) != active.as_ref())
            .filter(|record| Some(&record.version) != protected_rollback.as_ref())
            .cloned()
            .collect::<Vec<_>>();
        inactive.sort_by(|left, right| right.version.cmp(&left.version));
        Ok(inactive.into_iter().skip(keep_last_n as usize).collect())
    }

    fn pin_active_from_record(&mut self, record: &AppRecord) {
        let rollback_target_version = self
            .active_pin(&record.package_id)
            .map(|pin| pin.pinned_version.clone())
            .filter(|version| *version != record.version);
        let pin = ActivePinRecord {
            package_id: record.package_id.clone(),
            pinned_version: record.version.clone(),
            content_blake3: record.archive_content_root,
            is_critical_boot_app: record.protected,
            health_check_status: if record.protected {
                HealthCheckStatus::Probation
            } else {
                HealthCheckStatus::Healthy
            },
            rollback_target_version,
        };
        if let Some(index) = self
            .active_pins
            .iter()
            .position(|existing| existing.package_id == record.package_id)
        {
            self.active_pins[index] = pin;
        } else {
            self.active_pins.push(pin);
            self.active_pins
                .sort_by(|left, right| left.package_id.cmp(&right.package_id));
        }
    }
}

fn validate_active_pins(
    records: &[AppRecord],
    pins: &[ActivePinRecord],
) -> Result<(), RegistryError> {
    let mut sorted = pins.to_vec();
    sorted.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    if sorted
        .windows(2)
        .any(|pair| pair[0].package_id == pair[1].package_id)
    {
        return Err(RegistryError::CorruptRecord);
    }
    for pin in pins {
        let record = records
            .iter()
            .find(|record| {
                record.package_id == pin.package_id && record.version == pin.pinned_version
            })
            .ok_or(RegistryError::NotFound)?;
        if record.archive_content_root != pin.content_blake3 {
            return Err(RegistryError::CorruptRecord);
        }
        if pin.is_critical_boot_app && !record.protected {
            return Err(RegistryError::CorruptRecord);
        }
        if let Some(rollback) = &pin.rollback_target_version {
            records
                .iter()
                .find(|record| record.package_id == pin.package_id && record.version == *rollback)
                .ok_or(RegistryError::NotFound)?;
        }
    }
    Ok(())
}

pub fn record_from_bundle(request: InstallRequest<'_>) -> Result<AppRecord, RegistryError> {
    validate_archive_path(request.archive_path)?;
    let archive = OpenArchive::parse_and_verify(request.archive_bytes, request.trusted_keys)
        .map_err(|_| RegistryError::Archive)?;
    let manifest_entry = archive
        .find(PACKAGE_MANIFEST_PATH)
        .ok_or(RegistryError::MissingManifest)?;
    let manifest_bytes = archive
        .read_file(manifest_entry)
        .map_err(|_| RegistryError::Archive)?;
    let (package_id, display_name, version, multi_version_policy, min_bexos_abi_version) =
        decode_manifest_header(&manifest_bytes)?;
    validate_package_id(&package_id)?;
    let architecture = ManifestArchitecture::decode(&manifest_bytes)
        .and_then(|m| m.validate(Some(Architecture::current_guest())))
        .map_err(RegistryError::Architecture)?;
    for entry in archive.entries() {
        if entry.path == PACKAGE_MANIFEST_PATH {
            continue;
        }
        let bytes = archive
            .read_file(entry)
            .map_err(|_| RegistryError::Archive)?;
        bexos_app_manifest::validate_payload(architecture, &bytes)
            .map_err(RegistryError::Architecture)?;
    }
    let package_instance_id = installation_instance_id(
        &manifest_bytes,
        archive.content_root(),
        archive.key_id(),
        request.archive_path,
    );
    let Some(verified_signer) = request.verified_signer else {
        return Err(RegistryError::Archive);
    };
    Ok(AppRecord {
        package_id,
        display_name,
        manifest_bytes,
        version,
        multi_version_policy,
        min_bexos_abi_version,
        archive_content_root: archive.content_root(),
        signer_key_id: archive.key_id(),
        verified_signer: Some(verified_signer),
        install_source: request.source,
        lifecycle_state: LifecycleState::Installed,
        protected: request.protected,
        archive_path: request.archive_path.to_string(),
        package_instance_id,
        accepted_generation: 0,
    })
}

pub fn product_trust_metadata() -> VerifiedSignerMetadata {
    VerifiedSignerMetadata {
        root_anchor_id: "bexos.product-image".into(),
        leaf_certificate_fingerprint: [0; 32],
        signature_algorithm: 0,
        granted_trust_tier: 0,
    }
}

pub fn validate_package_id(package_id: &str) -> Result<(), RegistryError> {
    if package_id.is_empty() {
        return Err(RegistryError::EmptyPackage);
    }
    if package_id.len() > 128
        || package_id
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_' && b != b':')
    {
        return Err(RegistryError::InvalidPackage);
    }
    Ok(())
}

fn validate_archive_path(path: &str) -> Result<(), RegistryError> {
    if path.is_empty() || path.len() > 256 || path.starts_with('/') || path.contains("..") {
        return Err(RegistryError::InvalidPackage);
    }
    Ok(())
}

fn decode_manifest_header(
    bytes: &[u8],
) -> Result<(String, String, SemVer, MultiVersionPolicy, u32), RegistryError> {
    ManifestArchitecture::decode(bytes)
        .and_then(|m| m.validate(Some(Architecture::current_guest())))
        .map_err(RegistryError::Architecture)?;
    decode_manifest_header_unchecked(bytes)
}

fn decode_manifest_header_unchecked(
    bytes: &[u8],
) -> Result<(String, String, SemVer, MultiVersionPolicy, u32), RegistryError> {
    let mut package_id = String::new();
    let mut display_name = String::new();
    let mut version = SemVer::default();
    let mut policy = MultiVersionPolicy::SingleActiveOnly;
    let mut min_bexos_abi_version = 0;
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => package_id = field.string()?,
            2 => display_name = field.string()?,
            13 => version = decode_semver(field.bytes()?)?,
            14 => policy = MultiVersionPolicy::from_proto(field.varint()?),
            15 => min_bexos_abi_version = field.varint()? as u32,
            _ => {}
        }
    }
    if display_name.is_empty() {
        display_name = package_id.clone();
    }
    Ok((
        package_id,
        display_name,
        version,
        policy,
        min_bexos_abi_version,
    ))
}

fn decode_semver(bytes: &[u8]) -> Result<SemVer, RegistryError> {
    let mut version = SemVer::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => version.major = field.varint()? as u32,
            2 => version.minor = field.varint()? as u32,
            3 => version.patch = field.varint()? as u32,
            4 => version.build = field.varint()? as u32,
            5 => version.prerelease = field.string()?,
            _ => {}
        }
    }
    Ok(version)
}

#[cfg(feature = "redb_backend")]
fn encode_record(record: &AppRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &record.package_id);
    put_string(&mut out, 2, &record.display_name);
    put_bytes(&mut out, 3, &record.manifest_bytes);
    put_bytes(&mut out, 4, &record.archive_content_root);
    put_bytes(&mut out, 5, &record.signer_key_id);
    put_varint(&mut out, 6, record.install_source.to_u8() as u64);
    put_varint(&mut out, 7, record.lifecycle_state.to_u8() as u64);
    put_varint(&mut out, 8, record.protected as u64);
    put_string(&mut out, 9, &record.archive_path);
    put_bytes(&mut out, 10, &encode_semver(&record.version));
    put_varint(&mut out, 11, record.multi_version_policy.to_u8() as u64);
    put_varint(&mut out, 12, record.min_bexos_abi_version as u64);
    put_varint(&mut out, 13, record.package_instance_id);
    if let Some(signer) = &record.verified_signer {
        put_bytes(&mut out, 14, &encode_verified_signer(signer));
    }
    put_varint(&mut out, 15, record.accepted_generation);
    out
}

#[cfg(feature = "redb_backend")]
fn decode_record(bytes: &[u8]) -> Result<AppRecord, RegistryError> {
    let mut record = AppRecord {
        package_id: String::new(),
        display_name: String::new(),
        manifest_bytes: Vec::new(),
        version: SemVer::default(),
        multi_version_policy: MultiVersionPolicy::SingleActiveOnly,
        min_bexos_abi_version: 0,
        archive_content_root: [0; 32],
        signer_key_id: [0; 32],
        verified_signer: None,
        install_source: InstallSource::Debugd,
        lifecycle_state: LifecycleState::Installed,
        protected: false,
        archive_path: String::new(),
        package_instance_id: 0,
        accepted_generation: 0,
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => record.package_id = field.string()?,
            2 => record.display_name = field.string()?,
            3 => record.manifest_bytes = field.bytes()?.to_vec(),
            4 => record
                .archive_content_root
                .copy_from_slice(exact_32(field.bytes()?)?),
            5 => record
                .signer_key_id
                .copy_from_slice(exact_32(field.bytes()?)?),
            6 => record.install_source = InstallSource::from_u8(field.varint()? as u8)?,
            7 => record.lifecycle_state = LifecycleState::from_u8(field.varint()? as u8)?,
            8 => record.protected = field.varint()? != 0,
            9 => record.archive_path = field.string()?,
            10 => record.version = decode_semver(field.bytes()?)?,
            11 => {
                record.multi_version_policy = MultiVersionPolicy::from_u8(field.varint()? as u8)
                    .ok_or(RegistryError::CorruptRecord)?
            }
            12 => record.min_bexos_abi_version = field.varint()? as u32,
            13 => record.package_instance_id = field.varint()?,
            14 => record.verified_signer = Some(decode_verified_signer(field.bytes()?)?),
            15 => record.accepted_generation = field.varint()?,
            _ => {}
        }
    }
    validate_package_id(&record.package_id)?;
    if record.package_instance_id == 0 {
        record.package_instance_id = installation_instance_id(
            &record.manifest_bytes,
            record.archive_content_root,
            record.signer_key_id,
            &record.archive_path,
        );
    }
    // Decode durable records without rewriting their signed manifest. Availability
    // is checked by record selection, so one old installation cannot prevent boot.
    Ok(record)
}

#[cfg(feature = "redb_backend")]
fn encode_verified_signer(signer: &VerifiedSignerMetadata) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &signer.root_anchor_id);
    put_bytes(&mut out, 2, &signer.leaf_certificate_fingerprint);
    put_varint(&mut out, 3, signer.signature_algorithm as u64);
    put_varint(&mut out, 4, signer.granted_trust_tier as u64);
    out
}

#[cfg(feature = "redb_backend")]
fn decode_verified_signer(bytes: &[u8]) -> Result<VerifiedSignerMetadata, RegistryError> {
    let mut signer = VerifiedSignerMetadata {
        root_anchor_id: String::new(),
        leaf_certificate_fingerprint: [0; 32],
        signature_algorithm: 0,
        granted_trust_tier: u8::MAX,
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => signer.root_anchor_id = field.string()?,
            2 => signer
                .leaf_certificate_fingerprint
                .copy_from_slice(exact_32(field.bytes()?)?),
            3 => signer.signature_algorithm = field.varint()? as u8,
            4 => signer.granted_trust_tier = field.varint()? as u8,
            _ => {}
        }
    }
    if signer.root_anchor_id.is_empty() {
        return Err(RegistryError::CorruptRecord);
    }
    Ok(signer)
}

fn installation_instance_id(
    manifest_bytes: &[u8],
    content_root: [u8; 32],
    signer_key_id: [u8; 32],
    archive_path: &str,
) -> u64 {
    let mut value = 0xcbf29ce484222325u64;
    for byte in manifest_bytes
        .iter()
        .copied()
        .chain(content_root)
        .chain(signer_key_id)
        .chain(archive_path.bytes())
    {
        value ^= byte as u64;
        value = value.wrapping_mul(0x100000001b3);
    }
    value.max(1)
}

#[cfg(feature = "redb_backend")]
fn exact_32(bytes: &[u8]) -> Result<&[u8; 32], RegistryError> {
    bytes.try_into().map_err(|_| RegistryError::CorruptRecord)
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{
        ActivePinRecord, AppRecord, InstallRequest, LifecycleState, MemoryAppRegistry,
        RegistryError, decode_record, encode_record, record_from_bundle,
    };
    use alloc::vec::Vec;
    use bexos_redb::{BlockStore, open_or_create_with_store};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

    const APPS: TableDefinition<&str, &[u8]> = TableDefinition::new("apps");
    const PINS: TableDefinition<&str, &[u8]> = TableDefinition::new("active_pins");

    pub struct AppRegistryDb {
        db: Database,
    }

    impl AppRegistryDb {
        pub fn open<S: BlockStore>(store: S) -> Result<Self, RegistryError> {
            Ok(Self {
                db: open_or_create_with_store(store).map_err(|_| RegistryError::Storage)?,
            })
        }

        pub fn install_bundle(
            &self,
            request: InstallRequest<'_>,
        ) -> Result<AppRecord, RegistryError> {
            let record = record_from_bundle(request)?;
            self.check_replace_allowed(&record)?;
            let mut memory = self.snapshot_memory()?;
            memory.upsert_checked(record.clone())?;
            self.replace_from_memory(&memory)?;
            Ok(record)
        }

        pub fn import_boot_bundle(
            &self,
            archive_bytes: &[u8],
            trusted_keys: &[bexos_app_archive::TrustedKey<'_>],
            archive_path: &str,
        ) -> Result<AppRecord, RegistryError> {
            self.install_bundle(InstallRequest {
                archive_bytes,
                trusted_keys,
                verified_signer: Some(super::product_trust_metadata()),
                source: super::InstallSource::Bootfs,
                protected: true,
                archive_path,
            })
        }

        pub fn import_manifest_cache(
            &self,
            manifest_bytes: &[u8],
            source: super::InstallSource,
            protected: bool,
            archive_path: &str,
        ) -> Result<AppRecord, RegistryError> {
            let mut registry = MemoryAppRegistry::new();
            let record =
                registry.import_manifest_cache(manifest_bytes, source, protected, archive_path)?;
            let mut memory = self.snapshot_memory()?;
            memory.upsert_checked(record.clone())?;
            self.replace_from_memory(&memory)?;
            Ok(record)
        }

        pub fn uninstall_package(&self, package_id: &str) -> Result<AppRecord, RegistryError> {
            let mut memory = self.snapshot_memory()?;
            let record = memory.uninstall_package(package_id)?;
            self.replace_from_memory(&memory)?;
            Ok(record)
        }

        pub fn list_packages(&self) -> Result<Vec<AppRecord>, RegistryError> {
            let tx = self.db.begin_read().map_err(|_| RegistryError::Storage)?;
            let Ok(table) = tx.open_table(APPS) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| RegistryError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| RegistryError::Storage)?;
                    decode_record(value.value())
                })
                .collect()
        }

        pub fn load_manifest(&self, package_id: &str) -> Result<Vec<u8>, RegistryError> {
            Ok(self.record(package_id)?.manifest_bytes)
        }

        pub fn mark_lifecycle(
            &self,
            package_id: &str,
            state: LifecycleState,
        ) -> Result<(), RegistryError> {
            let mut record = self.record(package_id)?;
            record.lifecycle_state = state;
            self.put_record(&record)
        }

        pub fn pin_active_version(
            &self,
            package_id: &str,
            version: super::SemVer,
        ) -> Result<(), RegistryError> {
            let mut memory = self.snapshot_memory()?;
            memory.pin_active_version(package_id, version)?;
            self.replace_from_memory(&memory)
        }

        pub fn rollback_to_previous(
            &self,
            package_id: &str,
        ) -> Result<super::SemVer, RegistryError> {
            let mut memory = self.snapshot_memory()?;
            let version = memory.rollback_to_previous(package_id)?;
            self.replace_from_memory(&memory)?;
            Ok(version)
        }

        pub fn record(&self, package_id: &str) -> Result<AppRecord, RegistryError> {
            self.snapshot_memory()?.record(package_id).cloned()
        }

        pub fn snapshot_memory(&self) -> Result<MemoryAppRegistry, RegistryError> {
            let mut registry = MemoryAppRegistry::new();
            for record in self.list_packages()? {
                registry.upsert(record);
            }
            let pins = self.list_pins()?;
            if pins.is_empty() {
                let records = registry.records.clone();
                for record in records {
                    if record.multi_version_policy == super::MultiVersionPolicy::SingleActiveOnly {
                        registry.pin_active_from_record(&record);
                    }
                }
            } else {
                registry.replace_active_pins(pins)?;
            }
            Ok(registry)
        }

        pub fn replace_from_memory(&self, memory: &MemoryAppRegistry) -> Result<(), RegistryError> {
            let tx = self.db.begin_write().map_err(|_| RegistryError::Storage)?;
            {
                let mut table = tx.open_table(APPS).map_err(|_| RegistryError::Storage)?;
                table
                    .retain(|_, _| false)
                    .map_err(|_| RegistryError::Storage)?;
                for record in memory.list_packages() {
                    let bytes = encode_record(record);
                    table
                        .insert(record.package_key().as_str(), bytes.as_slice())
                        .map_err(|_| RegistryError::Storage)?;
                }
            }
            {
                let mut table = tx.open_table(PINS).map_err(|_| RegistryError::Storage)?;
                table
                    .retain(|_, _| false)
                    .map_err(|_| RegistryError::Storage)?;
                for pin in memory.active_pins() {
                    let bytes = pin.checkpoint();
                    table
                        .insert(pin.package_id.as_str(), bytes.as_slice())
                        .map_err(|_| RegistryError::Storage)?;
                }
            }
            tx.commit().map_err(|_| RegistryError::Storage)
        }

        fn list_pins(&self) -> Result<Vec<ActivePinRecord>, RegistryError> {
            let tx = self.db.begin_read().map_err(|_| RegistryError::Storage)?;
            let Ok(table) = tx.open_table(PINS) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| RegistryError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| RegistryError::Storage)?;
                    ActivePinRecord::from_checkpoint(value.value())
                        .map_err(|_| RegistryError::CorruptRecord)
                })
                .collect()
        }

        pub fn commit_active_record(&self, record: &AppRecord) -> Result<(), RegistryError> {
            let mut current = self.snapshot_memory()?;
            let existing = current
                .record(&record.package_id)
                .map_err(|_| RegistryError::NotFound)?;
            if existing.protected && !record.protected {
                return Err(RegistryError::ProtectedPackage);
            }
            if record.accepted_generation < existing.accepted_generation {
                return Err(RegistryError::CorruptRecord);
            }
            current
                .replace_checkpoint_record(record.clone())
                .map_err(|_| RegistryError::CorruptRecord)?;
            // A replacement and its active content pin are one durable change.
            // Publishing only APPS makes the next registry snapshot corrupt.
            let tx = self.db.begin_write().map_err(|_| RegistryError::Storage)?;
            {
                let bytes = encode_record(record);
                let mut table = tx.open_table(APPS).map_err(|_| RegistryError::Storage)?;
                table
                    .insert(record.package_key().as_str(), bytes.as_slice())
                    .map_err(|_| RegistryError::Storage)?;
            }
            if let Some(pin) = current.active_pin(&record.package_id) {
                let bytes = pin.checkpoint();
                let mut table = tx.open_table(PINS).map_err(|_| RegistryError::Storage)?;
                table
                    .insert(pin.package_id.as_str(), bytes.as_slice())
                    .map_err(|_| RegistryError::Storage)?;
            }
            tx.commit().map_err(|_| RegistryError::Storage)
        }

        fn put_record(&self, record: &AppRecord) -> Result<(), RegistryError> {
            let bytes = encode_record(record);
            let tx = self.db.begin_write().map_err(|_| RegistryError::Storage)?;
            {
                let mut table = tx.open_table(APPS).map_err(|_| RegistryError::Storage)?;
                table
                    .insert(record.package_key().as_str(), bytes.as_slice())
                    .map_err(|_| RegistryError::Storage)?;
            }
            tx.commit().map_err(|_| RegistryError::Storage)
        }

        fn check_replace_allowed(&self, record: &AppRecord) -> Result<(), RegistryError> {
            match self.snapshot_memory()?.record(&record.package_key()) {
                Ok(existing) if existing.protected && !record.protected => {
                    Err(RegistryError::ProtectedPackage)
                }
                Ok(_) | Err(RegistryError::NotFound) => Ok(()),
                Err(error) => Err(error),
            }
        }
    }
}

impl MemoryAppRegistry {
    #[cfg(feature = "redb_backend")]
    fn install_decoded(&mut self, record: AppRecord) {
        if record.multi_version_policy == MultiVersionPolicy::SingleActiveOnly {
            self.pin_active_from_record(&record);
        }
        self.upsert(record);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Field<'a> {
    number: u32,
    wire_type: u8,
    value: &'a [u8],
}

impl<'a> Field<'a> {
    fn string(self) -> Result<String, RegistryError> {
        if self.wire_type != 2 {
            return Err(RegistryError::ManifestDecode);
        }
        let value = core::str::from_utf8(self.value).map_err(|_| RegistryError::ManifestDecode)?;
        Ok(value.to_string())
    }

    fn bytes(self) -> Result<&'a [u8], RegistryError> {
        if self.wire_type != 2 {
            return Err(RegistryError::ManifestDecode);
        }
        Ok(self.value)
    }

    fn varint(self) -> Result<u64, RegistryError> {
        if self.wire_type != 0 {
            return Err(RegistryError::ManifestDecode);
        }
        let mut value = 0u64;
        for (shift, byte) in self.value.iter().enumerate() {
            value |= u64::from(byte & 0x7f) << (shift * 7);
        }
        Ok(value)
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

    fn next_field(&mut self) -> Result<Option<Field<'a>>, RegistryError> {
        if self.offset == self.bytes.len() {
            return Ok(None);
        }
        let key = self.read_varint()?;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x7) as u8;
        match wire_type {
            0 => {
                let start = self.offset;
                self.read_varint()?;
                Ok(Some(Field {
                    number,
                    wire_type,
                    value: &self.bytes[start..self.offset],
                }))
            }
            2 => {
                let len = self.read_varint()? as usize;
                let start = self.offset;
                let end = start
                    .checked_add(len)
                    .ok_or(RegistryError::ManifestDecode)?;
                if end > self.bytes.len() {
                    return Err(RegistryError::ManifestDecode);
                }
                self.offset = end;
                Ok(Some(Field {
                    number,
                    wire_type,
                    value: &self.bytes[start..end],
                }))
            }
            _ => Err(RegistryError::ManifestDecode),
        }
    }

    fn read_varint(&mut self) -> Result<u64, RegistryError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(RegistryError::ManifestDecode)?;
            self.offset += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(RegistryError::ManifestDecode)
    }
}

#[cfg(feature = "redb_backend")]
fn put_string(out: &mut Vec<u8>, field: u32, value: &str) {
    put_bytes(out, field, value.as_bytes());
}

#[cfg(feature = "redb_backend")]
fn put_bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_raw_varint(out, u64::from(field << 3 | 2));
    put_raw_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

#[cfg(feature = "redb_backend")]
fn put_varint(out: &mut Vec<u8>, field: u32, value: u64) {
    put_raw_varint(out, u64::from(field << 3));
    put_raw_varint(out, value);
}

#[cfg(feature = "redb_backend")]
fn put_raw_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(feature = "redb_backend")]
fn encode_semver(version: &SemVer) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, version.major as u64);
    put_varint(&mut out, 2, version.minor as u64);
    put_varint(&mut out, 3, version.patch as u64);
    put_varint(&mut out, 4, version.build as u64);
    put_string(&mut out, 5, &version.prerelease);
    out
}
