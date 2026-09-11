use bexos_trust_store::{AppSigningRootAnchor, RevocationPayload, TrustTier};
use bexos_trustd::{TrustdService, VerificationStatus};
use ed25519_dalek::{Signer, SigningKey};
use p256::ecdsa::SigningKey as P256SigningKey;
use p256::ecdsa::signature::Signer as P256Signer;
use std::path::PathBuf;

fn service() -> TrustdService {
    TrustdService::new(vec![AppSigningRootAnchor {
        anchor_id: "bexos-dev-app-root".into(),
        tier: TrustTier::Tier1PlatformApp,
        algorithm: "Ed25519".into(),
        public_key_bytes: [7u8; 32].to_vec(),
        certificate_der: Vec::new(),
        permitted_package_prefixes: vec!["com.bexos.*".into()],
        valid_from: 10,
        valid_until: 100,
        is_hardware_anchored: true,
        immutable: true,
        enterprise: false,
    }])
}

#[test]
fn validates_direct_ed25519_anchor_metadata() {
    let result = service().validate_direct_ed25519("com.bexos.settings", 50, &[7u8; 32]);
    assert_eq!(result.status, VerificationStatus::Valid);
    assert_eq!(result.granted_tier, TrustTier::Tier1PlatformApp);
    assert_eq!(result.root_anchor_id, "bexos-dev-app-root");
}

#[test]
fn rejects_prefix_time_and_unknown_key() {
    assert_eq!(
        service()
            .validate_direct_ed25519("com.other.settings", 50, &[7u8; 32])
            .status,
        VerificationStatus::PrefixViolation
    );
    assert_eq!(
        service()
            .validate_direct_ed25519("com.bexos.settings", 101, &[7u8; 32])
            .status,
        VerificationStatus::ExpiredCertificate
    );
    assert_eq!(
        service()
            .validate_direct_ed25519("com.bexos.settings", 50, &[8u8; 32])
            .status,
        VerificationStatus::UntrustedRoot
    );
}

#[test]
fn guest_entry_is_tokio_future() {
    let _future = bexos_trustd::main(0);
}

#[test]
fn validate_app_signer_verifies_digest_signature_and_tier() {
    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let public = signing.verifying_key().to_bytes();
    let mut anchor = service().app_roots()[0].clone();
    anchor.public_key_bytes = public.to_vec();
    anchor.valid_from = 0;
    anchor.valid_until = 0;
    anchor.immutable = false;
    let mut service = TrustdService::new(vec![anchor]);
    let digest = [3u8; 32];
    let signature = signing.sign(&digest).to_bytes();
    let result = service.validate_app_signer(
        "com.bexos.settings",
        0,
        trust_fidl::SignatureAlgorithm::Ed25519,
        &[public.to_vec()],
        &digest,
        &signature,
        TrustTier::Tier1PlatformApp,
    );
    assert_eq!(result.status, VerificationStatus::Valid);

    let revoked = RevocationPayload {
        generation: 1,
        valid_from: 0,
        valid_until: 10,
        revoked_cert_fingerprints: vec![blake3::hash(&public).into()],
        revoked_spki_fingerprints: Vec::new(),
        revoked_serials: Vec::new(),
    };
    service.update_revocation(revoked, 0).unwrap();
    let result = service.validate_app_signer(
        "com.bexos.settings",
        0,
        trust_fidl::SignatureAlgorithm::Ed25519,
        &[public.to_vec()],
        &digest,
        &signature,
        TrustTier::Tier1PlatformApp,
    );
    assert_eq!(result.status, VerificationStatus::RevokedCertificate);
}

