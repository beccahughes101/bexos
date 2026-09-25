use crate::{
    Error, Result,
    cas::Cas,
    oci::{Oci, parse_digest},
    secure_state::{Grant, RepositoryState, SecureStore},
    transport::Transport,
};
use bexos_pkg_client::{ArtifactKind, ArtifactQuery, BlobDigest, HashType};
use bexos_pkg_config::Config;
use bexos_tuf::TargetSearch;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub struct Resolved {
    pub bytes: std::rc::Rc<crate::payload::Payload>,
    pub digest: BlobDigest,
}
pub struct Prepared {
    pub resolved: Resolved,
    pub state: RepositoryState,
    pub repository: String,
    pub previous_revision: u64,
}

pub async fn resolve<T: Transport + Clone + 'static, S: SecureStore>(
    config: &Config,
    caller: &str,
    query: &ArtifactQuery,
    now: u64,
    oci: &mut Oci<T>,
    secure: &mut S,
    cas: &mut Cas,
) -> Result<Resolved> {
    query.validate()?;
    let repository = config.repository(query).ok_or(Error::AccessDenied)?;
    if repository.id() != oci.repository.id()
        || !config.permits(caller, &repository.id(), query.kind)
    {
        return Err(Error::AccessDenied);
    }
    let state = secure
        .load(&repository.id())?
        .unwrap_or(RepositoryState::new(&repository.trusted_root)?);
    let prepared = download(
        config,
        query,
        now,
        oci,
        state,
        |digest| cas.get(digest),
        |state| persist_transition(secure, &repository.id(), state),
    )
    .await?;
    secure.commit(
        &prepared.repository,
        prepared.previous_revision,
        &prepared.state,
    )?;
    cas.put(
        &prepared.resolved.digest.digest,
        &prepared.resolved.bytes,
        &BTreeSet::new(),
    )?;
    Ok(prepared.resolved)
}

pub async fn download<T: Transport + Clone + 'static>(
    config: &Config,
    query: &ArtifactQuery,
    now: u64,
    oci: &mut Oci<T>,
    mut state: RepositoryState,
    mut cached: impl FnMut(&[u8; 32]) -> Result<Option<Vec<u8>>>,
    mut persist: impl FnMut(&mut RepositoryState) -> Result<()>,
) -> Result<Prepared> {
    query.validate()?;
    let repository = config.repository(query).ok_or(Error::AccessDenied)?;
    if now == 0 || now < state.time_floor {
        return Err(Error::VerifyFailed);
    }
    // Root updates use sequential immutable version tags; every root is checked
    // against both its predecessor and its own threshold before use.
    for _ in 0..32 {
        let next = state
            .tuf
            .root
            .version
            .checked_add(1)
            .ok_or(Error::VerifyFailed)?;
        match oci.role(&format!("tuf-root-{next}"), "root").await {
            Ok(bytes) => {
                state
                    .tuf
                    .rotate_root(&bytes)
                    .map_err(|_| Error::VerifyFailed)?;
                persist(&mut state)?;
            }
            Err(Error::NotFound) => break,
            Err(error) => return Err(error),
        }
    }
    let timestamp = oci.role("tuf-timestamp", "timestamp").await?;
    let snapshot_ref = state
        .tuf
        .accept_timestamp(&timestamp, now)
        .map_err(|_| Error::VerifyFailed)?;
    state.time_floor = now;
    persist(&mut state)?;
    let snapshot = oci.blob(&snapshot_ref.sha256, snapshot_ref.length).await?;
    state
        .tuf
        .accept_snapshot(&snapshot, &snapshot_ref, now)
        .map_err(|_| Error::VerifyFailed)?;
    persist(&mut state)?;
    let mut search = TargetSearch::new(&state.tuf, &snapshot, &query.tag, now)
        .map_err(|_| Error::VerifyFailed)?;
    while let Some(reference) = search.next_reference().map_err(|_| Error::VerifyFailed)? {
        let bytes = oci.blob(&reference.sha256, reference.length).await?;
        search
            .accept(&mut state.tuf, &bytes)
            .map_err(|_| Error::VerifyFailed)?;
        persist(&mut state)?;
    }
    let previous_revision = state.revision;
    let target = search.target().ok_or(Error::NotFound)?;
    let kind = match query.kind {
        ArtifactKind::Application => "application",
        ArtifactKind::Driver => "driver",
        ArtifactKind::Font => "font",
        ArtifactKind::Firmware => "firmware",
        ArtifactKind::NetworkExtension => "network_extension",
        ArtifactKind::Container => "container",
    };
    if target.kind.as_deref() != Some(kind)
        || target.length == 0
        || target.length > config.max_payload_bytes
    {
        return Err(Error::VerifyFailed);
    }
    let target_digest = parse_digest(&format!(
        "sha256:{}",
        target.hashes.get("sha256").ok_or(Error::VerifyFailed)?
    ))?;
    let (bytes, digest, length, manifest_sha256, manifest_length) =
        if query.kind == ArtifactKind::NetworkExtension {
            let manifest = oci.payload(&target_digest, target.length).await?;
            target
                .verify_target(&manifest)
                .map_err(|_| Error::VerifyFailed)?;
            let layer = oci
                .network_extension(&manifest, config.max_payload_bytes)
                .await?;
            (
                layer.bytes,
                layer.digest,
                layer.length,
                Some(target_digest),
                target.length,
            )
        } else {
            let bytes = match cached(&target_digest)? {
                Some(bytes) => {
                    std::rc::Rc::new(crate::payload::Payload::from_bytes(&bytes, &target_digest)?)
                }
                None => oci.payload(&target_digest, target.length).await?,
            };
            target
                .verify_target(&bytes)
                .map_err(|_| Error::VerifyFailed)?;
            (bytes, target_digest, target.length, None, 0)
        };
    let blake3 = *blake3::hash(&bytes).as_bytes();
    if query
        .expected_digest
        .as_ref()
        .is_some_and(|expected| match expected.hash_type {
            HashType::Sha256 => expected.digest != digest,
            HashType::Blake3 => expected.digest != blake3,
        })
    {
        return Err(Error::VerifyFailed);
    }
    state.time_floor = now;
    if !state
        .grants
        .iter()
        .any(|grant| grant.sha256 == digest && grant.kind == query.kind)
    {
        if state.grants.len() >= 4096 {
            return Err(Error::ResourceExhausted);
        }
        state.grants.push(Grant {
            sha256: digest,
            blake3,
            kind: query.kind,
            tag: query.tag.clone(),
            length,
            manifest_sha256,
            manifest_length,
        });
    }
    state.revision = previous_revision
        .checked_add(1)
        .ok_or(Error::ResourceExhausted)?;
    Ok(Prepared {
        resolved: Resolved {
            bytes,
            digest: BlobDigest {
                hash_type: HashType::Sha256,
                digest,
            },
        },
        state,
        repository: repository.id(),
        previous_revision,
    })
}

