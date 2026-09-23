//! Pollable package acquisition keeps appd's service broker live during downloads.
use alloc::{string::String, vec::Vec};
use app_manager_fidl::{AppManagerStatus, FidlEncode};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_pkg_client::{ArtifactQuery, PendingResolution};
use bexos_userspace::{Channel, Memory};
pub const KEY: u64 = 14;
pub struct PendingInstall {
    pub caller: u64,
    pub resolver: u64,
    pub query: ArtifactQuery,
    pub mapped_package: String,
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
    writer.word(1);
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
    }
    writer.finish()
}
pub fn decode(bytes: &[u8]) -> Result<(Vec<PendingInstall>, u64), Error> {
    let mut reader = Decoder::new(bytes);
    if reader.word()? != 1 {
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
        _ => {}
    }
    if manifest.processes.iter().any(|p| {
        (driver || p.service) && p.lifecycle.update_strategy != UpdateStrategy::HeartTransplant
    }) {
        return Err(AppManagerStatus::VerifyFailed);
    }
    Ok(())
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