#[test]
fn validates_der_x509_path_and_revokes_spki() {
    let root_key = SigningKey::from_bytes(&[11u8; 32]);
    let intermediate_key = SigningKey::from_bytes(&[12u8; 32]);
    let leaf_key = SigningKey::from_bytes(&[13u8; 32]);
    let root_name = name("BexOS Test Root");
    let intermediate_name = name("BexOS Test Intermediate");
    let leaf_name = name("BexOS Test Leaf");
    let root_cert = ed25519_cert(
        1,
        &root_name,
        &root_name,
        &root_key,
        &root_key.verifying_key().to_bytes(),
        true,
    );
    let intermediate_cert = ed25519_cert(
        2,
        &root_name,
        &intermediate_name,
        &root_key,
        &intermediate_key.verifying_key().to_bytes(),
        true,
    );
    let leaf_cert = ed25519_cert(
        3,
        &intermediate_name,
        &leaf_name,
        &intermediate_key,
        &leaf_key.verifying_key().to_bytes(),
        false,
    );
    let service = TrustdService::new(vec![AppSigningRootAnchor {
        anchor_id: "bexos-x509-root".into(),
        tier: TrustTier::Tier2VerifiedEco,
        algorithm: "Ed25519".into(),
        public_key_bytes: Vec::new(),
        certificate_der: root_cert.clone(),
        permitted_package_prefixes: vec!["com.example.*".into()],
        valid_from: 0,
        valid_until: 0,
        is_hardware_anchored: false,
        immutable: true,
        enterprise: false,
    }]);
    let digest = [42u8; 32];
    let signature = leaf_key.sign(&digest).to_bytes();

    let result = service.validate_app_signer(
        "com.example.app",
        1_893_456_000,
        trust_fidl::SignatureAlgorithm::Ed25519,
        &[
            leaf_cert.clone(),
            intermediate_cert.clone(),
            root_cert.clone(),
        ],
        &digest,
        &signature,
        TrustTier::Tier2VerifiedEco,
    );
    assert_eq!(result.status, VerificationStatus::Valid);
    assert_eq!(result.root_anchor_id, "bexos-x509-root");

    let mut revoked = service;
    revoked
        .update_revocation(
            RevocationPayload {
                generation: 1,
                valid_from: 0,
                valid_until: u64::MAX,
                revoked_cert_fingerprints: Vec::new(),
                revoked_spki_fingerprints: vec![
                    blake3::hash(&leaf_spki_der(&leaf_key.verifying_key().to_bytes())).into(),
                ],
                revoked_serials: Vec::new(),
            },
            1_893_456_000,
        )
        .unwrap();
    let result = revoked.validate_app_signer(
        "com.example.app",
        1_893_456_000,
        trust_fidl::SignatureAlgorithm::Ed25519,
        &[leaf_cert, intermediate_cert, root_cert],
        &digest,
        &signature,
        TrustTier::Tier2VerifiedEco,
    );
    assert_eq!(result.status, VerificationStatus::RevokedCertificate);
}

#[test]
fn validates_der_x509_p256_package_signature() {
    let root_key = P256SigningKey::from_slice(&[21u8; 32]).unwrap();
    let leaf_key = P256SigningKey::from_slice(&[22u8; 32]).unwrap();
    let root_name = name("BexOS P256 Root");
    let leaf_name = name("BexOS P256 Leaf");
    let root_cert = p256_cert(
        11,
        &root_name,
        &root_name,
        &root_key,
        root_key.verifying_key().to_encoded_point(false).as_bytes(),
        true,
    );
    let leaf_cert = p256_cert(
        12,
        &root_name,
        &leaf_name,
        &root_key,
        leaf_key.verifying_key().to_encoded_point(false).as_bytes(),
        false,
    );
    let service = TrustdService::new(vec![AppSigningRootAnchor {
        anchor_id: "bexos-p256-root".into(),
        tier: TrustTier::Tier2VerifiedEco,
        algorithm: "ECDSA_P256_SHA256".into(),
        public_key_bytes: Vec::new(),
        certificate_der: root_cert.clone(),
        permitted_package_prefixes: vec!["com.example.*".into()],
        valid_from: 0,
        valid_until: 0,
        is_hardware_anchored: false,
        immutable: true,
        enterprise: false,
    }]);
    let digest = [5u8; 32];
    let signature: p256::ecdsa::Signature = leaf_key.sign(&digest);

    let result = service.validate_app_signer(
        "com.example.p256",
        1_893_456_000,
        trust_fidl::SignatureAlgorithm::EcdsaP256Sha256,
        &[leaf_cert, root_cert],
        &digest,
        &signature.to_bytes(),
        TrustTier::Tier2VerifiedEco,
    );

    assert_eq!(result.status, VerificationStatus::Valid);
    assert_eq!(
        result.signature_algorithm,
        trust_fidl::SignatureAlgorithm::EcdsaP256Sha256
    );
}

