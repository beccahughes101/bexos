use bexos_trust_store::{
    AppSigningRootAnchor, TrustTier, decode_app_anchor, encode_app_anchor, package_permitted,
    root_id, validate_app_anchor, validate_direct_app_signature,
};

fn app_anchor() -> AppSigningRootAnchor {
    AppSigningRootAnchor {
        anchor_id: "bexos-dev-app-root".into(),
        tier: TrustTier::Tier1PlatformApp,
        algorithm: "Ed25519".into(),
        public_key_bytes: [7u8; 32].to_vec(),
        certificate_der: Vec::new(),
        permitted_package_prefixes: vec!["com.bexos.*".into()],
        valid_from: 1,
        valid_until: 100,
        is_hardware_anchored: true,
        immutable: true,
        enterprise: false,
    }
}

#[test]
fn root_ids_are_deterministic_blake3_hashes() {
    assert_eq!(root_id(b"root").unwrap(), root_id(b"root").unwrap());
    assert_ne!(root_id(b"root").unwrap(), root_id(b"other").unwrap());
}

#[test]
fn app_anchor_round_trips_and_validates() {
    let anchor = app_anchor();
    validate_app_anchor(&anchor).unwrap();
    let decoded = decode_app_anchor(&encode_app_anchor(&anchor)).unwrap();
    assert_eq!(decoded, anchor);
}

#[test]
fn prefix_constraints_match_exact_star_and_prefix() {
    let anchor = app_anchor();
    assert!(package_permitted(&anchor, "com.bexos.settings"));
    assert!(!package_permitted(&anchor, "com.other.settings"));
}

#[test]
fn direct_signature_anchor_checks_key_prefix_and_time() {
    let anchors = [app_anchor()];
    assert_eq!(
        validate_direct_app_signature(&anchors, "com.bexos.settings", 50, &[7u8; 32])
            .unwrap()
            .anchor_id,
        "bexos-dev-app-root"
    );
    assert!(validate_direct_app_signature(&anchors, "com.other.app", 50, &[7u8; 32]).is_err());
    assert!(
        validate_direct_app_signature(&anchors, "com.bexos.settings", 101, &[7u8; 32]).is_err()
    );
    assert!(validate_direct_app_signature(&anchors, "com.bexos.settings", 50, &[8u8; 32]).is_err());
}
