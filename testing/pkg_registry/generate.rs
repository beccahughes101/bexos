//! Signed local OCI fixture and prototxt configuration, generated only by Bazel.
mod font;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs};
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn signed(value: Value) -> Vec<u8> {
    let key = SigningKey::from_bytes(&[42; 32]);
    let signature = hex(&key.sign(&serde_json::to_vec(&value).unwrap()).to_bytes());
    serde_json::to_vec(&json!({"signed":value,"signatures":[{"keyid":"fixture","sig":signature}]}))
        .unwrap()
}
fn reference(bytes: &[u8]) -> Value {
    json!({"version":1,"length":bytes.len(),"hashes":{"sha256":hex(&Sha256::digest(bytes))}})
}
fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:03o}")).collect()
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        8,
        "configuration responses digest TLS-root application-archive font"
    );
    let key = SigningKey::from_bytes(&[42; 32]);
    let root = signed(
        json!({"_type":"root","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","keys":{"fixture":{"keytype":"ed25519","scheme":"ed25519","keyval":{"public":hex(&key.verifying_key().to_bytes())}}},"roles":{"root":{"keyids":["fixture"],"threshold":1},"timestamp":{"keyids":["fixture"],"threshold":1},"snapshot":{"keyids":["fixture"],"threshold":1},"targets":{"keyids":["fixture"],"threshold":1}}}),
    );
    let payload = fs::read(&args[4]).unwrap();
    let digest = Sha256::digest(&payload);
    let font = font::remote_fixture(fs::read(&args[5]).unwrap());
    let driver = fs::read(&args[6]).unwrap();
    let firmware = fs::read(&args[7]).unwrap();
    let font_digest = hex(&Sha256::digest(&font));
    let targets = signed(
        json!({"_type":"targets","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","targets":{"driver":{"length":driver.len(),"hashes":{"sha256":hex(&Sha256::digest(&driver))},"custom":{"bexos":{"kind":"driver"}}},"firmware":{"length":firmware.len(),"hashes":{"sha256":hex(&Sha256::digest(&firmware))},"custom":{"bexos":{"kind":"firmware"}}},"remote-font":{"length":font.len(),"hashes":{"sha256":font_digest},"custom":{"bexos":{"kind":"font"}}},"latest":{"length":payload.len(),"hashes":{"sha256":hex(&digest)},"custom":{"bexos":{"kind":"application"}}}}}),
    );
    let snapshot = signed(
        json!({"_type":"snapshot","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","meta":{"targets.json":reference(&targets)}}),
    );
    let timestamp = signed(
        json!({"_type":"timestamp","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","meta":{"snapshot.json":reference(&snapshot)}}),
    );
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","artifactType":"application/vnd.bexos.tuf.role.v1","config":{"mediaType":"application/vnd.oci.empty.v1+json","size":2,"digest":format!("sha256:{}",hex(&Sha256::digest(b"{}")))},"layers":[{"mediaType":"application/vnd.bexos.tuf.timestamp.v1+json","size":timestamp.len(),"digest":format!("sha256:{}",hex(&Sha256::digest(&timestamp)))}]})).unwrap();
    let mut responses = BTreeMap::new();
    responses.insert(
        "/v2/apps/demo/manifests/tuf-timestamp".to_string(),
        manifest,
    );
    for bytes in [
        payload, font, driver, firmware, targets, snapshot, timestamp,
    ] {
        responses.insert(
            format!(
                "/v2/apps/demo/blobs/sha256:{}",
                hex(&Sha256::digest(&bytes))
            ),
            bytes,
        );
    }
    let tls = fs::read(&args[3]).unwrap();
    let config = format!(
        "# RFC 64 test fixture only; QEMU user-network host endpoint.\nrepositories {{ registry_host: \"10.0.2.2:18464\" repository: \"apps/demo\" trusted_root: \"{}\" tls_roots_der: \"{}\" token_origins: \"https://10.0.2.2:18464\" }}\nconsumers {{ package_id: \"bexos.platform.pkg_probe\" artifact_kinds: 1 repositories: \"10.0.2.2:18464/apps/demo\" }}\nconsumers {{ package_id: \"bexos.platform.appd\" artifact_kinds: 1 artifact_kinds: 2 artifact_kinds: 4 repositories: \"10.0.2.2:18464/apps/demo\" }}\nmax_cache_bytes: 33554432\nmax_payload_bytes: 8388608\nmax_inflight: 8\nmax_waiters: 16\n",
        escaped(&root),
        escaped(&tls)
    );
    let config = format!(
        "{config}consumers {{ package_id: \"bexos.service.fontd\" artifact_kinds: 3 repositories: \"10.0.2.2:18464/apps/demo\" }}\nmappings {{ name: \"Probe\" registry_host: \"10.0.2.2:18464\" repository: \"apps/demo\" tag: \"remote-font\" kind: 3 }}\n"
    );
    let config = format!(
        "{config}mappings {{ name: \"bexos.test.pkg_driver\" registry_host: \"10.0.2.2:18464\" repository: \"apps/demo\" tag: \"driver\" kind: 2 }}\nmappings {{ name: \"bexos.test.pkg_firmware\" registry_host: \"10.0.2.2:18464\" repository: \"apps/demo\" tag: \"firmware\" kind: 4 }}\n"
    );
    let config = format!("{config}connect_timeout_ms: 30000\nrequest_timeout_ms: 180000\n");
    fs::write(&args[0], config).unwrap();
    fs::write(&args[1], serde_json::to_vec(&responses).unwrap()).unwrap();
    fs::write(&args[2], digest).unwrap();
}