#[test]
fn enterprise_roots_are_dynamic_and_platform_prefixes_are_reserved() {
    let mut service = service();
    assert!(
        service
            .install_enterprise_root(vec![1, 2, 3], vec!["bexos.*".into()])
            .is_err()
    );
    let id = service
        .install_enterprise_root(vec![1, 2, 3], vec!["com.example.*".into()])
        .unwrap();
    assert_eq!(service.enterprise_roots().len(), 1);
    service.remove_enterprise_root(&id).unwrap();
    assert!(service.enterprise_roots().is_empty());
}

#[test]
fn loads_app_roots_from_redb_bytes() {
    let root = AppSigningRootAnchor {
        anchor_id: "bexos-dev-app-root".into(),
        tier: TrustTier::Tier1PlatformApp,
        algorithm: "Ed25519".into(),
        public_key_bytes: [9u8; 32].to_vec(),
        certificate_der: Vec::new(),
        permitted_package_prefixes: vec!["bexos.*".into()],
        valid_from: 0,
        valid_until: 0,
        is_hardware_anchored: true,
        immutable: true,
        enterprise: false,
    };
    let path = unique_temp_path("trustd-app-roots.redb");
    let _ = std::fs::remove_file(&path);
    bexos_trust_store::persistent::TrustStoreDb::create_app(&path, &[root])
        .expect("create app root redb");
    let app_roots_redb = std::fs::read(&path).expect("read app root redb");
    let _ = std::fs::remove_file(&path);

    let service = TrustdService::from_root_store_bytes(b"tls-redb".to_vec(), &app_roots_redb)
        .expect("load app roots");
    let result = service.validate_direct_ed25519("bexos.settings", 0, &[9u8; 32]);

    assert_eq!(service.app_roots().len(), 1);
    assert_eq!(result.status, VerificationStatus::Valid);
    assert_eq!(result.granted_tier, TrustTier::Tier1PlatformApp);
    assert_eq!(result.root_anchor_id, "bexos-dev-app-root");
    assert_eq!(service.tls_root_bundle().0, b"tls-redb");
}

fn unique_temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    path
}

fn ed25519_cert(
    serial: u8,
    issuer: &[u8],
    subject: &[u8],
    issuer_key: &SigningKey,
    subject_public_key: &[u8; 32],
    ca: bool,
) -> Vec<u8> {
    let alg = seq(vec![oid(&[0x2b, 0x65, 0x70])]);
    let tbs = seq(vec![
        explicit(0, int(&[2])),
        int(&[serial]),
        alg.clone(),
        issuer.to_vec(),
        seq(vec![utc("260101000000Z"), utc("300101000000Z")]),
        subject.to_vec(),
        leaf_spki_der(subject_public_key),
        explicit(
            3,
            seq(vec![
                extension(
                    &[0x55, 0x1d, 0x13],
                    seq(if ca { vec![boolean(true)] } else { vec![] }),
                ),
                extension(
                    &[0x55, 0x1d, 0x0f],
                    bit_string(if ca { &[0x04] } else { &[0x80] }),
                ),
                extension(
                    &[0x55, 0x1d, 0x25],
                    seq(vec![oid(&[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03])]),
                ),
            ]),
        ),
    ]);
    let signature = issuer_key.sign(&tbs).to_bytes();
    seq(vec![tbs, alg, bit_string(&signature)])
}

