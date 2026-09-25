//! Pollable package acquisition keeps appd's service broker live during downloads.
use alloc::{string::String, vec::Vec};
use app_manager_fidl::{AppManagerStatus, FidlEncode};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_pkg_client::{ArtifactQuery, PendingResolution};
use bexos_pkg_config::HardwareIdentity;
use bexos_userspace::{Channel, Memory};
pub const KEY: u64 = 14;
pub struct PendingInstall {
    pub caller: u64,
    pub resolver: u64,
    pub query: ArtifactQuery,
    pub mapped_package: String,
    pub waiting_node_id: Option<u64>,
    pub expected_hardware: Option<HardwareIdentity>,
}

pub fn begin(
    state: &mut crate::guest::state::AppdState,
    caller: u64,
    query: ArtifactQuery,
) -> Result<(), AppManagerStatus> {
    query
        .validate()
        .map_err(|_| AppManagerStatus::InvalidArgs)?;
    if state.package_installs.len() >= 32 {
        return Err(AppManagerStatus::Storage);
    }
    let provider = state
        .services
        .iter()
        .find(|service| service.package == "bexos.service.pkgd")
        .ok_or(AppManagerStatus::NotFound)?;
    let (client, server) = Channel::pair().map_err(|_| AppManagerStatus::Storage)?;
    if Channel(provider.manager)
        .send(
            b"bexos.pkg.PackageResolver|PackageResolver|Resolve|1,2||bexos.platform.appd|0|bg",
            &[server.0],
        )
        .is_err()
    {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
        return Err(AppManagerStatus::Network);
    }
    let pending = PendingResolution::begin(client, &query).map_err(status)?;
    state.package_installs.push(PendingInstall {
        caller,
        resolver: pending.into_channel().ok_or(AppManagerStatus::Network)?.0,
        query,
        mapped_package: String::new(),
        waiting_node_id: None,
        expected_hardware: None,
    });
    Ok(())
}

