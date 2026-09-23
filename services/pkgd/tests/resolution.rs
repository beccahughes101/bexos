use super::fixture::*;
use bexos_pkg_client::{ArtifactKind, BlobDigest, HashType};
use bexos_pkgd::{
    Error,
    cas::Cas,
    oci::Oci,
    resolution::{resolve, resolve_cached},
    secure_state::RepositoryState,
};
use sha2::{Digest, Sha256};
fn cas() -> Cas {
    Cas::open(
        bexos_redb::create_with_store(bexos_redb::mem::MemBlockStore::new()).unwrap(),
        1024 * 1024,
    )
    .unwrap()
}

#[test]
fn signed_oci_pipeline_commits_before_release_and_serves_offline() {
    let fixture = fixture();
    let mut secure = TestStore::default();
    let mut cas = cas();
    let mut oci = Oci {
        repository: fixture.config.repositories[0].clone(),
        transport: fixture.transport,
        token: None,
        transfers: Default::default(),
        credential_generation: 0,
    };
    let result = run(resolve(
        &fixture.config,
        "appd",
        &fixture.query,
        1_800_000_000,
        &mut oci,
        &mut secure,
        &mut cas,
    ))
    .unwrap();
    assert_eq!(&**result.bytes, fixture.payload.as_slice());
    assert_eq!(secure.commits, 4);
    assert!(
        oci.transport
            .calls
            .iter()
            .all(|path| !path.contains("tuf-snapshot") && !path.contains("tuf-targets"))
    );
    assert_eq!(
        &**resolve_cached(
            &fixture.config,
            "appd",
            &result.digest,
            &mut secure,
            &mut cas
        )
        .unwrap()
        .bytes,
        fixture.payload.as_slice()
    );
    let digest = BlobDigest {
        hash_type: HashType::Blake3,
        digest: secure.state.as_ref().unwrap().grants[0].blake3,
    };
    assert!(resolve_cached(&fixture.config, "appd", &digest, &mut secure, &mut cas).is_ok());
    assert!(matches!(
        resolve_cached(&fixture.config, "fontd", &digest, &mut secure, &mut cas),
        Err(Error::NotFound)
    ));
    let state = secure.state.unwrap();
    assert_eq!(
        RepositoryState::decode(&state.encode()).unwrap().floors(),
        state.floors()
    );
}
#[test]
fn failed_secure_commit_never_populates_cas() {
    let fixture = fixture();
    let mut secure = TestStore {
        reject: true,
        ..Default::default()
    };
    let mut cas = cas();
    let mut oci = Oci {
        repository: fixture.config.repositories[0].clone(),
        transport: fixture.transport,
        token: None,
        transfers: Default::default(),
        credential_generation: 0,
    };
    assert!(matches!(
        run(resolve(
            &fixture.config,
            "appd",
            &fixture.query,
            1_800_000_000,
            &mut oci,
            &mut secure,
            &mut cas
        )),
        Err(Error::Unavailable)
    ));
    assert!(
        cas.get(&Sha256::digest(&fixture.payload).into())
            .unwrap()
            .is_none()
    );
    assert!(secure.state.is_none());
}
#[test]
fn expected_digest_and_artifact_kind_cannot_be_bypassed() {
    let mut fixture = fixture();
    let mut secure = TestStore::default();
    let mut cas = cas();
    let mut oci = Oci {
        repository: fixture.config.repositories[0].clone(),
        transport: fixture.transport,
        token: None,
        transfers: Default::default(),
        credential_generation: 0,
    };
    fixture.query.expected_digest = Some(BlobDigest {
        hash_type: HashType::Sha256,
        digest: [0; 32],
    });
    assert!(matches!(
        run(resolve(
            &fixture.config,
            "appd",
            &fixture.query,
            1_800_000_000,
            &mut oci,
            &mut secure,
            &mut cas
        )),
        Err(Error::VerifyFailed)
    ));
    fixture.query.expected_digest = None;
    fixture.query.kind = ArtifactKind::Font;
    fixture.config.consumers[0].kinds.push(3);
    assert!(matches!(
        run(resolve(
            &fixture.config,
            "appd",
            &fixture.query,
            1_800_000_000,
            &mut oci,
            &mut secure,
            &mut cas
        )),
        Err(Error::VerifyFailed)
    ));
    assert!(secure.commits >= 3);
    assert!(secure.state.as_ref().unwrap().grants.is_empty());
}
#[test]
fn missing_time_is_rejected_without_network_activity() {
    let fixture = fixture();
    let mut secure = TestStore::default();
    let mut cas = cas();
    let mut oci = Oci {
        repository: fixture.config.repositories[0].clone(),
        transport: fixture.transport,
        token: None,
        transfers: Default::default(),
        credential_generation: 0,
    };
    assert!(matches!(
        run(resolve(
            &fixture.config,
            "appd",
            &fixture.query,
            0,
            &mut oci,
            &mut secure,
            &mut cas
        )),
        Err(Error::VerifyFailed)
    ));
    assert!(oci.transport.calls.is_empty());
}

#[test]
fn known_digest_survives_tag_removal_and_checks_source_authorization() {
    use bexos_pkgd::resolution::{download_known, known_source};
    let fixture = fixture();
    let mut secure = TestStore::default();
    let mut cache = cas();
    let mut oci = Oci {
        repository: fixture.config.repositories[0].clone(),
        transport: fixture.transport,
        token: None,
        transfers: Default::default(),
        credential_generation: 0,
    };
    let result = run(resolve(
        &fixture.config,
        "appd",
        &fixture.query,
        1_800_000_000,
        &mut oci,
        &mut secure,
        &mut cache,
    ))
    .unwrap();
    let source = known_source(&fixture.config, "appd", &result.digest, &mut secure).unwrap();
    assert!(matches!(
        known_source(&fixture.config, "fontd", &result.digest, &mut secure),
        Err(Error::NotFound)
    ));
    oci.transport
        .responses
        .retain(|path, _| path.contains("/blobs/"));
    let restored = run(download_known(
        &fixture.config,
        &source,
        result.digest,
        &mut oci,
        secure.state.unwrap(),
    ))
    .unwrap();
    assert_eq!(&**restored.resolved.bytes, fixture.payload.as_slice());
    assert_eq!(restored.state.revision, 5);
}

#[test]
fn interrupted_payload_keeps_verified_metadata_but_no_artifact_grant() {
    let fixture = fixture();
    let mut secure = TestStore::default();
    let mut cache = cas();
    let mut transport = fixture.transport;
    transport.responses.remove(&format!(
        "/v2/apps/demo/blobs/sha256:{}",
        hex(&Sha256::digest(&fixture.payload))
    ));
    let mut oci = Oci {
        repository: fixture.config.repositories[0].clone(),
        transport,
        token: None,
        transfers: Default::default(),
        credential_generation: 0,
    };
    assert!(matches!(
        run(resolve(
            &fixture.config,
            "appd",
            &fixture.query,
            1_800_000_000,
            &mut oci,
            &mut secure,
            &mut cache
        )),
        Err(Error::NotFound)
    ));
    assert_eq!(secure.commits, 3);
    let state = secure.state.unwrap();
    assert_eq!(state.tuf.timestamp_version, 1);
    assert_eq!(state.tuf.snapshot_version, 1);
    assert_eq!(state.time_floor, 1_800_000_000);
    assert!(state.grants.is_empty());
    assert!(
        cache
            .get(&Sha256::digest(&fixture.payload).into())
            .unwrap()
            .is_none()
    );
}
