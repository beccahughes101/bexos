use bexos_pkgd::{Error, cas::Cas, oci::parse_digest};
mod auth;
mod credentials;
mod fixture;
mod recovery;
mod resolution;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[test]
fn configured_network_deadlines_are_defaulted_and_bounded() {
    use bexos_pkg_config::Config;
    let defaults = Config::decode(&[]).unwrap();
    assert_eq!(defaults.connect_timeout_ms, 10_000);
    assert_eq!(defaults.request_timeout_ms, 30_000);
    fn field(tag: u8, mut value: u64) -> Vec<u8> {
        let mut bytes = vec![tag];
        while value >= 128 {
            bytes.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }
    let mut bytes = field(8 << 3, 30_000);
    bytes.extend(field(9 << 3, 180_000));
    let configured = Config::decode(&bytes).unwrap();
    assert_eq!(configured.connect_timeout_ms, 30_000);
    assert_eq!(configured.request_timeout_ms, 180_000);
    for (tag, invalid) in [
        (8 << 3, 0),
        (8 << 3, 120_001),
        (9 << 3, 0),
        (9 << 3, 600_001),
    ] {
        assert!(Config::decode(&field(tag, invalid)).is_err());
    }
}

#[test]
fn cas_deduplicates_evicts_and_reopens() {
    let store = std::sync::Arc::new(bexos_redb::mem::MemBlockStore::new());
    let db = bexos_redb::create_with_store(store.clone()).unwrap();
    let mut cas = Cas::open(db, 8).unwrap();
    let one: [u8; 32] = Sha256::digest(b"first").into();
    let two: [u8; 32] = Sha256::digest(b"next").into();
    cas.put(&one, b"first", &BTreeSet::new()).unwrap();
    assert_eq!(cas.get(&one).unwrap().unwrap(), b"first");
    assert_eq!(
        cas.put(&two, b"next", &BTreeSet::from([one])),
        Err(Error::ResourceExhausted)
    );
    assert!(cas.get(&one).unwrap().is_some());
    cas.put(&two, b"next", &BTreeSet::new()).unwrap();
    assert!(cas.get(&one).unwrap().is_none());
    drop(cas);
    let db = bexos_redb::open_or_create_with_store(store).unwrap();
    let mut cas = Cas::open(db, 8).unwrap();
    assert_eq!(cas.get(&two).unwrap().unwrap(), b"next");
}

#[test]
fn cas_never_accepts_mislabeled_content() {
    let db = bexos_redb::create_with_store(bexos_redb::mem::MemBlockStore::new()).unwrap();
    let mut cas = Cas::open(db, 1024).unwrap();
    assert_eq!(
        cas.put(&[0; 32], b"payload", &BTreeSet::new()),
        Err(Error::VerifyFailed)
    );
    assert!(cas.get(&[0; 32]).unwrap().is_none());
}

#[test]
fn oci_requires_exact_lowercase_sha256_descriptors() {
    assert!(parse_digest(&format!("sha256:{}", "ab".repeat(32))).is_ok());
    for value in [
        "sha256:abc".to_string(),
        format!("sha256:{}", "AB".repeat(32)),
        format!("blake3:{}", "ab".repeat(32)),
    ] {
        assert!(parse_digest(&value).is_err());
    }
}

#[test]
fn network_extension_requires_exact_oci_media_types_and_returns_wasm_layer() {
    use bexos_pkg_config::Repository;
    use bexos_pkgd::oci::{
        NETWORK_EXTENSION_ABI, NETWORK_EXTENSION_CONFIG, NETWORK_EXTENSION_MANIFEST,
        NETWORK_EXTENSION_WASM, Oci, hex,
    };
    use serde_json::json;

    let config = serde_json::to_vec(&json!({"abi": NETWORK_EXTENSION_ABI})).unwrap();
    let wasm = b"\0asm\x01\0\0\0".to_vec();
    let config_digest: [u8; 32] = Sha256::digest(&config).into();
    let wasm_digest: [u8; 32] = Sha256::digest(&wasm).into();
    let manifest = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "artifactType": NETWORK_EXTENSION_MANIFEST,
        "annotations": {"org.bexos.network.abi": NETWORK_EXTENSION_ABI},
        "config": {
            "mediaType": NETWORK_EXTENSION_CONFIG,
            "digest": format!("sha256:{}", hex(&config_digest)),
            "size": config.len(),
        },
        "layers": [{
            "mediaType": NETWORK_EXTENSION_WASM,
            "digest": format!("sha256:{}", hex(&wasm_digest)),
            "size": wasm.len(),
        }],
    }))
    .unwrap();
    let repository = Repository {
        host: "registry.test".into(),
        repository: "extensions/firewall".into(),
        trusted_root: Vec::new(),
        token_origins: Vec::new(),
        redirect_origins: Vec::new(),
        tls_roots_der: Vec::new(),
    };
    let mut transport = fixture::MemoryTransport {
        responses: Default::default(),
        calls: Vec::new(),
    };
    transport.responses.insert(
        format!(
            "/v2/extensions/firewall/blobs/sha256:{}",
            hex(&config_digest)
        ),
        config,
    );
    transport.responses.insert(
        format!("/v2/extensions/firewall/blobs/sha256:{}", hex(&wasm_digest)),
        wasm.clone(),
    );
    let mut oci = Oci {
        transport,
        repository,
        token: None,
        transfers: Default::default(),
        credential_generation: 0,
    };
    let layer = fixture::run(oci.network_extension(&manifest, 1024)).unwrap();
    assert_eq!(&**layer.bytes, wasm.as_slice());
    assert_eq!(layer.digest, wasm_digest);
    assert_eq!(layer.length, wasm.len() as u64);

    let mut invalid: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    invalid["layers"][0]["mediaType"] = json!("application/wasm");
    assert!(matches!(
        fixture::run(oci.network_extension(&serde_json::to_vec(&invalid).unwrap(), 1024)),
        Err(Error::VerifyFailed)
    ));
}

