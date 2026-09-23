use bexos_tuf::{ClientState, MetadataSet, TargetSearch, TufError, UpdateSelector};
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

#[test]
fn root_rotation_is_sequential_and_requires_both_thresholds() {
    let (root, _, _) = repo();
    let mut state = ClientState::new(root.clone()).unwrap();
    let mut value: Value = serde_json::from_slice::<Value>(&root).unwrap()["signed"].clone();
    value["version"] = json!(3);
    assert!(state.rotate_root(&signed(value.clone())).is_err());
    value["version"] = json!(2);
    value["roles"]["root"]["threshold"] = json!(2);
    assert!(state.rotate_root(&signed(value.clone())).is_err());
    assert_eq!(state.root.version, 1);
    value["roles"]["root"]["threshold"] = json!(1);
    state.rotate_root(&signed(value)).unwrap();
    assert_eq!(state.root.version, 2);
    assert!(state.rotate_root(&root).is_err());
}

#[test]
fn failed_refresh_does_not_persist_any_version_transition() {
    let (root, mut metadata, _) = repo();
    let mut state = ClientState::new(root).unwrap();
    let before = state.encode_state();
    metadata.targets.get_mut("targets").unwrap()[20] ^= 1;
    assert!(state.refresh(&metadata, 1).is_err());
    assert_eq!(state.encode_state(), before);
}

#[test]
fn expiration_is_required_and_timestamp_reference_version_is_exact() {
    let (root, mut metadata, _) = repo();
    let mut state = ClientState::new(root).unwrap();
    let mut timestamp =
        serde_json::from_slice::<Value>(&metadata.timestamp).unwrap()["signed"].clone();
    timestamp.as_object_mut().unwrap().remove("expires");
    metadata.timestamp = signed(timestamp.clone());
    assert!(state.refresh(&metadata, 1).is_err());
    timestamp["expires"] = json!("2099-01-01T00:00:00Z");
    timestamp["meta"]["snapshot.json"]["version"] = json!(2);
    metadata.timestamp = signed(timestamp);
    assert!(state.refresh(&metadata, 1).is_err());
}

#[test]
fn canonical_verification_rejects_duplicate_json_keys() {
    let (root, mut metadata, _) = repo();
    let mut state = ClientState::new(root).unwrap();
    let timestamp = String::from_utf8(metadata.timestamp).unwrap();
    metadata.timestamp = timestamp
        .replacen("\"version\":1", "\"version\":9,\"version\":1", 1)
        .into_bytes();
    assert!(state.refresh(&metadata, 1).is_err());
}

#[test]
fn invalid_top_level_metadata_cannot_advance_trusted_state() {
    let (root, mut metadata, _) = repo();
    let mut state = ClientState::new(root.clone()).unwrap();
    let mut zero_root = serde_json::from_slice::<Value>(&root).unwrap()["signed"].clone();
    zero_root["version"] = json!(0);
    assert!(ClientState::new(signed(zero_root)).is_err());
    let mut timestamp =
        serde_json::from_slice::<Value>(&metadata.timestamp).unwrap()["signed"].clone();
    timestamp["version"] = json!(0);
    let before = state.encode_state();
    assert!(
        state
            .accept_timestamp(&signed(timestamp.clone()), 1)
            .is_err()
    );
    assert_eq!(state.encode_state(), before);
    let mut snapshot =
        serde_json::from_slice::<Value>(&metadata.snapshot).unwrap()["signed"].clone();
    snapshot["meta"] = json!({});
    metadata.snapshot = signed(snapshot);
    timestamp["version"] = json!(1);
    timestamp["meta"]["snapshot.json"] = json!({
        "version": 1, "length": metadata.snapshot.len(),
        "hashes": {"sha256": hex(&sha2::Sha256::digest(&metadata.snapshot))}
    });
    let reference = state.accept_timestamp(&signed(timestamp), 1).unwrap();
    let before = state.encode_state();
    assert_eq!(
        state
            .accept_snapshot(&metadata.snapshot, &reference, 1)
            .unwrap_err(),
        TufError::MissingMetadata
    );
    assert_eq!(state.encode_state(), before);
}