pub fn poll(state: &mut crate::guest::state::AppdState) -> (bool, bool, Vec<String>) {
    let pending = core::mem::take(&mut state.package_installs);
    let mut changed = false;
    let mut installed = false;
    let mut drivers = Vec::new();
    for mut install in pending {
        let mut call =
            PendingResolution::adopt(Channel(install.resolver), install.query.expected_digest);
        if install.caller != 0 && crate::guest::handle_peer_closed(install.caller) {
            changed = true;
            continue;
        }
        let result = match call.poll() {
            Ok(None) => {
                install.resolver = call.into_channel().unwrap().0;
                state.package_installs.push(install);
                continue;
            }
            result => result,
        };
        changed = true;
        let (result, package) = match result {
            Ok(Some(vmo)) if validate_payload(vmo.bytes(), &install).is_ok() => {
                let (result, package) = crate::guest::install_archive_bytes(
                    &mut state.registry,
                    &mut state.permissions,
                    &mut state.openers,
                    state.vfsd,
                    None,
                    vmo.bytes(),
                    bexos_app_registry::InstallSource::Oci,
                    "pkg/oci-install.bex",
                );
                let result = match result {
                    app_lifecycle_fidl::AppLifecycleStatus::Ok => AppManagerStatus::Ok,
                    app_lifecycle_fidl::AppLifecycleStatus::Storage => AppManagerStatus::Storage,
                    _ => AppManagerStatus::VerifyFailed,
                };
                installed |= result == AppManagerStatus::Ok;
                if result == AppManagerStatus::Ok
                    && install.query.kind == bexos_pkg_client::ArtifactKind::Driver
                {
                    drivers.push(package.clone());
                }
                (result, package)
            }
            Ok(Some(_)) => (AppManagerStatus::VerifyFailed, String::new()),
            Err(error) => (status(error), String::new()),
            Ok(None) => unreachable!(),
        };
        if install.caller != 0 {
            let response = app_manager_fidl::AppManagerInstallAppFromArtifactResponse {
                status: result,
                allocated_package_id: &package,
            };
            let mut bytes = [0; 256];
            if let Ok(encoded) = response.encode(&mut bytes, &mut []) {
                let _ = Channel(install.caller).send(&bytes[..encoded.bytes], &[]);
            }
        }
    }
    (changed, installed, drivers)
}
fn status(error: bexos_pkg_client::PackageStatus) -> AppManagerStatus {
    match error {
        bexos_pkg_client::PackageStatus::AccessDenied => AppManagerStatus::AccessDenied,
        bexos_pkg_client::PackageStatus::InvalidArgs => AppManagerStatus::InvalidArgs,
        bexos_pkg_client::PackageStatus::VerifyFailed => AppManagerStatus::VerifyFailed,
        bexos_pkg_client::PackageStatus::NotFound => AppManagerStatus::NotFound,
        _ => AppManagerStatus::Network,
    }
}
pub fn encode(installs: &[PendingInstall], retry_after: u64) -> Vec<u8> {
    let mut writer = Encoder::new();
    writer.word(2);
    writer.word(retry_after);
    writer.word(installs.len() as u64);
    for install in installs {
        writer.word(install.caller);
        writer.word(install.resolver);
        writer.text(&install.mapped_package);
        writer.text(&install.query.registry_host);
        writer.text(&install.query.repository);
        writer.text(&install.query.tag);
        writer.word(install.query.kind as u64);
        if let Some(expected) = install.query.expected_digest {
            writer.word(expected.hash_type as u64);
            writer.bytes(&expected.digest);
        } else {
            writer.word(0);
        }
        writer.word(install.waiting_node_id.is_some() as u64);
        writer.word(install.waiting_node_id.unwrap_or(0));
        writer.word(install.expected_hardware.is_some() as u64);
        if let Some(identity) = install.expected_hardware {
            for value in [
                identity.pci_segment as u64,
                identity.pci_bus as u64,
                identity.pci_device as u64,
                identity.pci_function as u64,
                identity.pci_vendor_id as u64,
                identity.pci_device_id as u64,
                identity.pci_class as u64,
                identity.pci_subclass as u64,
                identity.pci_prog_if as u64,
            ] {
                writer.word(value);
            }
        }
    }
    writer.finish()
}
pub fn decode(bytes: &[u8]) -> Result<(Vec<PendingInstall>, u64), Error> {
    let mut reader = Decoder::new(bytes);
    let version = reader.word()?;
    if !(1..=2).contains(&version) {
        return Err(Error::UnsupportedVersion);
    }
    let retry_after = reader.word()?;
    let mut installs = Vec::new();
    for _ in 0..reader.count(32)? {
        let caller = reader.word()?;
        let resolver = reader.word()?;
        let mapped_package = reader.text(128)?.into();
        let registry_host = reader.text(128)?.into();
        let repository = reader.text(128)?.into();
        let tag = reader.text(64)?.into();
        let kind = match reader.word()? {
            1 => pkg_fidl::ArtifactKind::Application,
            2 => pkg_fidl::ArtifactKind::Driver,
            4 => pkg_fidl::ArtifactKind::Firmware,
            _ => return Err(Error::InvalidData),
        };
        let hash = reader.word()?;
        let expected_digest = if hash == 0 {
            None
        } else {
            Some(pkg_fidl::BlobDigest {
                hash_type: match hash {
                    1 => pkg_fidl::HashType::Sha256,
                    2 => pkg_fidl::HashType::Blake3,
                    _ => return Err(Error::InvalidData),
                },
                digest: reader
                    .bytes(32)?
                    .try_into()
                    .map_err(|_| Error::InvalidData)?,
            })
        };
        let query = ArtifactQuery {
            registry_host,
            repository,
            tag,
            kind,
            expected_digest,
        };
        query.validate().map_err(|_| Error::InvalidData)?;
        if resolver == 0 {
            return Err(Error::InvalidData);
        }
        installs.push(PendingInstall {
            caller,
            resolver,
            mapped_package,
            query,
            waiting_node_id: if version >= 2 {
                let present = reader.flag()?;
                let node_id = reader.word()?;
                present.then_some(node_id)
            } else {
                None
            },
            expected_hardware: if version >= 2 && reader.flag()? {
                Some(HardwareIdentity {
                    pci_segment: u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_bus: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_device: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_function: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_vendor_id: u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_device_id: u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_class: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_subclass: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    pci_prog_if: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                })
            } else {
                None
            },
        });
    }
    reader.finish()?;
    Ok((installs, retry_after))
}

fn validate_payload(bytes: &[u8], install: &PendingInstall) -> Result<(), AppManagerStatus> {
    use crate::manifest::{Manifest, PackageKind, UpdateStrategy};
    use bexos_pkg_client::ArtifactKind;
    let archive =
        bexos_app_archive::OpenArchive::parse(bytes).map_err(|_| AppManagerStatus::VerifyFailed)?;
    let entry = archive
        .find(bexos_app_registry::PACKAGE_MANIFEST_PATH)
        .ok_or(AppManagerStatus::VerifyFailed)?;
    let manifest = Manifest::decode(
        &archive
            .read_file(entry)
            .map_err(|_| AppManagerStatus::VerifyFailed)?,
    )
    .map_err(|_| AppManagerStatus::VerifyFailed)?;
    if !install.mapped_package.is_empty() && manifest.package_name != install.mapped_package {
        return Err(AppManagerStatus::VerifyFailed);
    }
    if let Some(identity) = install.expected_hardware {
        if !manifest_matches_hardware(&manifest, identity) {
            return Err(AppManagerStatus::VerifyFailed);
        }
    }
    let driver = manifest.driver_info.is_some() || !manifest.bind_rules.is_empty();
    match install.query.kind {
        ArtifactKind::Driver if !driver => return Err(AppManagerStatus::VerifyFailed),
        ArtifactKind::Application if driver => return Err(AppManagerStatus::VerifyFailed),
        ArtifactKind::Firmware
            if manifest.package_kind != PackageKind::Library || !manifest.processes.is_empty() =>
        {
            return Err(AppManagerStatus::VerifyFailed);
        }
        ArtifactKind::Font => return Err(AppManagerStatus::InvalidArgs),
        ArtifactKind::NetworkExtension => return Err(AppManagerStatus::InvalidArgs),
        _ => {}
    }
    if manifest.processes.iter().any(|p| {
        (driver || p.service) && p.lifecycle.update_strategy != UpdateStrategy::HeartTransplant
    }) {
        return Err(AppManagerStatus::VerifyFailed);
    }
    Ok(())
}