#[test]
fn payload_rejects_partial_overlong_and_corrupt_streams() {
    use bexos_pkgd::payload::Payload;
    let hash: [u8; 32] = Sha256::digest(b"abcd").into();
    let mut partial = Payload::new(4).unwrap();
    partial.write(b"abc").unwrap();
    assert!(partial.seal(&hash).is_err());
    let mut long = Payload::new(4).unwrap();
    assert!(long.write(b"abcde").is_err());
    let mut bad = Payload::new(4).unwrap();
    bad.write(b"abce").unwrap();
    assert!(bad.seal(&hash).is_err());
    let mut valid = Payload::new(4).unwrap();
    valid.write(b"ab").unwrap();
    valid.write(b"cd").unwrap();
    let mut valid = valid.seal(&hash).unwrap();
    assert_eq!(&*valid, b"abcd");
    assert!(valid.write(b"x").is_err());
}

#[test]
fn corrupt_cas_entry_is_removed_on_read() {
    let store = std::sync::Arc::new(bexos_redb::mem::MemBlockStore::new());
    let db = bexos_redb::create_with_store(store.clone()).unwrap();
    let mut cas = Cas::open(db, 1024).unwrap();
    let digest: [u8; 32] = Sha256::digest(b"valid").into();
    cas.put(&digest, b"valid", &BTreeSet::new()).unwrap();
    drop(cas);
    let db = bexos_redb::open_or_create_with_store(store.clone()).unwrap();
    let write = db.begin_write().unwrap();
    {
        let mut table = write
            .open_table(redb::TableDefinition::<&[u8], &[u8]>::new("pkg_blobs_v1"))
            .unwrap();
        table
            .insert(digest.as_slice(), b"broken".as_slice())
            .unwrap();
    }
    write.commit().unwrap();
    drop(db);
    let db = bexos_redb::open_or_create_with_store(store.clone()).unwrap();
    let mut cas = Cas::open(db, 1024).unwrap();
    assert!(cas.get(&digest).unwrap().is_none());
    drop(cas);
    let db = bexos_redb::open_or_create_with_store(store).unwrap();
    let mut cas = Cas::open(db, 1024).unwrap();
    assert!(cas.get(&digest).unwrap().is_none());
}

#[test]
fn digest_transfers_share_work_and_cancel_when_last_waiter_drops() {
    use bexos_pkgd::{payload::Payload, transfers::Transfers};
    use std::{
        cell::Cell,
        future::Future,
        rc::Rc,
        task::{Context, Poll, Waker},
    };
    struct Work {
        polls: Rc<Cell<u32>>,
        drops: Rc<Cell<u32>>,
    }
    impl Future for Work {
        type Output = bexos_pkgd::Result<Payload>;
        fn poll(self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            self.polls.set(self.polls.get() + 1);
            Poll::Pending
        }
    }
    impl Drop for Work {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }
    let pool = Transfers::new(1);
    let polls = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let mut cx = Context::from_waker(Waker::noop());
    let mut first = Box::pin(pool.get("digest".into(), || {
        Box::pin(Work {
            polls: polls.clone(),
            drops: drops.clone(),
        })
    }));
    assert!(first.as_mut().poll(&mut cx).is_pending());
    let mut second = Box::pin(pool.get("digest".into(), || panic!("duplicate transfer")));
    assert!(second.as_mut().poll(&mut cx).is_pending());
    assert_eq!(polls.get(), 2);
    let mut over_quota =
        Box::pin(pool.get("other".into(), || panic!("quota must precede creation")));
    assert!(matches!(
        over_quota.as_mut().poll(&mut cx),
        Poll::Ready(Err(Error::ResourceExhausted))
    ));
    drop(first);
    assert_eq!(drops.get(), 0);
    drop(second);
    assert_eq!(drops.get(), 1);
}

#[test]
fn physical_store_quota_rejects_growth_before_mutation() {
    use bexos_redb::{BlockStore, BlockStoreError, bounded::BoundedStore};
    let store = std::sync::Arc::new(bexos_redb::mem::MemBlockStore::new());
    let bounded = BoundedStore::new(store.clone(), 4096).unwrap();
    bounded.set_len(4096).unwrap();
    assert_eq!(bounded.set_len(4097), Err(BlockStoreError::OutOfBounds));
    assert_eq!(
        bounded.write_at(4095, b"ab"),
        Err(BlockStoreError::OutOfBounds)
    );
    assert_eq!(
        bounded.write_at(u64::MAX, b"a"),
        Err(BlockStoreError::OutOfBounds)
    );
    assert_eq!(store.len().unwrap(), 4096);
    assert!(BoundedStore::new(store, 4095).is_err());
}
