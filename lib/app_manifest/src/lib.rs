//! Architecture policy for compiled, signed package manifests.
#![no_std]
extern crate alloc;
mod wire;
use wire::{fields, varint};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Architecture {
    #[default]
    Multi = 0,
    Aarch64 = 1,
    X86_64 = 2,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Malformed,
    UnknownArchitecture,
    ConflictingArchitecture,
    NativeArchitectureRequired,
    IncompatibleArchitecture,
    InvalidElf,
    InvalidPackage,
    IncompatibleAbi,
    MissingHeartTransplant,
    InvalidComponentType,
    InvalidBootWave,
    InvalidDriver,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdkComponentMetadata {
    pub package_name: alloc::string::String,
    pub min_bexos_abi_version: u32,
    pub component_type: SdkComponentType,
    pub boot_wave: Option<u32>,
}

pub type SdkAppMetadata = SdkComponentMetadata;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SdkComponentType {
    Application,
    Service,
    Driver,
}

pub fn validate_sdk_app(
    bytes: &[u8],
    expected_package_name: &str,
    maximum_abi_version: u32,
) -> Result<SdkAppMetadata, Error> {
    let metadata = validate_sdk_component_contract(
        bytes,
        maximum_abi_version,
        SdkComponentType::Application,
        None,
    )?;
    if metadata.package_name != expected_package_name {
        return Err(Error::InvalidPackage);
    }
    Ok(metadata)
}

pub fn validate_sdk_app_contract(
    bytes: &[u8],
    maximum_abi_version: u32,
) -> Result<SdkAppMetadata, Error> {
    validate_sdk_component_contract(
        bytes,
        maximum_abi_version,
        SdkComponentType::Application,
        None,
    )
}

pub fn validate_sdk_component(
    bytes: &[u8],
    expected_package_name: &str,
    maximum_abi_version: u32,
    component_type: SdkComponentType,
    boot_wave: Option<u32>,
) -> Result<SdkComponentMetadata, Error> {
    let metadata =
        validate_sdk_component_contract(bytes, maximum_abi_version, component_type, boot_wave)?;
    if metadata.package_name != expected_package_name {
        return Err(Error::InvalidPackage);
    }
    Ok(metadata)
}

pub fn validate_sdk_component_contract(
    bytes: &[u8],
    maximum_abi_version: u32,
    component_type: SdkComponentType,
    boot_wave: Option<u32>,
) -> Result<SdkComponentMetadata, Error> {
    let mut package_name: Option<alloc::string::String> = None;
    let mut package_kind = 0u64;
    let mut min_abi = 0u32;
    let mut process_count = 0usize;
    let mut service_count = 0usize;
    let mut declared_wave = None;
    let mut driver_package_id: Option<alloc::string::String> = None;
    let mut bind_rules = 0usize;
    for field in fields(bytes) {
        let (number, kind, value) = field?;
        match number {
            1 if kind == 2 => {
                package_name = Some(
                    core::str::from_utf8(value)
                        .map_err(|_| Error::Malformed)?
                        .into(),
                );
            }
            3 if kind == 2 => {
                process_count += 1;
                let process = validate_sdk_process(value)?;
                if process.service {
                    service_count += 1;
                    if let Some(prior) = declared_wave {
                        if process.wave != Some(prior) {
                            return Err(Error::InvalidBootWave);
                        }
                    } else {
                        declared_wave = process.wave;
                    }
                }
            }
            8 if kind == 2 => {
                driver_package_id = Some(validate_driver_info(value)?);
            }
            9 if kind == 2 => {
                validate_bind_rule(value)?;
                bind_rules += 1;
            }
            11 if kind == 0 => package_kind = varint(value)?.0,
            15 if kind == 0 => {
                min_abi = u32::try_from(varint(value)?.0).map_err(|_| Error::Malformed)?;
            }
            1 | 3 | 8 | 9 | 11 | 15 => return Err(Error::Malformed),
            _ => {}
        }
    }
    let package_name = package_name.ok_or(Error::InvalidPackage)?;
    if package_kind != 0 {
        return Err(Error::InvalidPackage);
    }
    if min_abi == 0 || min_abi > maximum_abi_version {
        return Err(Error::IncompatibleAbi);
    }
    match component_type {
        SdkComponentType::Application if driver_package_id.is_some() || bind_rules != 0 => {
            return Err(Error::InvalidComponentType);
        }
        SdkComponentType::Service
            if driver_package_id.is_some()
                || bind_rules != 0
                || service_count == 0
                || service_count != process_count =>
        {
            return Err(Error::InvalidComponentType);
        }
        SdkComponentType::Driver
            if driver_package_id.as_deref() != Some(package_name.as_str())
                || bind_rules == 0
                || process_count != 1
                || service_count != 1 =>
        {
            return Err(Error::InvalidDriver);
        }
        _ => {}
    }
    if let Some(expected) = boot_wave {
        if declared_wave != Some(expected) {
            return Err(Error::InvalidBootWave);
        }
    }
    Ok(SdkComponentMetadata {
        package_name,
        min_bexos_abi_version: min_abi,
        component_type,
        boot_wave: declared_wave,
    })
}

struct SdkProcess {
    service: bool,
    wave: Option<u32>,
}

fn validate_sdk_process(bytes: &[u8]) -> Result<SdkProcess, Error> {
    let mut service = false;
    let mut update_strategy = 0u64;
    let mut wave = None;
    let mut runner = None;
    let mut executable = None;
    for field in fields(bytes) {
        let (number, kind, value) = field?;
        match number {
            4 if kind == 0 => service = varint(value)?.0 != 0,
            2 if kind == 2 => runner = Some(value),
            7 if kind == 2 => {
                for option in fields(value) {
                    let (number, kind, value) = option?;
                    if number == 2 {
                        if kind != 2 {
                            return Err(Error::Malformed);
                        }
                        for elf in fields(value) {
                            let (number, kind, value) = elf?;
                            if number == 1 {
                                if kind != 2 {
                                    return Err(Error::Malformed);
                                }
                                executable = Some(
                                    core::str::from_utf8(value).map_err(|_| Error::Malformed)?,
                                );
                            }
                        }
                    }
                }
            }
            9 if kind == 2 => {
                for lifecycle_field in fields(value) {
                    let (number, kind, value) = lifecycle_field?;
                    if number == 1 {
                        if kind != 0 {
                            return Err(Error::Malformed);
                        }
                        update_strategy = varint(value)?.0;
                    }
                }
            }
            8 if kind == 0 => {
                wave = Some(u32::try_from(varint(value)?.0).map_err(|_| Error::Malformed)?);
            }
            2 | 4 | 7 | 8 | 9 => return Err(Error::Malformed),
            _ => {}
        }
    }
    if runner.is_some_and(|value| value.eq_ignore_ascii_case(b"elf"))
        && !executable.is_some_and(|path| path.starts_with("/pkg/bin/") && !path.contains(".."))
    {
        return Err(Error::InvalidPackage);
    }
    if service && update_strategy != 1 {
        Err(Error::MissingHeartTransplant)
    } else {
        Ok(SdkProcess { service, wave })
    }
}

fn validate_driver_info(bytes: &[u8]) -> Result<alloc::string::String, Error> {
    let mut name = false;
    let mut package_id = None;
    let mut max_instances = 0u64;
    let mut resources = 0usize;
    for field in fields(bytes) {
        let (number, kind, value) = field?;
        match number {
            1 if kind == 2 => name = !value.is_empty(),
            2 if kind == 2 => {
                package_id = Some(
                    core::str::from_utf8(value)
                        .map_err(|_| Error::Malformed)?
                        .into(),
                )
            }
            4 if kind == 2 => {
                for execution in fields(value) {
                    let (number, kind, value) = execution?;
                    if number == 2 {
                        if kind != 0 {
                            return Err(Error::Malformed);
                        }
                        max_instances = varint(value)?.0;
                    }
                }
            }
            5 if kind == 2 => {
                resources += 1;
                let mut resource_kind = 0u64;
                let mut minimum = 0u64;
                for resource in fields(value) {
                    let (number, kind, value) = resource?;
                    match number {
                        1 if kind == 0 => resource_kind = varint(value)?.0,
                        2 if kind == 0 => minimum = varint(value)?.0,
                        1 | 2 => return Err(Error::Malformed),
                        _ => {}
                    }
                }
                if !(1..=6).contains(&resource_kind) || !(1..=16).contains(&minimum) {
                    return Err(Error::InvalidDriver);
                }
            }
            1 | 2 | 4 | 5 => return Err(Error::Malformed),
            _ => {}
        }
    }
    if !name || !(1..=64).contains(&max_instances) || !(1..=16).contains(&resources) {
        return Err(Error::InvalidDriver);
    }
    package_id.ok_or(Error::InvalidDriver)
}

fn validate_bind_rule(bytes: &[u8]) -> Result<(), Error> {
    let mut conditions = 0usize;
    for field in fields(bytes) {
        let (number, kind, value) = field?;
        if number != 1 {
            continue;
        }
        if kind != 2 {
            return Err(Error::Malformed);
        }
        conditions += 1;
        let mut bus = 0u64;
        let mut properties = 0usize;
        for condition in fields(value) {
            let (number, kind, value) = condition?;
            match number {
                1 if kind == 0 => bus = varint(value)?.0,
                2 if kind == 2 => {
                    properties += 1;
                    let mut key = false;
                    for property in fields(value) {
                        let (number, kind, value) = property?;
                        if number == 1 {
                            if kind != 2 {
                                return Err(Error::Malformed);
                            }
                            key = !value.is_empty();
                        }
                    }
                    if !key {
                        return Err(Error::InvalidDriver);
                    }
                }
                1 | 2 => return Err(Error::Malformed),
                _ => {}
            }
        }
        if !(1..=5).contains(&bus) || properties == 0 {
            return Err(Error::InvalidDriver);
        }
    }
    if conditions == 0 {
        return Err(Error::InvalidDriver);
    }
    Ok(())
}
impl Architecture {
    pub const fn from_proto(value: u64) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Multi),
            1 => Ok(Self::Aarch64),
            2 => Ok(Self::X86_64),
            _ => Err(Error::UnknownArchitecture),
        }
    }
    pub const fn current_guest() -> Self {
        if cfg!(bexos_arch_x86_64) {
            Self::X86_64
        } else {
            Self::Aarch64
        }
    }
    pub const fn compatible_with(self, target: Self) -> bool {
        matches!(self, Self::Multi) || self as u8 == target as u8
    }
    pub const fn elf_machine(self) -> Option<u16> {
        match self {
            Self::Multi => None,
            Self::Aarch64 => Some(183),
            Self::X86_64 => Some(62),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestArchitecture {
    pub declared: Option<Architecture>,
    pub native: bool,
}
impl ManifestArchitecture {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut result = Self {
            declared: None,
            native: false,
        };
        for field in fields(bytes) {
            let (number, kind, value) = field?;
            match number {
                20 => {
                    if kind != 0 || result.declared.is_some() {
                        return Err(Error::Malformed);
                    }
                    result.declared = Some(Architecture::from_proto(varint(value)?.0)?);
                }
                16 | 19 => {
                    if kind != 2 {
                        return Err(Error::Malformed);
                    }
                    result.native = true;
                }
                3 => {
                    if kind != 2 {
                        return Err(Error::Malformed);
                    }
                    for process in fields(value) {
                        let (number, kind, value) = process?;
                        if number == 2 {
                            if kind != 2 {
                                return Err(Error::Malformed);
                            }
                            result.native |= value.eq_ignore_ascii_case(b"elf");
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(result)
    }
    pub fn architecture(self) -> Architecture {
        self.declared.unwrap_or_default()
    }
    pub fn validate(self, target: Option<Architecture>) -> Result<Architecture, Error> {
        let arch = self.architecture();
        if self.native && arch == Architecture::Multi {
            return Err(Error::NativeArchitectureRequired);
        }
        if target.is_some_and(|target| !arch.compatible_with(target)) {
            return Err(Error::IncompatibleArchitecture);
        }
        Ok(arch)
    }
}

/// Operates only on unsigned compiler output, before archive construction.
pub fn stamp(bytes: &[u8], architecture: Architecture) -> Result<alloc::vec::Vec<u8>, Error> {
    let parsed = ManifestArchitecture::decode(bytes)?;
    if parsed.declared.is_some_and(|value| value != architecture) {
        return Err(Error::ConflictingArchitecture);
    }
    let mut output = bytes.to_vec();
    if parsed.declared.is_none() {
        output.extend_from_slice(&[0xa0, 0x01, architecture as u8]);
    }
    ManifestArchitecture::decode(&output)?.validate(None)?;
    Ok(output)
}

pub fn validate_payload(architecture: Architecture, bytes: &[u8]) -> Result<(), Error> {
    if !bytes.starts_with(b"\x7fELF") {
        return Ok(());
    }
    let machine = architecture
        .elf_machine()
        .ok_or(Error::NativeArchitectureRequired)?;
    if bytes.len() < 64 || bytes[4] != 2 || bytes[5] != 1 || bytes[6] != 1 {
        return Err(Error::InvalidElf);
    }
    if u16::from_le_bytes([bytes[18], bytes[19]]) != machine {
        return Err(Error::IncompatibleArchitecture);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