fn manifest_matches_hardware(
    manifest: &crate::manifest::Manifest,
    identity: HardwareIdentity,
) -> bool {
    manifest.bind_rules.iter().any(|rule| {
        rule.conditions.iter().any(|condition| {
            condition.bus == crate::manifest::BindBusType::Pci
                && !condition.properties.is_empty()
                && condition.properties.iter().all(|property| {
                    let actual = match property.key.as_str() {
                        "pci.segment" => identity.pci_segment as u32,
                        "pci.bus" => identity.pci_bus as u32,
                        "pci.device" => identity.pci_device as u32,
                        "pci.function" => identity.pci_function as u32,
                        "pci.vendor_id" => identity.pci_vendor_id as u32,
                        "pci.device_id" => identity.pci_device_id as u32,
                        "pci.class" => identity.pci_class as u32,
                        "pci.subclass" => identity.pci_subclass as u32,
                        "pci.prog_if" => identity.pci_prog_if as u32,
                        _ => return false,
                    };
                    actual == property.value
                })
        })
    })
}

pub fn acquire_hardware_driver(state: &mut crate::guest::state::AppdState) -> bool {
    let now = bexos_userspace::live_migration::now_ms();
    if now < state.package_retry_after
        || !state
            .services
            .iter()
            .any(|service| service.package == "bexos.service.pkgd")
    {
        return false;
    }
    let Ok(config) = bexos_pkg_config::Config::decode(include_bytes!(env!("PKGD_CONFIG"))) else {
        return false;
    };
    let pending_nodes: alloc::vec::Vec<_> = state
        .devices
        .nodes()
        .iter()
        .filter(|node| {
            node.present
                && matches!(
                    node.state,
                    crate::DeviceNodeState::Unbound | crate::DeviceNodeState::BindFailed { .. }
                )
        })
        .filter_map(|node| hardware_identity(node).map(|identity| (node.info.node_id, identity)))
        .collect();
    let mut queued = false;
    for (node_id, identity) in pending_nodes {
        if state
            .package_installs
            .iter()
            .any(|pending| pending.waiting_node_id == Some(node_id))
        {
            continue;
        }
        let Some(mapping) = config.driver_mapping(identity) else {
            continue;
        };
        if state.registry.record(&mapping.name).is_ok() {
            continue;
        }
        if begin(state, 0, mapping.query.clone()).is_ok() {
            let pending = state.package_installs.last_mut().unwrap();
            pending.mapped_package = mapping.name.clone();
            pending.waiting_node_id = Some(node_id);
            pending.expected_hardware = Some(identity);
            queued = true;
        }
    }
    if queued {
        state.package_retry_after = now.saturating_add(60_000);
    }
    queued
}

fn hardware_identity(node: &crate::RegisteredDeviceNode) -> Option<HardwareIdentity> {
    if node.info.bus != crate::BusType::Pci {
        return None;
    }
    let property = |key: &str| {
        node.info
            .properties
            .iter()
            .find(|property| property.key == key)
            .map(|property| property.value)
    };
    Some(HardwareIdentity {
        pci_segment: u16::try_from(property("pci.segment").unwrap_or(0)).ok()?,
        pci_bus: u8::try_from(property("pci.bus")?).ok()?,
        pci_device: u8::try_from(property("pci.device")?).ok()?,
        pci_function: u8::try_from(property("pci.function")?).ok()?,
        pci_vendor_id: u16::try_from(property("pci.vendor_id")?).ok()?,
        pci_device_id: u16::try_from(property("pci.device_id")?).ok()?,
        pci_class: u8::try_from(property("pci.class")?).ok()?,
        pci_subclass: u8::try_from(property("pci.subclass")?).ok()?,
        pci_prog_if: u8::try_from(property("pci.prog_if")?).ok()?,
    })
}

pub fn acquire_configured(state: &mut crate::guest::state::AppdState) -> bool {
    let now = bexos_userspace::live_migration::now_ms();
    if now < state.package_retry_after
        || !state
            .services
            .iter()
            .any(|s| s.package == "bexos.service.pkgd")
    {
        return false;
    }
    state.package_retry_after = now.saturating_add(60_000);
    let Ok(config) = bexos_pkg_config::Config::decode(include_bytes!(env!("PKGD_CONFIG"))) else {
        return false;
    };
    for mapping in config.mappings {
        if !matches!(
            mapping.query.kind,
            bexos_pkg_client::ArtifactKind::Driver | bexos_pkg_client::ArtifactKind::Firmware
        ) || state.registry.record(&mapping.name).is_ok()
            || state
                .package_installs
                .iter()
                .any(|p| p.mapped_package == mapping.name)
        {
            continue;
        }
        if begin(state, 0, mapping.query).is_ok() {
            state.package_installs.last_mut().unwrap().mapped_package = mapping.name;
        }
    }
    true
}