fn delegation_repo(terminating: bool, pattern: &str) -> (Vec<u8>, MetadataSet) {
    let (root, mut metadata, _) = repo();
    let original =
        serde_json::from_slice::<Value>(&metadata.targets["targets"]).unwrap()["signed"].clone();
    let leaf = signed(original.clone());
    let mut empty = original.clone();
    empty["targets"] = json!({});
    let first = signed(empty.clone());
    let public = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
    empty["delegations"] = json!({"keys": {KEY_ID: {"keytype":"ed25519","scheme":"ed25519","keyval":{"public":hex(&public)}}}, "roles": [
        {"name":"first","keyids":[KEY_ID],"threshold":1,"terminating":terminating,"paths":[pattern]},
        {"name":"second","keyids":[KEY_ID],"threshold":1,"terminating":false,"paths":[pattern]}
    ]});
    metadata.targets = BTreeMap::from([
        ("targets".into(), signed(empty)),
        ("first".into(), first),
        ("second".into(), leaf),
    ]);
    let mut meta = serde_json::Map::new();
    for (role, bytes) in &metadata.targets {
        meta.insert(format!("{role}.json"),json!({"version":1,"length":bytes.len(),"hashes":{"sha256":hex(&sha2::Sha256::digest(bytes))}}));
    }
    metadata.snapshot = signed(
        json!({"_type":"snapshot","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","meta":meta}),
    );
    metadata.timestamp = signed(
        json!({"_type":"timestamp","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","meta":{"snapshot.json":{"version":1,"length":metadata.snapshot.len(),"hashes":{"sha256":hex(&sha2::Sha256::digest(&metadata.snapshot))}}}}),
    );
    (root, metadata)
}
#[test]
fn delegation_lookup_honors_order_termination_and_path_authority() {
    for (terminating, pattern, expected) in [
        (true, "apps/*", false),
        (false, "apps/*", true),
        (false, "fonts/*", false),
        (false, "apps/[a-f]emo.bex", true),
        (false, "apps/[!a-f]emo.bex", false),
        (false, "apps/[!x-z]emo.bex", true),
    ] {
        let (root, metadata) = delegation_repo(terminating, pattern);
        let verified = ClientState::new(root)
            .unwrap()
            .refresh(&metadata, 1)
            .unwrap();
        assert_eq!(
            verified.verified_target("apps/demo.bex").is_some(),
            expected
        );
    }
}

fn search_state(root: Vec<u8>, metadata: &MetadataSet) -> ClientState {
    let mut state = ClientState::new(root).unwrap();
    let snapshot = state.accept_timestamp(&metadata.timestamp, 1).unwrap();
    state
        .accept_snapshot(&metadata.snapshot, &snapshot, 1)
        .unwrap();
    ClientState::decode_state(&state.encode_state()).unwrap()
}

#[test]
fn incremental_search_fetches_only_authorized_roles_in_order() {
    for (terminating, pattern, roles, found) in [
        (true, "apps/*", vec!["targets", "first"], false),
        (false, "apps/*", vec!["targets", "first", "second"], true),
        (false, "fonts/*", vec!["targets"], false),
    ] {
        let (root, metadata) = delegation_repo(terminating, pattern);
        let mut state = search_state(root, &metadata);
        let mut search = TargetSearch::new(&state, &metadata.snapshot, "apps/demo.bex", 1).unwrap();
        let mut requested = Vec::new();
        while let Some(reference) = search.next_reference().unwrap() {
            let role = reference.name.strip_suffix(".json").unwrap();
            requested.push(role.to_string());
            search.accept(&mut state, &metadata.targets[role]).unwrap();
            // Each accepted role can survive a stop before the next download.
            state = ClientState::decode_state(&state.encode_state()).unwrap();
        }
        assert_eq!(requested, roles);
        assert_eq!(search.target().is_some(), found);
        assert_eq!(state.targets_versions.len(), requested.len());
    }
}

fn repin_targets(metadata: &mut MetadataSet) {
    let mut snapshot =
        serde_json::from_slice::<Value>(&metadata.snapshot).unwrap()["signed"].clone();
    for (name, bytes) in &metadata.targets {
        snapshot["meta"][format!("{name}.json")] = json!({
            "version": 1, "length": bytes.len(),
            "hashes": {"sha256": hex(&sha2::Sha256::digest(bytes))}
        });
    }
    metadata.snapshot = signed(snapshot);
    let mut timestamp =
        serde_json::from_slice::<Value>(&metadata.timestamp).unwrap()["signed"].clone();
    timestamp["meta"]["snapshot.json"] = json!({
        "version": 1, "length": metadata.snapshot.len(),
        "hashes": {"sha256": hex(&sha2::Sha256::digest(&metadata.snapshot))}
    });
    metadata.timestamp = signed(timestamp);
}

#[test]
fn incremental_search_skips_cycles_and_stops_at_the_first_target() {
    let (root, mut metadata) = delegation_repo(false, "apps/*");
    let top =
        serde_json::from_slice::<Value>(&metadata.targets["targets"]).unwrap()["signed"].clone();
    // first delegates back to itself, then to second. The snapshot also contains
    // second as a sibling: it must be downloaded once, through the first path.
    metadata.targets.insert("first".into(), signed(top));
    repin_targets(&mut metadata);
    let mut state = search_state(root, &metadata);
    let mut search = TargetSearch::new(&state, &metadata.snapshot, "apps/demo.bex", 1).unwrap();
    let mut roles = Vec::new();
    while let Some(reference) = search.next_reference().unwrap() {
        let role = reference.name.strip_suffix(".json").unwrap();
        roles.push(role.to_string());
        search.accept(&mut state, &metadata.targets[role]).unwrap();
    }
    assert_eq!(roles, ["targets", "first", "second"]);
    assert_eq!(search.target().unwrap().role, "second");
}

