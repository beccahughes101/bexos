use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

extern crate alloc;

use bexos_update::ArtifactKind;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const MAX_DELEGATION_DEPTH: usize = 8;
pub const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_TARGET_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TufError {
    InvalidJson,
    UnsupportedSpec,
    WrongRole,
    Expired,
    Rollback,
    UnknownKey,
    DuplicateKey,
    Threshold,
    BadSignature,
    BadHash,
    BadLength,
    MissingMetadata,
    MissingTarget,
    UnsafeTargetPath,
    UnsupportedKey,
    UnsupportedHash,
    DelegationLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedRoot {
    pub bytes: Vec<u8>,
    pub version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataSet {
    pub root: Vec<u8>,
    pub timestamp: Vec<u8>,
    pub snapshot: Vec<u8>,
    pub targets: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientState {
    pub root: TrustedRoot,
    pub timestamp_version: u64,
    pub snapshot_version: u64,
    pub targets_versions: BTreeMap<String, u64>,
}

impl ClientState {
    pub fn new(root: Vec<u8>) -> Result<Self, TufError> {
        let signed = signed_root(&root)?;
        Ok(TrustedRoot {
            bytes: root,
            version: signed.signed.version,
        }
        .into())
    }

    pub fn refresh(
        &mut self,
        metadata: &MetadataSet,
        now_unix_seconds: u64,
    ) -> Result<VerifiedRepository, TufError> {
        if metadata.root.len() > MAX_METADATA_BYTES
            || metadata.timestamp.len() > MAX_METADATA_BYTES
            || metadata.snapshot.len() > MAX_METADATA_BYTES
            || metadata
                .targets
                .values()
                .any(|bytes| bytes.len() > MAX_METADATA_BYTES)
        {
            return Err(TufError::BadLength);
        }

        let old_root = signed_root(&self.root.bytes)?;
        let root = signed_root(&metadata.root)?;
        verify_role(&metadata.root, &old_root.signed, "root")?;
        verify_role(&metadata.root, &root.signed, "root")?;
        if root.signed.version < self.root.version {
            return Err(TufError::Rollback);
        }
        expire(root.signed.expires.as_deref(), now_unix_seconds)?;
        self.root = TrustedRoot {
            bytes: metadata.root.clone(),
            version: root.signed.version,
        };

        let timestamp = signed_timestamp(&metadata.timestamp)?;
        verify_role(&metadata.timestamp, &root.signed, "timestamp")?;
        if timestamp.signed.version < self.timestamp_version {
            return Err(TufError::Rollback);
        }
        expire(timestamp.signed.expires.as_deref(), now_unix_seconds)?;
        self.timestamp_version = timestamp.signed.version;

        let snapshot_meta = timestamp
            .signed
            .meta
            .get("snapshot.json")
            .ok_or(TufError::MissingMetadata)?;
        verify_meta(snapshot_meta, &metadata.snapshot)?;
        let snapshot = signed_snapshot(&metadata.snapshot)?;
        verify_role(&metadata.snapshot, &root.signed, "snapshot")?;
        if snapshot.signed.version < self.snapshot_version {
            return Err(TufError::Rollback);
        }
        expire(snapshot.signed.expires.as_deref(), now_unix_seconds)?;
        self.snapshot_version = snapshot.signed.version;

        let root_targets = metadata
            .targets
            .get("targets")
            .or_else(|| metadata.targets.get("targets.json"))
            .ok_or(TufError::MissingMetadata)?;
        let targets_meta = snapshot
            .signed
            .meta
            .get("targets.json")
            .ok_or(TufError::MissingMetadata)?;
        verify_meta(targets_meta, root_targets)?;

        let mut verified = BTreeMap::new();
        let mut queue = VecDeque::new();
        queue.push_back((
            "targets".to_string(),
            "targets.json".to_string(),
            root_targets.clone(),
            0usize,
            None,
        ));
        while let Some((role_name, meta_name, bytes, depth, parent)) = queue.pop_front() {
            if depth > MAX_DELEGATION_DEPTH {
                return Err(TufError::DelegationLimit);
            }
            let role = signed_targets(&bytes)?;
            if role.signed.version < snapshot_meta.version.unwrap_or(0) {
                return Err(TufError::Rollback);
            }
            verify_targets_role(&bytes, &root.signed, &role_name, parent.as_ref())?;
            expire(role.signed.expires.as_deref(), now_unix_seconds)?;
            if role.signed.version < *self.targets_versions.get(&role_name).unwrap_or(&0) {
                return Err(TufError::Rollback);
            }
            self.targets_versions
                .insert(role_name.clone(), role.signed.version);
            for path in role.signed.targets.keys() {
                ensure_safe_target_path(path)?;
            }
            for delegated in role.signed.delegations.roles.iter().flatten() {
                let child_meta_name = format!("{}.json", delegated.name);
                let snapshot_meta = snapshot
                    .signed
                    .meta
                    .get(&child_meta_name)
                    .ok_or(TufError::MissingMetadata)?;
                let child_bytes = metadata
                    .targets
                    .get(&delegated.name)
                    .or_else(|| metadata.targets.get(&child_meta_name))
                    .ok_or(TufError::MissingMetadata)?;
                verify_meta(snapshot_meta, child_bytes)?;
                queue.push_back((
                    delegated.name.clone(),
                    child_meta_name,
                    child_bytes.clone(),
                    depth + 1,
                    Some(role.signed.clone()),
                ));
            }
            let _ = meta_name;
            verified.insert(role_name, role);
        }
        Ok(VerifiedRepository { targets: verified })
    }

    pub fn encode_state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_u64(&mut out, 1);
        put_bytes(&mut out, &self.root.bytes);
        put_u64(&mut out, self.root.version);
        put_u64(&mut out, self.timestamp_version);
        put_u64(&mut out, self.snapshot_version);
        put_u64(&mut out, self.targets_versions.len() as u64);
        for (role, version) in &self.targets_versions {
            put_bytes(&mut out, role.as_bytes());
            put_u64(&mut out, *version);
        }
        out
    }

    pub fn decode_state(bytes: &[u8]) -> Result<Self, TufError> {
        let mut cursor = StateCursor { bytes, offset: 0 };
        if cursor.u64()? != 1 {
            return Err(TufError::UnsupportedSpec);
        }
        let root_bytes = cursor.bytes(MAX_METADATA_BYTES)?.to_vec();
        let root_version = cursor.u64()?;
        let timestamp_version = cursor.u64()?;
        let snapshot_version = cursor.u64()?;
        let mut targets_versions = BTreeMap::new();
        for _ in 0..cursor.u64()?.min(256) {
            let role = core::str::from_utf8(cursor.bytes(256)?)
                .map_err(|_| TufError::InvalidJson)?
                .to_string();
            let version = cursor.u64()?;
            targets_versions.insert(role, version);
        }
        if cursor.offset != bytes.len() {
            return Err(TufError::InvalidJson);
        }
        Ok(Self {
            root: TrustedRoot {
                bytes: root_bytes,
                version: root_version,
            },
            timestamp_version,
            snapshot_version,
            targets_versions,
        })
    }
}

impl From<TrustedRoot> for ClientState {
    fn from(root: TrustedRoot) -> Self {
        Self {
            root,
            timestamp_version: 0,
            snapshot_version: 0,
            targets_versions: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedRepository {
    targets: BTreeMap<String, TargetsMetadata>,
}

impl VerifiedRepository {
    pub fn verified_target(&self, path: &str) -> Option<VerifiedTarget> {
        self.targets.iter().find_map(|(role, metadata)| {
            metadata
                .signed
                .targets
                .get(path)
                .map(|target| VerifiedTarget {
                    role: role.clone(),
                    path: path.to_string(),
                    length: target.length,
                    hashes: target.hashes.clone(),
                    url: target.custom.bexos.url.clone(),
                    generation: target.custom.bexos.generation.unwrap_or(0),
                })
        })
    }

    pub fn candidates(&self, selector: UpdateSelector) -> Vec<UpdateCandidate> {
        let mut out = Vec::new();
        for (role, metadata) in &self.targets {
            for (path, target) in &metadata.signed.targets {
                let Some(kind) = target.custom.bexos.kind.as_deref().and_then(kind_from_str) else {
                    continue;
                };
                let target_id = target
                    .custom
                    .bexos
                    .target_id
                    .clone()
                    .unwrap_or_else(|| path.clone());
                if selector.matches(kind, &target_id) {
                    out.push(UpdateCandidate {
                        role: role.clone(),
                        path: path.clone(),
                        target_id,
                        kind,
                        length: target.length,
                        url: target.custom.bexos.url.clone(),
                        generation: target.custom.bexos.generation.unwrap_or(0),
                        hashes: target.hashes.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.target_id.cmp(&b.target_id).then(a.path.cmp(&b.path)));
        out
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedTarget {
    pub role: String,
    pub path: String,
    pub length: u64,
    pub hashes: BTreeMap<String, String>,
    pub url: Option<String>,
    pub generation: u64,
}

impl VerifiedTarget {
    pub fn verify_target(&self, bytes: &[u8]) -> Result<(), TufError> {
        verify_target_bytes(self.length, &self.hashes, bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateSelector<'a> {
    All,
    Package(&'a str),
    Kernel,
    Tee,
    Hypervisor,
}

impl<'a> UpdateSelector<'a> {
    fn matches(self, kind: ArtifactKind, target_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Package(id) => kind == ArtifactKind::AppPackage && target_id == id,
            Self::Kernel => kind == ArtifactKind::Microkernel,
            Self::Tee => kind == ArtifactKind::TeeImage,
            Self::Hypervisor => kind == ArtifactKind::Hypervisor,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCandidate {
    pub role: String,
    pub path: String,
    pub target_id: String,
    pub kind: ArtifactKind,
    pub length: u64,
    pub url: Option<String>,
    pub generation: u64,
    pub hashes: BTreeMap<String, String>,
}

impl UpdateCandidate {
    pub fn verify_target(&self, bytes: &[u8]) -> Result<(), TufError> {
        verify_target_bytes(self.length, &self.hashes, bytes)
    }
}

fn verify_target_bytes(
    length: u64,
    hashes: &BTreeMap<String, String>,
    bytes: &[u8],
) -> Result<(), TufError> {
    if bytes.len() as u64 != length || length > MAX_TARGET_BYTES {
        return Err(TufError::BadLength);
    }
    for (name, expected) in hashes {
        match name.as_str() {
            "sha256" => {
                let actual = hex(&Sha256::digest(bytes));
                if &actual != expected {
                    return Err(TufError::BadHash);
                }
            }
            "blake3" => {
                let actual = blake3::hash(bytes).to_hex().to_string();
                if &actual != expected {
                    return Err(TufError::BadHash);
                }
            }
            _ => return Err(TufError::UnsupportedHash),
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct SignedEnvelope<T> {
    signatures: Vec<TufSignature>,
    signed: T,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct TufSignature {
    keyid: String,
    sig: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct RootMetadata {
    #[serde(rename = "_type")]
    role_type: String,
    spec_version: String,
    version: u64,
    expires: Option<String>,
    keys: BTreeMap<String, TufKey>,
    roles: BTreeMap<String, RootRole>,
    consistent_snapshot: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct TufKey {
    keytype: String,
    scheme: String,
    keyval: KeyValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct KeyValue {
    public: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct RootRole {
    keyids: Vec<String>,
    threshold: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct TimestampMetadata {
    #[serde(rename = "_type")]
    role_type: String,
    spec_version: String,
    version: u64,
    expires: Option<String>,
    meta: BTreeMap<String, MetaFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct SnapshotMetadata {
    #[serde(rename = "_type")]
    role_type: String,
    spec_version: String,
    version: u64,
    expires: Option<String>,
    meta: BTreeMap<String, MetaFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct TargetsMetadata {
    signatures: Vec<TufSignature>,
    signed: TargetsSigned,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct TargetsSigned {
    #[serde(rename = "_type")]
    role_type: String,
    spec_version: String,
    version: u64,
    expires: Option<String>,
    targets: BTreeMap<String, TargetFile>,
    #[serde(default)]
    delegations: Delegations,
}

#[derive(Clone, Debug, Deserialize, Default, Eq, PartialEq)]
struct Delegations {
    keys: BTreeMap<String, TufKey>,
    roles: Option<Vec<DelegatedRole>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct DelegatedRole {
    name: String,
    keyids: Vec<String>,
    threshold: u64,
    paths: Option<Vec<String>>,
    terminating: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct MetaFile {
    version: Option<u64>,
    length: Option<u64>,
    hashes: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct TargetFile {
    length: u64,
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    custom: TargetCustom,
}

#[derive(Clone, Debug, Deserialize, Default, Eq, PartialEq)]
struct TargetCustom {
    #[serde(default)]
    bexos: BexosTargetCustom,
}

#[derive(Clone, Debug, Deserialize, Default, Eq, PartialEq)]
struct BexosTargetCustom {
    kind: Option<String>,
    target_id: Option<String>,
    generation: Option<u64>,
    url: Option<String>,
}

fn signed_root(bytes: &[u8]) -> Result<SignedEnvelope<RootMetadata>, TufError> {
    let root: SignedEnvelope<RootMetadata> =
        serde_json::from_slice(bytes).map_err(|_| TufError::InvalidJson)?;
    if root.signed.role_type != "root" || !root.signed.spec_version.starts_with("1.") {
        return Err(TufError::UnsupportedSpec);
    }
    detect_duplicate_keys(&root.signed.keys)?;
    Ok(root)
}

fn signed_timestamp(bytes: &[u8]) -> Result<SignedEnvelope<TimestampMetadata>, TufError> {
    let role: SignedEnvelope<TimestampMetadata> =
        serde_json::from_slice(bytes).map_err(|_| TufError::InvalidJson)?;
    if role.signed.role_type != "timestamp" || !role.signed.spec_version.starts_with("1.") {
        return Err(TufError::WrongRole);
    }
    Ok(role)
}

fn signed_snapshot(bytes: &[u8]) -> Result<SignedEnvelope<SnapshotMetadata>, TufError> {
    let role: SignedEnvelope<SnapshotMetadata> =
        serde_json::from_slice(bytes).map_err(|_| TufError::InvalidJson)?;
    if role.signed.role_type != "snapshot" || !role.signed.spec_version.starts_with("1.") {
        return Err(TufError::WrongRole);
    }
    Ok(role)
}

fn signed_targets(bytes: &[u8]) -> Result<TargetsMetadata, TufError> {
    let role: TargetsMetadata = serde_json::from_slice(bytes).map_err(|_| TufError::InvalidJson)?;
    if role.signed.role_type != "targets" || !role.signed.spec_version.starts_with("1.") {
        return Err(TufError::WrongRole);
    }
    Ok(role)
}

fn verify_role(bytes: &[u8], root: &RootMetadata, role: &str) -> Result<(), TufError> {
    let envelope: SignedEnvelope<serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|_| TufError::InvalidJson)?;
    let role = root.roles.get(role).ok_or(TufError::WrongRole)?;
    verify_signatures(
        bytes,
        &envelope.signatures,
        &root.keys,
        &role.keyids,
        role.threshold,
    )
}

fn verify_targets_role(
    bytes: &[u8],
    root: &RootMetadata,
    role_name: &str,
    parent: Option<&TargetsSigned>,
) -> Result<(), TufError> {
    let envelope: TargetsMetadata =
        serde_json::from_slice(bytes).map_err(|_| TufError::InvalidJson)?;
    if role_name == "targets" {
        let role = root.roles.get("targets").ok_or(TufError::WrongRole)?;
        return verify_signatures(
            bytes,
            &envelope.signatures,
            &root.keys,
            &role.keyids,
            role.threshold,
        );
    }
    let parent = parent.ok_or(TufError::WrongRole)?;
    let delegated = parent
        .delegations
        .roles
        .as_ref()
        .and_then(|roles| roles.iter().find(|role| role.name == role_name))
        .ok_or(TufError::WrongRole)?;
    verify_signatures(
        bytes,
        &envelope.signatures,
        &parent.delegations.keys,
        &delegated.keyids,
        delegated.threshold,
    )
}

fn verify_signatures(
    bytes: &[u8],
    signatures: &[TufSignature],
    keys: &BTreeMap<String, TufKey>,
    keyids: &[String],
    threshold: u64,
) -> Result<(), TufError> {
    let signed = canonical_signed(bytes)?;
    let allowed: BTreeSet<_> = keyids.iter().collect();
    let mut accepted = BTreeSet::new();
    for sig in signatures {
        if !allowed.contains(&sig.keyid) || accepted.contains(&sig.keyid) {
            continue;
        }
        let key = keys.get(&sig.keyid).ok_or(TufError::UnknownKey)?;
        if verify_key_signature(key, &signed, &sig.sig)? {
            accepted.insert(&sig.keyid);
        }
    }
    if accepted.len() as u64 >= threshold {
        Ok(())
    } else {
        Err(TufError::Threshold)
    }
}

fn verify_key_signature(key: &TufKey, payload: &[u8], sig_hex: &str) -> Result<bool, TufError> {
    if key.keytype != "ed25519" || !matches!(key.scheme.as_str(), "ed25519" | "ed25519-ph") {
        return Err(TufError::UnsupportedKey);
    }
    let public = hex_bytes(&key.keyval.public)?;
    let signature = hex_bytes(sig_hex)?;
    let public: [u8; 32] = public.try_into().map_err(|_| TufError::UnsupportedKey)?;
    let signature: [u8; 64] = signature.try_into().map_err(|_| TufError::BadSignature)?;
    let verifying = VerifyingKey::from_bytes(&public).map_err(|_| TufError::UnsupportedKey)?;
    Ok(verifying
        .verify(payload, &Signature::from_bytes(&signature))
        .is_ok())
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

struct StateCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> StateCursor<'a> {
    fn u64(&mut self) -> Result<u64, TufError> {
        let raw = self.take(8)?;
        let mut value = [0; 8];
        value.copy_from_slice(raw);
        Ok(u64::from_le_bytes(value))
    }

    fn bytes(&mut self, max: usize) -> Result<&'a [u8], TufError> {
        let len = self.u64()? as usize;
        if len > max {
            return Err(TufError::BadLength);
        }
        self.take(len)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], TufError> {
        let end = self.offset.checked_add(len).ok_or(TufError::BadLength)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(TufError::BadLength)?;
        self.offset = end;
        Ok(bytes)
    }
}

fn canonical_signed(bytes: &[u8]) -> Result<Vec<u8>, TufError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| TufError::InvalidJson)?;
    let signed = value.get("signed").ok_or(TufError::InvalidJson)?;
    serde_json::to_vec(signed).map_err(|_| TufError::InvalidJson)
}

fn verify_meta(meta: &MetaFile, bytes: &[u8]) -> Result<(), TufError> {
    if let Some(length) = meta.length {
        if length != bytes.len() as u64 {
            return Err(TufError::BadLength);
        }
    }
    if let Some(hashes) = &meta.hashes {
        for (name, expected) in hashes {
            match name.as_str() {
                "sha256" => {
                    if hex(&Sha256::digest(bytes)) != *expected {
                        return Err(TufError::BadHash);
                    }
                }
                "blake3" => {
                    if blake3::hash(bytes).to_hex().to_string() != *expected {
                        return Err(TufError::BadHash);
                    }
                }
                _ => return Err(TufError::UnsupportedHash),
            }
        }
    }
    Ok(())
}

fn detect_duplicate_keys(keys: &BTreeMap<String, TufKey>) -> Result<(), TufError> {
    let mut seen = BTreeSet::new();
    for (id, key) in keys {
        let fingerprint = format!("{}:{}:{}", key.keytype, key.scheme, key.keyval.public);
        if !seen.insert(fingerprint) && keys.len() > 1 {
            return Err(TufError::DuplicateKey);
        }
        if id.is_empty() {
            return Err(TufError::UnknownKey);
        }
    }
    Ok(())
}

fn ensure_safe_target_path(path: &str) -> Result<(), TufError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(TufError::UnsafeTargetPath);
    }
    Ok(())
}

fn expire(expires: Option<&str>, now: u64) -> Result<(), TufError> {
    let Some(expires) = expires else {
        return Ok(());
    };
    let Some(ts) = parse_rfc3339_seconds(expires) else {
        return Err(TufError::Expired);
    };
    if ts < now {
        Err(TufError::Expired)
    } else {
        Ok(())
    }
}

fn kind_from_str(value: &str) -> Option<ArtifactKind> {
    match value {
        "app" | "APP_PACKAGE" => Some(ArtifactKind::AppPackage),
        "kernel" | "MICROKERNEL" => Some(ArtifactKind::Microkernel),
        "tee" | "TEE_IMAGE" => Some(ArtifactKind::TeeImage),
        "hypervisor" | "HYPERVISOR" => Some(ArtifactKind::Hypervisor),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0xf) as usize] as char);
    }
    out
}

fn hex_bytes(value: &str) -> Result<Vec<u8>, TufError> {
    if value.len() % 2 != 0 {
        return Err(TufError::BadHash);
    }
    let mut out = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks(2) {
        out.push((hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, TufError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(TufError::BadHash),
    }
}

fn parse_rfc3339_seconds(value: &str) -> Option<u64> {
    if value.len() < 20 || !value.ends_with('Z') {
        return None;
    }
    let year: i32 = value.get(0..4)?.parse().ok()?;
    let month: u32 = value.get(5..7)?.parse().ok()?;
    let day: u32 = value.get(8..10)?.parse().ok()?;
    let hour: u32 = value.get(11..13)?.parse().ok()?;
    let minute: u32 = value.get(14..16)?.parse().ok()?;
    let second: u32 = value.get(17..19)?.parse().ok()?;
    if value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    Some(days * 86_400 + hour as u64 * 3600 + minute as u64 * 60 + second as u64)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<u64> {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i32;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days >= 0).then_some(days as u64)
}