fn p256_cert(
    serial: u8,
    issuer: &[u8],
    subject: &[u8],
    issuer_key: &P256SigningKey,
    subject_public_key: &[u8],
    ca: bool,
) -> Vec<u8> {
    let alg = seq(vec![oid(&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02])]);
    let tbs = seq(vec![
        explicit(0, int(&[2])),
        int(&[serial]),
        alg.clone(),
        issuer.to_vec(),
        seq(vec![utc("260101000000Z"), utc("300101000000Z")]),
        subject.to_vec(),
        seq(vec![
            seq(vec![
                oid(&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]),
                oid(&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]),
            ]),
            bit_string(subject_public_key),
        ]),
        explicit(
            3,
            seq(vec![
                extension(
                    &[0x55, 0x1d, 0x13],
                    seq(if ca { vec![boolean(true)] } else { vec![] }),
                ),
                extension(
                    &[0x55, 0x1d, 0x0f],
                    bit_string(if ca { &[0x04] } else { &[0x80] }),
                ),
                extension(
                    &[0x55, 0x1d, 0x25],
                    seq(vec![oid(&[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03])]),
                ),
            ]),
        ),
    ]);
    let signature: p256::ecdsa::Signature = issuer_key.sign(&tbs);
    seq(vec![tbs, alg, bit_string(signature.to_der().as_bytes())])
}

fn leaf_spki_der(public_key: &[u8; 32]) -> Vec<u8> {
    seq(vec![
        seq(vec![oid(&[0x2b, 0x65, 0x70])]),
        bit_string(public_key),
    ])
}

fn name(common_name: &str) -> Vec<u8> {
    seq(vec![set(vec![seq(vec![
        oid(&[0x55, 0x04, 0x03]),
        utf8(common_name),
    ])])])
}

fn extension(oid_bytes: &[u8], value: Vec<u8>) -> Vec<u8> {
    seq(vec![oid(oid_bytes), octet_string(&value)])
}

fn seq(parts: Vec<Vec<u8>>) -> Vec<u8> {
    constructed(0x30, parts)
}

fn set(parts: Vec<Vec<u8>>) -> Vec<u8> {
    constructed(0x31, parts)
}

fn explicit(index: u8, value: Vec<u8>) -> Vec<u8> {
    der(0xa0 + index, &value)
}

fn constructed(tag: u8, parts: Vec<Vec<u8>>) -> Vec<u8> {
    let body = parts.into_iter().flatten().collect::<Vec<_>>();
    der(tag, &body)
}

fn int(bytes: &[u8]) -> Vec<u8> {
    der(0x02, bytes)
}

fn oid(bytes: &[u8]) -> Vec<u8> {
    der(0x06, bytes)
}

fn utc(s: &str) -> Vec<u8> {
    der(0x17, s.as_bytes())
}

fn boolean(value: bool) -> Vec<u8> {
    der(0x01, &[if value { 0xff } else { 0 }])
}

fn bit_string(bytes: &[u8]) -> Vec<u8> {
    let mut value = Vec::with_capacity(bytes.len() + 1);
    value.push(0);
    value.extend_from_slice(bytes);
    der(0x03, &value)
}

fn octet_string(bytes: &[u8]) -> Vec<u8> {
    der(0x04, bytes)
}

fn utf8(s: &str) -> Vec<u8> {
    der(0x0c, s.as_bytes())
}

fn der(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if body.len() < 128 {
        out.push(body.len() as u8);
    } else if body.len() < 256 {
        out.extend_from_slice(&[0x81, body.len() as u8]);
    } else {
        out.push(0x82);
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(body);
    out
}
