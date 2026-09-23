use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use app_manager_fidl as app_manager;
use bexos_domain_association::{
    DomainAssociationError, MemoryDomainAssociationCache, canonical_domain, decode_record,
    encode_record, parse_well_known_json, validate_record_for_domain,
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;

#[derive(Clone)]
pub struct AppManagerBinding {
    pub channel: u64,
    pub package: String,
    pub uid: u64,
    pub system: bool,
}

pub trait WellKnownFetcher {
    fn fetch_well_known(&mut self, domain: &str) -> Result<Vec<u8>, WebInstallError>;
}

pub trait AppBundleFetcher: WellKnownFetcher {
    fn fetch_app_bundle(&mut self, url: &str) -> Result<Vec<u8>, WebInstallError>;
}

pub struct UnavailableFetcher;

impl WellKnownFetcher for UnavailableFetcher {
    fn fetch_well_known(&mut self, _domain: &str) -> Result<Vec<u8>, WebInstallError> {
        Err(WebInstallError::Network)
    }
}

impl AppBundleFetcher for UnavailableFetcher {
    fn fetch_app_bundle(&mut self, _url: &str) -> Result<Vec<u8>, WebInstallError> {
        Err(WebInstallError::Network)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebInstallError {
    InvalidArgs,
    Storage,
    VerifyFailed,
    Network,
}

pub fn encode_app_manager_binding(binding: &AppManagerBinding) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(binding.channel);
    w.text(&binding.package);
    w.word(binding.uid);
    w.word(binding.system as u64);
    w.finish()
}

pub fn decode_app_manager_binding(bytes: &[u8]) -> Result<AppManagerBinding, Error> {
    let mut r = Decoder::new(bytes);
    let binding = AppManagerBinding {
        channel: r.word()?,
        package: r.text(128)?.to_string(),
        uid: r.word()?,
        system: r.flag()?,
    };
    r.finish()?;
    if binding.channel == 0 || binding.package.is_empty() {
        return Err(Error::InvalidData);
    }
    Ok(binding)
}

pub fn encode_domain_associations(cache: &MemoryDomainAssociationCache) -> Vec<u8> {
    let mut w = Encoder::new();
    let records = cache.records();
    w.word(records.len() as u64);
    for (domain, record) in records {
        w.text(domain);
        w.bytes(&encode_record(record));
    }
    w.finish()
}

pub fn decode_domain_associations(bytes: &[u8]) -> Result<MemoryDomainAssociationCache, Error> {
    let mut r = Decoder::new(bytes);
    let mut cache = MemoryDomainAssociationCache::new();
    for _ in 0..r.count(1024)? {
        let domain = canonical_domain(r.text(255)?).map_err(|_| Error::InvalidData)?;
        let record = decode_record(r.bytes(65536)?).map_err(|_| Error::InvalidData)?;
        validate_record_for_domain(&domain, &record).map_err(|_| Error::InvalidData)?;
        cache
            .upsert(&domain, record)
            .map_err(|_| Error::InvalidData)?;
    }
    r.finish()?;
    Ok(cache)
}

pub fn reload_well_known_for_domain(
    state: &mut crate::guest::state::AppdState,
    fetcher: &mut dyn WellKnownFetcher,
    domain: &str,
) -> (
    app_manager::AppManagerStatus,
    app_manager::AssociationStatus,
    u32,
) {
    let domain = match canonical_domain(domain) {
        Ok(domain) => domain,
        Err(_) => {
            return (
                app_manager::AppManagerStatus::InvalidArgs,
                app_manager::AssociationStatus::DomainUnreachable,
                0,
            );
        }
    };
    let bytes = match fetcher.fetch_well_known(&domain) {
        Ok(bytes) => bytes,
        Err(WebInstallError::Network) => {
            return (
                app_manager::AppManagerStatus::Network,
                app_manager::AssociationStatus::DomainUnreachable,
                0,
            );
        }
        Err(WebInstallError::Storage) => {
            return (
                app_manager::AppManagerStatus::Storage,
                app_manager::AssociationStatus::DomainUnreachable,
                0,
            );
        }
        Err(_) => {
            return (
                app_manager::AppManagerStatus::VerifyFailed,
                app_manager::AssociationStatus::OriginMismatch,
                0,
            );
        }
    };
    let now = bexos_userspace::syscall::ticks() / bexos_userspace::syscall::frequency().max(1);
    let record = match parse_well_known_json(&bytes, now)
        .and_then(|record| validate_record_for_domain(&domain, &record).map(|_| record))
    {
        Ok(record) => record,
        Err(DomainAssociationError::InvalidOrigin) => {
            return (
                app_manager::AppManagerStatus::VerifyFailed,
                app_manager::AssociationStatus::OriginMismatch,
                0,
            );
        }
        Err(_) => {
            return (
                app_manager::AppManagerStatus::VerifyFailed,
                app_manager::AssociationStatus::SigningKeyMismatch,
                0,
            );
        }
    };
    if state
        .domain_associations
        .upsert(&domain, record.clone())
        .is_err()
    {
        return (
            app_manager::AppManagerStatus::VerifyFailed,
            app_manager::AssociationStatus::OriginMismatch,
            0,
        );
    }
    #[cfg(feature = "persistent")]
    if let Some(stores) = &state.stores {
        if stores.domain_associations.put(&domain, &record).is_err() {
            return (
                app_manager::AppManagerStatus::Storage,
                app_manager::AssociationStatus::Verified,
                0,
            );
        }
    }
    let updated = refresh_verified_openers(state, &domain);
    (
        app_manager::AppManagerStatus::Ok,
        app_manager::AssociationStatus::Verified,
        updated,
    )
}

pub fn install_app_from_url(
    state: &mut crate::guest::state::AppdState,
    fetcher: &mut dyn AppBundleFetcher,
    url: &str,
) -> (
    app_manager::AppManagerStatus,
    app_manager::AssociationStatus,
    String,
) {
    let parsed_url = match bexos_distribution::parse_https_url(url) {
        Ok(parsed) if url.len() <= 2048 => parsed,
        _ => {
            return (
                app_manager::AppManagerStatus::InvalidArgs,
                app_manager::AssociationStatus::DomainUnreachable,
                String::new(),
            );
        }
    };
    let archive = match fetcher.fetch_app_bundle(url) {
        Ok(bytes) => bytes,
        Err(WebInstallError::Network) => {
            return (
                app_manager::AppManagerStatus::Network,
                app_manager::AssociationStatus::DomainUnreachable,
                String::new(),
            );
        }
        Err(WebInstallError::Storage) => {
            return (
                app_manager::AppManagerStatus::Storage,
                app_manager::AssociationStatus::DomainUnreachable,
                String::new(),
            );
        }
        Err(_) => {
            return (
                app_manager::AppManagerStatus::VerifyFailed,
                app_manager::AssociationStatus::OriginMismatch,
                String::new(),
            );
        }
    };
    let (status, package_id) = crate::guest::install_archive_bytes(
        &mut state.registry,
        &mut state.permissions,
        &mut state.openers,
        state.vfsd,
        None,
        &archive,
        bexos_app_registry::InstallSource::Debugd,
        "pkg/url-install.bex",
    );
    let status = match status {
        app_lifecycle_fidl::AppLifecycleStatus::Ok => app_manager::AppManagerStatus::Ok,
        app_lifecycle_fidl::AppLifecycleStatus::InvalidArgs => {
            app_manager::AppManagerStatus::InvalidArgs
        }
        app_lifecycle_fidl::AppLifecycleStatus::Storage => app_manager::AppManagerStatus::Storage,
        _ => app_manager::AppManagerStatus::VerifyFailed,
    };
    if status != app_manager::AppManagerStatus::Ok {
        return (
            status,
            app_manager::AssociationStatus::DomainUnreachable,
            package_id,
        );
    }
    let association = match fetcher.fetch_well_known(&parsed_url.host) {
        Ok(bytes) => {
            let now =
                bexos_userspace::syscall::ticks() / bexos_userspace::syscall::frequency().max(1);
            match parse_well_known_json(&bytes, now).and_then(|record| {
                validate_record_for_domain(&parsed_url.host, &record).map(|_| record)
            }) {
                Ok(record) => {
                    let _ = state.domain_associations.upsert(&parsed_url.host, record);
                    app_manager::AssociationStatus::Verified
                }
                Err(_) => app_manager::AssociationStatus::OriginMismatch,
            }
        }
        Err(_) => app_manager::AssociationStatus::DomainUnreachable,
    };
    (status, association, package_id)
}

pub fn refresh_verified_openers(state: &mut crate::guest::state::AppdState, domain: &str) -> u32 {
    let mut changed = 0;
    let records = state.registry.list_packages().to_vec();
    for record in records {
        let Ok(manifest) = crate::Manifest::decode(&record.manifest_bytes) else {
            continue;
        };
        let has_domain = manifest.processes.iter().any(|process| {
            process
                .handles
                .iter()
                .any(|filter| filter.domains.iter().any(|candidate| candidate == domain))
        });
        if !has_domain {
            continue;
        }
        let verified = state.domain_associations.get(domain).is_ok_and(|policy| {
            bexos_domain_association::record_allows_package(policy, &manifest.package_name)
        });
        state.openers.remove_package(&manifest.package_name);
        crate::register_manifest_openers(
            &mut state.openers,
            crate::OpenerScope::System,
            &manifest,
            verified,
        );
        changed += 1;
    }
    changed
}

pub fn handle_app_manager_message(
    state: &mut crate::guest::state::AppdState,
    channel: u64,
    bytes: &[u8],
    handles: &[u64],
) -> bool {
    use app_manager::FidlDecode;
    if bytes.len() < 8 {
        close_handles(handles);
        return false;
    }
    let ordinal = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let request = &bytes[8..];
    let handle_refs = handles
        .iter()
        .map(|raw| app_manager::HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    let mut durable_changed = false;
    match ordinal {
        4 => {
            let result =
                app_manager::AppManagerInstallAppFromArtifactRequest::decode(request, &handle_refs)
                    .map_err(|_| app_manager::AppManagerStatus::InvalidArgs)
                    .and_then(|request| {
                        if !handles.is_empty()
                            || request.query.kind != app_manager::InstallArtifactKind::Application
                        {
                            return Err(app_manager::AppManagerStatus::InvalidArgs);
                        }
                        let expected_digest = if request.query.expected_digest.len() == 0 {
                            None
                        } else {
                            let expected = request
                                .query
                                .expected_digest
                                .get(0)
                                .map_err(|_| app_manager::AppManagerStatus::InvalidArgs)?;
                            Some(pkg_fidl::BlobDigest {
                                hash_type: match expected.hash_type {
                                    app_manager::ArtifactDigestHash::Sha256 => {
                                        pkg_fidl::HashType::Sha256
                                    }
                                    app_manager::ArtifactDigestHash::Blake3 => {
                                        pkg_fidl::HashType::Blake3
                                    }
                                },
                                digest: expected.digest,
                            })
                        };
                        crate::package_install::begin(
                            state,
                            channel,
                            bexos_pkg_client::ArtifactQuery {
                                registry_host: request.query.registry_host.into(),
                                repository: request.query.repository.into(),
                                tag: request.query.tag.into(),
                                expected_digest,
                                kind: pkg_fidl::ArtifactKind::Application,
                            },
                        )
                    });
            close_handles(handles);
            if let Err(status) = result {
                app_manager_reply(
                    channel,
                    &app_manager::AppManagerInstallAppFromArtifactResponse {
                        status,
                        allocated_package_id: "",
                    },
                );
            }
        }
        1 => {
            let package_id;
            let response = match app_manager::AppManagerInstallAppFromUrlRequest::decode(
                request,
                &handle_refs,
            ) {
                Ok(request) => {
                    let mut fetcher = UnavailableFetcher;
                    let (status, association, installed_package_id) =
                        install_app_from_url(state, &mut fetcher, request.url);
                    package_id = installed_package_id;
                    app_manager::AppManagerInstallAppFromUrlResponse {
                        status,
                        allocated_package_id: &package_id,
                        association,
                    }
                }
                Err(_) => app_manager::AppManagerInstallAppFromUrlResponse {
                    status: app_manager::AppManagerStatus::InvalidArgs,
                    allocated_package_id: "",
                    association: app_manager::AssociationStatus::DomainUnreachable,
                },
            };
            durable_changed = response.status == app_manager::AppManagerStatus::Ok;
            app_manager_reply(channel, &response);
        }
        2 => {
            let response = match app_manager::AppManagerReloadWellKnownForDomainRequest::decode(
                request,
                &handle_refs,
            ) {
                Ok(request) => {
                    let mut fetcher = UnavailableFetcher;
                    let (status, association, updated_handlers_count) =
                        reload_well_known_for_domain(state, &mut fetcher, request.domain);
                    app_manager::AppManagerReloadWellKnownForDomainResponse {
                        status,
                        association,
                        updated_handlers_count,
                    }
                }
                Err(_) => app_manager::AppManagerReloadWellKnownForDomainResponse {
                    status: app_manager::AppManagerStatus::InvalidArgs,
                    association: app_manager::AssociationStatus::DomainUnreachable,
                    updated_handlers_count: 0,
                },
            };
            durable_changed = response.status == app_manager::AppManagerStatus::Ok;
            app_manager_reply(channel, &response);
        }
        3 => {
            let response = match app_manager::AppManagerGetDomainAssociationRequest::decode(
                request,
                &handle_refs,
            ) {
                Ok(request) => get_domain_association(state, request.package_id),
                Err(_) => app_manager::AppManagerGetDomainAssociationResponse {
                    status: app_manager::AppManagerStatus::InvalidArgs,
                    domain: "",
                    association: app_manager::AssociationStatus::DomainUnreachable,
                    last_verified_timestamp: 0,
                },
            };
            app_manager_reply(channel, &response);
        }
        _ => close_handles(handles),
    }
    durable_changed
}

fn get_domain_association<'a>(
    state: &'a crate::guest::state::AppdState,
    package_id: &str,
) -> app_manager::AppManagerGetDomainAssociationResponse<'a> {
    for (domain, record) in state.domain_associations.records() {
        if bexos_domain_association::record_allows_package(record, package_id) {
            return app_manager::AppManagerGetDomainAssociationResponse {
                status: app_manager::AppManagerStatus::Ok,
                domain: domain.as_str(),
                association: app_manager::AssociationStatus::Verified,
                last_verified_timestamp: record.fetched_timestamp,
            };
        }
    }
    app_manager::AppManagerGetDomainAssociationResponse {
        status: app_manager::AppManagerStatus::NotFound,
        domain: "",
        association: app_manager::AssociationStatus::DomainUnreachable,
        last_verified_timestamp: 0,
    }
}

fn app_manager_reply<T: app_manager::FidlEncode>(channel: u64, value: &T) {
    let mut bytes = alloc::vec![0; 65500];
    let mut handles = [app_manager::HandleRef { raw: 0 }; 16];
    if let Ok(encoded) = value.encode(&mut bytes, &mut handles) {
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = Channel(channel).send(&bytes[..encoded.bytes], &raw_handles);
    }
}

fn close_handles(handles: &[u64]) {
    for handle in handles {
        let _ = bexos_userspace::Memory::close(*handle);
    }
}