pub fn resolve_cached<S: SecureStore>(
    config: &Config,
    caller: &str,
    digest: &BlobDigest,
    secure: &mut S,
    cas: &mut Cas,
) -> Result<Resolved> {
    for repository in &config.repositories {
        if !config.consumers.iter().any(|consumer| {
            consumer.package == caller && consumer.repositories.contains(&repository.id())
        }) {
            continue;
        }
        let Some(state) = secure.load(&repository.id())? else {
            continue;
        };
        for grant in &state.grants {
            let matches = match digest.hash_type {
                HashType::Sha256 => digest.digest == grant.sha256,
                HashType::Blake3 => digest.digest == grant.blake3,
            };
            if !matches || !config.permits(caller, &repository.id(), grant.kind) {
                continue;
            }
            if let Some(bytes) = cas.get(&grant.sha256)? {
                if <[u8; 32]>::from(Sha256::digest(&bytes[..])) != grant.sha256
                    || *blake3::hash(&bytes).as_bytes() != grant.blake3
                {
                    return Err(Error::VerifyFailed);
                }
                return Ok(Resolved {
                    bytes: std::rc::Rc::new(crate::payload::Payload::from_bytes(
                        &bytes,
                        &grant.sha256,
                    )?),
                    digest: *digest,
                });
            }
        }
    }
    Err(Error::NotFound)
}

/// Recover only a source whose provenance was committed and still permits this caller.
pub fn known_source<S: SecureStore>(
    config: &Config,
    caller: &str,
    digest: &BlobDigest,
    secure: &mut S,
) -> Result<ArtifactQuery> {
    for repository in &config.repositories {
        if !config
            .consumers
            .iter()
            .any(|c| c.package == caller && c.repositories.contains(&repository.id()))
        {
            continue;
        }
        let Some(state) = secure.load(&repository.id())? else {
            continue;
        };
        for grant in &state.grants {
            let matches = match digest.hash_type {
                HashType::Sha256 => digest.digest == grant.sha256,
                HashType::Blake3 => digest.digest == grant.blake3,
            };
            if matches && config.permits(caller, &repository.id(), grant.kind) {
                return Ok(ArtifactQuery {
                    registry_host: repository.host.clone(),
                    repository: repository.repository.clone(),
                    tag: grant.tag.clone(),
                    kind: grant.kind,
                    expected_digest: Some(*digest),
                });
            }
        }
    }
    Err(Error::NotFound)
}

/// Digest-only recovery uses the immutable descriptor in protected provenance;
/// mutable tags cannot redirect it to a different release after cache eviction.
pub async fn download_known<T: Transport + Clone + 'static>(
    config: &Config,
    query: &ArtifactQuery,
    digest: BlobDigest,
    oci: &mut Oci<T>,
    mut state: RepositoryState,
) -> Result<Prepared> {
    let grant = state
        .grants
        .iter()
        .find(|grant| {
            grant.kind == query.kind
                && match digest.hash_type {
                    HashType::Sha256 => grant.sha256 == digest.digest,
                    HashType::Blake3 => grant.blake3 == digest.digest,
                }
        })
        .ok_or(Error::NotFound)?
        .clone();
    if grant.length == 0 || grant.length > config.max_payload_bytes {
        return Err(Error::ResourceExhausted);
    }
    let bytes = oci.payload(&grant.sha256, grant.length).await?;
    if *blake3::hash(&bytes).as_bytes() != grant.blake3 {
        return Err(Error::VerifyFailed);
    }
    let previous_revision = state.revision;
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or(Error::ResourceExhausted)?;
    Ok(Prepared {
        resolved: Resolved {
            bytes,
            digest: BlobDigest {
                hash_type: HashType::Sha256,
                digest: grant.sha256,
            },
        },
        state,
        repository: oci.repository.id(),
        previous_revision,
    })
}

/// Every verified metadata transition becomes durable independently of payload success.
pub fn persist_transition<S: SecureStore>(
    secure: &mut S,
    repository: &str,
    state: &mut RepositoryState,
) -> Result<()> {
    let expected = state.revision;
    if secure
        .load(repository)?
        .as_ref()
        .map_or(0, |old| old.revision)
        != expected
    {
        return Err(Error::TimedOut);
    }
    state.revision = expected.checked_add(1).ok_or(Error::ResourceExhausted)?;
    secure.commit(repository, expected, state)
}
