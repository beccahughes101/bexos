use bexos_tuf::{ClientState, MetadataSet, TufError, UpdateSelector};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::BTreeMap;

const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const KEY_ID: &str = "dev-key";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn signed(role: Value) -> Vec<u8> {
    let signing = SigningKey::from_bytes(&SEED);
    let signed_bytes = serde_json::to_vec(&role).unwrap();
    let sig = signing.sign(&signed_bytes).to_bytes();
    serde_json::to_vec(&json!({
        "signatures": [{"keyid": KEY_ID, "sig": hex(&sig)}],
        "signed": role,
    }))
    .unwrap()
}

fn repo() -> (Vec<u8>, MetadataSet, Vec<u8>) {
    repo_for_kind("APP_PACKAGE", "com.bexos.demo")
}

fn repo_for_kind(kind: &str, target_id: &str) -> (Vec<u8>, MetadataSet, Vec<u8>) {
    let public = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
    let target = b"app bundle bytes".to_vec();
    let targets = signed(json!({
        "_type": "targets",
        "spec_version": "1.0.26",
        "version": 1,
        "expires": "2099-01-01T00:00:00Z",
        "targets": {
            "apps/demo.bex": {
                "length": target.len(),
                "hashes": {
                    "sha256": hex(&sha2::Sha256::digest(&target)),
                    "blake3": blake3::hash(&target).to_hex().to_string()
                },
                "custom": {"bexos": {
                    "kind": kind,
                    "target_id": target_id,
                    "generation": 4,
                    "url": "https://repo.example/apps/demo.bex"
                }}
            }
        },
        "delegations": {
            "keys": {},
            "roles": []
        }
    }));
    let snapshot = signed(json!({
        "_type": "snapshot",
        "spec_version": "1.0.26",
        "version": 1,
        "expires": "2099-01-01T00:00:00Z",
        "meta": {
            "targets.json": {"version": 1, "length": targets.len(), "hashes": {"sha256": hex(&sha2::Sha256::digest(&targets))}}
        }
    }));
    let timestamp = signed(json!({
        "_type": "timestamp",
        "spec_version": "1.0.26",
        "version": 1,
        "expires": "2099-01-01T00:00:00Z",
        "meta": {
            "snapshot.json": {"version": 1, "length": snapshot.len(), "hashes": {"sha256": hex(&sha2::Sha256::digest(&snapshot))}}
        }
    }));
    let root = signed(json!({
        "_type": "root",
        "spec_version": "1.0.26",
        "version": 1,
        "expires": "2099-01-01T00:00:00Z",
        "consistent_snapshot": true,
        "keys": {
            KEY_ID: {
                "keytype": "ed25519",
                "scheme": "ed25519",
                "keyval": {"public": hex(&public)}
            }
        },
        "roles": {
            "root": {"keyids": [KEY_ID], "threshold": 1},
            "timestamp": {"keyids": [KEY_ID], "threshold": 1},
            "snapshot": {"keyids": [KEY_ID], "threshold": 1},
            "targets": {"keyids": [KEY_ID], "threshold": 1}
        }
    }));
    let mut targets_map = BTreeMap::new();
    targets_map.insert("targets".into(), targets);
    (
        root.clone(),
        MetadataSet {
            root,
            timestamp,
            snapshot,
            targets: targets_map,
        },
        target,
    )
}

#[test]
fn verifies_valid_repository_and_target() {
    let (root, metadata, target) = repo();
    let mut state = ClientState::new(root).unwrap();
    let verified = state.refresh(&metadata, 0).unwrap();
    let candidates = verified.candidates(UpdateSelector::Package("com.bexos.demo"));
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].generation, 4);
    candidates[0].verify_target(&target).unwrap();
}

#[test]
fn rejects_bad_signature_hash_expiry_rollback_and_target_length() {
    let (root, mut metadata, target) = repo();
    metadata.targets.get_mut("targets").unwrap()[20] ^= 1;
    let mut state = ClientState::new(root.clone()).unwrap();
    assert_eq!(state.refresh(&metadata, 0), Err(TufError::BadHash));

    let (_, mut metadata, _) = repo();
    let mut timestamp: Value = serde_json::from_slice(&metadata.timestamp).unwrap();
    timestamp["signatures"][0]["sig"] = Value::String("00".repeat(64));
    metadata.timestamp = serde_json::to_vec(&timestamp).unwrap();
    let mut state = ClientState::new(root.clone()).unwrap();
    assert_eq!(state.refresh(&metadata, 0), Err(TufError::Threshold));

    let (_, mut metadata, _) = repo();
    metadata.snapshot[40] ^= 1;
    let mut state = ClientState::new(root.clone()).unwrap();
    assert_eq!(state.refresh(&metadata, 0), Err(TufError::BadHash));

    let (_, metadata, _) = repo();
    let mut state = ClientState::new(root.clone()).unwrap();
    assert_eq!(
        state.refresh(&metadata, 4_102_444_800),
        Err(TufError::Expired)
    );

    let (_, metadata, _) = repo();
    let mut state = ClientState::new(root).unwrap();
    let verified = state.refresh(&metadata, 0).unwrap();
    let candidate = verified.candidates(UpdateSelector::All).remove(0);
    assert_eq!(
        candidate.verify_target(&target[..target.len() - 1]),
        Err(TufError::BadLength)
    );
}

#[test]
fn signed_hypervisor_candidates_are_separate_from_kernel_and_tee() {
    let (root, metadata, target) = repo_for_kind("HYPERVISOR", "qemu-x86_64-monitor");
    let mut state = ClientState::new(root).unwrap();
    let verified = state.refresh(&metadata, 0).unwrap();
    assert!(verified.candidates(UpdateSelector::Kernel).is_empty());
    assert!(verified.candidates(UpdateSelector::Tee).is_empty());
    let candidates = verified.candidates(UpdateSelector::Hypervisor);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].kind, bexos_update::ArtifactKind::Hypervisor);
    assert_eq!(candidates[0].target_id, "qemu-x86_64-monitor");
    candidates[0].verify_target(&target).unwrap();
}
