use std::sync::Arc;

use bexos_domain_association::{
    DomainPolicyRecord, MemoryDomainAssociationCache, canonical_domain, decode_record,
    encode_record, parse_well_known_json, record_expired, reversed_domain_prefix,
};
use bexos_redb::mem::MemBlockStore;

#[test]
fn canonical_domain_strips_origin_port_path_and_trailing_dot() {
    assert_eq!(
        canonical_domain("https://Waymo.COM:443/apps/rider.bex").unwrap(),
        "waymo.com"
    );
    assert_eq!(canonical_domain("example.com.").unwrap(), "example.com");
    assert_eq!(
        reversed_domain_prefix("apps.github.io").unwrap(),
        "io.github.apps"
    );
}

#[test]
fn protobuf_record_round_trips() {
    let record = sample_record(10);
    let decoded = decode_record(&encode_record(&record)).unwrap();
    assert_eq!(decoded, record);
}

#[test]
fn well_known_json_decodes_into_record() {
    let record = parse_well_known_json(
        br#"{
          "origin": "https://waymo.com",
          "allowed_package_prefixes": ["com.waymo"],
          "trusted_distribution_origins": ["https://cdn.waymo.com"],
          "trusted_peer_domains": ["https://google.com"],
          "signing_certificate_fingerprints": ["SHA256:00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"],
          "ttl_seconds": 60
        }"#,
        42,
    )
    .unwrap();
    assert_eq!(record.origin, "https://waymo.com");
    assert_eq!(record.fetched_timestamp, 42);
    assert_eq!(record.ttl_seconds, 60);
}

#[test]
fn memory_cache_finds_package_and_expires_records() {
    let mut cache = MemoryDomainAssociationCache::new();
    cache.upsert("waymo.com", sample_record(10)).unwrap();
    assert_eq!(
        cache.find_package("com.waymo:rider").unwrap().0,
        "waymo.com"
    );
    assert!(record_expired(cache.get("waymo.com").unwrap(), 10 + 61));
}

#[test]
fn redb_store_reopens_and_replaces_domain_record() {
    use bexos_domain_association::persistent::DomainAssociationDb;

    let store = Arc::new(MemBlockStore::new());
    let db = DomainAssociationDb::open(store.clone()).unwrap();
    db.put("waymo.com", &sample_record(10)).unwrap();
    let mut replacement = sample_record(20);
    replacement.ttl_seconds = 120;
    db.put("WAYMO.com", &replacement).unwrap();

    let reopened = DomainAssociationDb::open(store).unwrap();
    let record = reopened
        .get("https://waymo.com/.well-known/bexos-manifest.json")
        .unwrap();
    assert_eq!(record.fetched_timestamp, 20);
    assert_eq!(record.ttl_seconds, 120);
}

#[test]
fn malformed_protobuf_is_rejected() {
    assert!(decode_record(&[0x0f]).is_err());
}

fn sample_record(fetched_timestamp: u64) -> DomainPolicyRecord {
    DomainPolicyRecord {
        origin: "https://waymo.com".into(),
        allowed_package_prefixes: vec!["com.waymo".into()],
        trusted_distribution_origins: vec!["https://cdn.waymo.com".into()],
        trusted_peer_domains: vec!["https://google.com".into()],
        signing_certificate_fingerprints: vec![
            "SHA256:00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff".into(),
        ],
        fetched_timestamp,
        ttl_seconds: 60,
    }
}