#[test]
fn incremental_search_rejects_tampering_versions_and_expiration_before_state_changes() {
    for failure in ["hash", "version", "expiry", "signature"] {
        let (root, mut metadata) = delegation_repo(false, "apps/*");
        let mut first =
            serde_json::from_slice::<Value>(&metadata.targets["first"]).unwrap()["signed"].clone();
        if failure == "version" {
            first["version"] = json!(2);
        }
        if failure == "expiry" {
            first["expires"] = json!("1970-01-01T00:00:00Z");
        }
        let mut bytes = signed(first);
        if failure == "signature" {
            let mut envelope = serde_json::from_slice::<Value>(&bytes).unwrap();
            envelope["signatures"][0]["sig"] = json!("00".repeat(64));
            bytes = serde_json::to_vec(&envelope).unwrap();
        }
        metadata.targets.insert("first".into(), bytes);
        repin_targets(&mut metadata);
        let mut state = search_state(root, &metadata);
        let mut search = TargetSearch::new(&state, &metadata.snapshot, "apps/demo.bex", 1).unwrap();
        search
            .accept(&mut state, &metadata.targets["targets"])
            .unwrap();
        let before = state.encode_state();
        let mut bytes = metadata.targets["first"].clone();
        if failure == "hash" {
            bytes.push(b' ');
        }
        assert!(search.accept(&mut state, &bytes).is_err(), "{failure}");
        assert_eq!(before, state.encode_state(), "{failure}");
    }
}

#[test]
fn same_version_metadata_changes_are_rejected_across_state_reopen() {
    let (root, mut metadata, _) = repo();
    let mut state = ClientState::new(root).unwrap();
    state.refresh(&metadata, 1).unwrap();
    let mut state = ClientState::decode_state(&state.encode_state()).unwrap();
    let mut timestamp =
        serde_json::from_slice::<Value>(&metadata.timestamp).unwrap()["signed"].clone();
    timestamp["expires"] = json!("2098-01-01T00:00:00Z");
    metadata.timestamp = signed(timestamp);
    assert!(state.refresh(&metadata, 1).is_err());
}

#[test]
fn snapshot_references_survive_interrupted_downloads_and_reopen() {
    let (root, metadata, _) = repo();
    let mut state = ClientState::new(root).unwrap();
    let mut snapshot =
        serde_json::from_slice::<Value>(&metadata.snapshot).unwrap()["signed"].clone();
    snapshot["meta"]["unfetched.json"] = snapshot["meta"]["targets.json"].clone();
    snapshot["meta"]["unfetched.json"]["version"] = json!(9);
    let bytes = signed(snapshot.clone());
    let reference = |bytes: &[u8], version| bexos_tuf::MetadataReference {
        name: "snapshot.json".into(),
        version,
        length: bytes.len() as u64,
        sha256: sha2::Sha256::digest(bytes).into(),
    };
    state
        .accept_snapshot(&bytes, &reference(&bytes, 1), 1)
        .unwrap();
    let mut state = ClientState::decode_state(&state.encode_state()).unwrap();
    let before = state.encode_state();
    snapshot["version"] = json!(2);
    snapshot["meta"]["unfetched.json"]["version"] = json!(8);
    let bytes = signed(snapshot.clone());
    assert!(matches!(
        state.accept_snapshot(&bytes, &reference(&bytes, 2), 1),
        Err(TufError::Rollback)
    ));
    assert_eq!(state.encode_state(), before);
    snapshot["meta"]
        .as_object_mut()
        .unwrap()
        .remove("unfetched.json");
    let bytes = signed(snapshot);
    assert!(matches!(
        state.accept_snapshot(&bytes, &reference(&bytes, 2), 1),
        Err(TufError::Rollback)
    ));
    assert_eq!(state.encode_state(), before);
}

#[test]
fn online_key_rotation_recovers_fast_forward_versions_only_after_root_verification() {
    let (root, metadata, _) = repo();
    let mut state = ClientState::new(root.clone()).unwrap();
    state.refresh(&metadata, 1).unwrap();
    let targets = state.targets_versions.clone();
    state.timestamp_version = 1000;
    state.snapshot_version = 1000;
    let mut new = serde_json::from_slice::<Value>(&root).unwrap()["signed"].clone();
    new["version"] = json!(2);
    state.rotate_root(&signed(new.clone())).unwrap();
    assert_eq!(state.timestamp_version, 1000);
    new["version"] = json!(3);
    new["keys"]["rotated"] = new["keys"][KEY_ID].clone();
    new["keys"]["rotated"]["keyval"]["public"] = json!(hex(&SigningKey::from_bytes(&[17; 32])
        .verifying_key()
        .to_bytes()));
    new["roles"]["timestamp"]["keyids"] = json!(["rotated"]);
    let mut invalid: Value = serde_json::from_slice(&signed(new.clone())).unwrap();
    invalid["signatures"][0]["sig"] = json!("00".repeat(64));
    assert!(
        state
            .rotate_root(&serde_json::to_vec(&invalid).unwrap())
            .is_err()
    );
    assert_eq!(state.timestamp_version, 1000);
    state.rotate_root(&signed(new)).unwrap();
    assert_eq!(state.timestamp_version, 0);
    assert_eq!(state.snapshot_version, 0);
    assert_eq!(state.targets_versions, targets);
    assert_eq!(
        ClientState::decode_state(&state.encode_state()).unwrap(),
        state
    );
}
