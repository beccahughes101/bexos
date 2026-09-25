use crate::{Error, Result};
use bexos_migration::codec::{Decoder, Encoder};
use bexos_pkg_client::{ArtifactKind, BlobDigest, HashType};
use bexos_tuf::ClientState;
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct Grant {
    pub sha256: [u8; 32],
    pub blake3: [u8; 32],
    pub kind: ArtifactKind,
    pub tag: String,
    pub length: u64,
    pub manifest_sha256: Option<[u8; 32]>,
    pub manifest_length: u64,
}
#[derive(Clone)]
pub struct RepositoryState {
    pub revision: u64,
    pub time_floor: u64,
    pub tuf: ClientState,
    pub grants: Vec<Grant>,
}
impl RepositoryState {
    pub fn new(root: &[u8]) -> Result<Self> {
        Ok(Self {
            revision: 0,
            time_floor: 0,
            tuf: ClientState::new(root.to_vec()).map_err(|_| Error::VerifyFailed)?,
            grants: Vec::new(),
        })
    }
    pub fn grant(&self, digest: &BlobDigest) -> Option<&Grant> {
        self.grants.iter().find(|grant| match digest.hash_type {
            HashType::Sha256 => grant.sha256 == digest.digest,
            HashType::Blake3 => grant.blake3 == digest.digest,
        })
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Encoder::new();
        writer.word(4);
        writer.word(self.revision);
        writer.word(self.time_floor);
        writer.bytes(&self.tuf.encode_state());
        writer.word(self.grants.len() as u64);
        for grant in &self.grants {
            writer.bytes(&grant.sha256);
            writer.bytes(&grant.blake3);
            writer.word(grant.kind as u64);
            writer.text(&grant.tag);
            writer.word(grant.length);
            writer.word(grant.manifest_sha256.is_some() as u64);
            if let Some(digest) = grant.manifest_sha256 {
                writer.bytes(&digest);
                writer.word(grant.manifest_length);
            }
        }
        writer.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let decode = || -> core::result::Result<Self, bexos_migration::Error> {
            let mut reader = Decoder::new(bytes);
            let version = reader.word()?;
            if !matches!(version, 3 | 4) {
                return Err(bexos_migration::Error::UnsupportedVersion);
            }
            let revision = reader.word()?;
            let time_floor = reader.word()?;
            let tuf = ClientState::decode_state(reader.bytes(2 * 1024 * 1024)?)
                .map_err(|_| bexos_migration::Error::InvalidData)?;
            let mut grants = Vec::new();
            for _ in 0..reader.count(4096)? {
                let sha256 = reader
                    .bytes(32)?
                    .try_into()
                    .map_err(|_| bexos_migration::Error::InvalidData)?;
                let blake3 = reader
                    .bytes(32)?
                    .try_into()
                    .map_err(|_| bexos_migration::Error::InvalidData)?;
                let kind = match reader.word()? {
                    1 => ArtifactKind::Application,
                    2 => ArtifactKind::Driver,
                    3 => ArtifactKind::Font,
                    4 => ArtifactKind::Firmware,
                    5 => ArtifactKind::NetworkExtension,
                    6 => ArtifactKind::Container,
                    _ => return Err(bexos_migration::Error::InvalidData),
                };
                let tag = reader.text(64)?.into();
                let length = reader.word()?;
                let (manifest_sha256, manifest_length) = if version >= 4 && reader.flag()? {
                    (
                        Some(
                            reader
                                .bytes(32)?
                                .try_into()
                                .map_err(|_| bexos_migration::Error::InvalidData)?,
                        ),
                        reader.word()?,
                    )
                } else {
                    (None, 0)
                };
                grants.push(Grant {
                    sha256,
                    blake3,
                    kind,
                    tag,
                    length,
                    manifest_sha256,
                    manifest_length,
                });
            }
            reader.finish()?;
            Ok(Self {
                revision,
                time_floor,
                tuf,
                grants,
            })
        };
        decode().map_err(|_| Error::VerifyFailed)
    }
    pub fn floors(&self) -> BTreeMap<String, u64> {
        let mut floors = self.tuf.targets_versions.clone();
        floors.insert("$root".into(), self.tuf.root.version);
        floors.insert("$timestamp".into(), self.tuf.timestamp_version);
        floors.insert("$snapshot".into(), self.tuf.snapshot_version);
        floors.insert("$time".into(), self.time_floor);
        floors
    }
}

/// Implementations must atomically compare revision and persist to Trusty/RPMB.
/// The production daemon never substitutes a memory implementation.
pub trait SecureStore {
    fn load(&mut self, repository: &str) -> Result<Option<RepositoryState>>;
    fn commit(
        &mut self,
        repository: &str,
        expected_revision: u64,
        state: &RepositoryState,
    ) -> Result<()>;
}

/// Deliberate fail-closed state for products without an attached secure endpoint.
pub struct UnavailableStore;
impl SecureStore for UnavailableStore {
    fn load(&mut self, _: &str) -> Result<Option<RepositoryState>> {
        Err(Error::Unavailable)
    }
    fn commit(&mut self, _: &str, _: u64, _: &RepositoryState) -> Result<()> {
        Err(Error::Unavailable)
    }
}

const STATES: redb::TableDefinition<&str, &[u8]> =
    redb::TableDefinition::new("pkg_secure_payloads_v1");
pub struct TrustyStore {
    pub endpoint: bexos_userspace::Channel,
    database: redb::Database,
}
impl TrustyStore {
    pub fn put_public(&mut self, bytes: &[u8]) -> Result<[u8; 32]> {
        use sha2::{Digest, Sha256};
        if bytes.len() > 32 * 1024 {
            return Err(Error::ResourceExhausted);
        }
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let key = format!("public/{}", crate::oci::hex(&digest));
        let transaction = self.database.begin_write().map_err(|_| Error::Io)?;
        transaction
            .open_table(STATES)
            .map_err(|_| Error::Io)?
            .insert(key.as_str(), bytes)
            .map_err(|_| Error::Io)?;
        transaction.commit().map_err(|_| Error::Io)?;
        Ok(digest)
    }
    pub fn get_public(&self, digest: &[u8]) -> Result<Vec<u8>> {
        use redb::ReadableDatabase;
        use sha2::{Digest, Sha256};
        if digest.len() != 32 {
            return Err(Error::VerifyFailed);
        }
        let transaction = self.database.begin_read().map_err(|_| Error::Io)?;
        let table = transaction.open_table(STATES).map_err(|_| Error::Io)?;
        let key = format!("public/{}", crate::oci::hex(digest));
        let value = table
            .get(key.as_str())
            .map_err(|_| Error::Io)?
            .ok_or(Error::NotFound)?;
        if Sha256::digest(value.value()).as_slice() != digest {
            return Err(Error::VerifyFailed);
        }
        Ok(value.value().to_vec())
    }
    pub fn new(endpoint: bexos_userspace::Channel, database: redb::Database) -> Result<Self> {
        let transaction = database.begin_write().map_err(|_| Error::Io)?;
        transaction.open_table(STATES).map_err(|_| Error::Io)?;
        transaction.commit().map_err(|_| Error::Io)?;
        Ok(Self { endpoint, database })
    }
    pub fn exchange(
        &mut self,
        name: &str,
        update: Option<(u64, [u64; 4], &[u8])>,
    ) -> Result<Option<Vec<u8>>> {
        use sha2::{Digest, Sha256};
        use tee_manager_fidl::{FidlDecode, FidlEncode};
        let key: [u8; 32] = Sha256::digest(name.as_bytes()).into();
        let mut request = vec![0; 96];
        request[..8].copy_from_slice(b"PKGSEC01");
        request[8..12].copy_from_slice(&(if update.is_some() { 2u32 } else { 1u32 }).to_le_bytes());
        request[16..48].copy_from_slice(&key);
        if let Some((revision, floors, payload)) = update {
            if payload.len() > 3072 {
                return Err(Error::ResourceExhausted);
            }
            request[48..56].copy_from_slice(&revision.to_le_bytes());
            for (i, floor) in floors.into_iter().enumerate() {
                request[56 + i * 8..64 + i * 8].copy_from_slice(&floor.to_le_bytes());
            }
            request[88..92].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            request.extend_from_slice(payload);
        }
        let secret = crate::credentials::Secret::new(request);
        let mut wire = vec![0; 4096];
        let encoded = tee_manager_fidl::TeeManagerExchangePackageStateRequest {
            request: secret.bytes(),
        }
        .encode(&mut wire, &mut [])
        .map_err(|_| Error::InvalidArgs)?;
        let result =
            bexos_userspace::Rpc(self.endpoint).call_raw(14, &wire[..encoded.bytes], &[], true);
        for byte in &mut wire {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        let message = result.map_err(|_| Error::Unavailable)?;
        for handle in &message.handles {
            let _ = bexos_userspace::Memory::close(*handle);
        }
        if !message.handles.is_empty() {
            return Err(Error::VerifyFailed);
        }
        let message_bytes = crate::credentials::Secret::new(message.bytes);
        let response = tee_manager_fidl::TeeManagerExchangePackageStateResponse::decode(
            message_bytes.bytes(),
            &[],
        )
        .map_err(|_| Error::VerifyFailed)?;
        if response.status != tee_manager_fidl::TeeStatus::Ok {
            return Err(Error::Unavailable);
        }
        let bytes = response.response;
        if bytes.len() < 96 || &bytes[..8] != b"PKGSEC01" {
            return Err(Error::VerifyFailed);
        }
        match u32::from_le_bytes(bytes[12..16].try_into().unwrap()) {
            0 => {}
            1 if update.is_none() => return Ok(None),
            2 => return Err(Error::VerifyFailed),
            _ => return Err(Error::Unavailable),
        }
        let length = u32::from_le_bytes(bytes[88..92].try_into().unwrap()) as usize;
        if bytes[16..48] != key || bytes.len() != 96 + length || length > 3072 {
            return Err(Error::VerifyFailed);
        }
        if let Some((revision, floors, payload)) = update {
            if u64::from_le_bytes(bytes[48..56].try_into().unwrap())
                != revision.checked_add(1).ok_or(Error::VerifyFailed)?
                || &bytes[96..] != payload
            {
                return Err(Error::VerifyFailed);
            }
            for (i, floor) in floors.into_iter().enumerate() {
                if u64::from_le_bytes(bytes[56 + i * 8..64 + i * 8].try_into().unwrap()) != floor {
                    return Err(Error::VerifyFailed);
                }
            }
        }
        Ok(Some(bytes.to_vec()))
    }
}
impl SecureStore for TrustyStore {
    fn load(&mut self, repository: &str) -> Result<Option<RepositoryState>> {
        use redb::ReadableDatabase;
        use sha2::{Digest, Sha256};
        let Some(record) = self.exchange(&format!("repository:{repository}"), None)? else {
            return Ok(None);
        };
        if record.len() != 128 {
            return Err(Error::VerifyFailed);
        }
        let key = format!("{repository}/{}", crate::oci::hex(&record[96..]));
        let transaction = self.database.begin_read().map_err(|_| Error::Io)?;
        let table = transaction.open_table(STATES).map_err(|_| Error::Io)?;
        let value = table
            .get(key.as_str())
            .map_err(|_| Error::Io)?
            .ok_or(Error::VerifyFailed)?;
        if Sha256::digest(value.value()).as_slice() != &record[96..] {
            return Err(Error::VerifyFailed);
        }
        let state = RepositoryState::decode(value.value())?;
        if state.revision != u64::from_le_bytes(record[48..56].try_into().unwrap()) {
            return Err(Error::VerifyFailed);
        }
        let floors = [
            state.tuf.root.version,
            state.tuf.timestamp_version,
            state.tuf.snapshot_version,
            state.time_floor,
        ];
        for (i, floor) in floors.into_iter().enumerate() {
            if floor != u64::from_le_bytes(record[56 + i * 8..64 + i * 8].try_into().unwrap()) {
                return Err(Error::VerifyFailed);
            }
        }
        Ok(Some(state))
    }
    fn commit(
        &mut self,
        repository: &str,
        expected_revision: u64,
        state: &RepositoryState,
    ) -> Result<()> {
        use redb::ReadableTable;
        use sha2::{Digest, Sha256};
        if state.revision
            != expected_revision
                .checked_add(1)
                .ok_or(Error::VerifyFailed)?
        {
            return Err(Error::VerifyFailed);
        }
        let payload = state.encode();
        let digest = Sha256::digest(&payload);
        let key = format!("{repository}/{}", crate::oci::hex(&digest));
        let transaction = self.database.begin_write().map_err(|_| Error::Io)?;
        transaction
            .open_table(STATES)
            .map_err(|_| Error::Io)?
            .insert(key.as_str(), payload.as_slice())
            .map_err(|_| Error::Io)?;
        transaction.commit().map_err(|_| Error::Io)?;
        self.exchange(
            &format!("repository:{repository}"),
            Some((
                expected_revision,
                [
                    state.tuf.root.version,
                    state.tuf.timestamp_version,
                    state.tuf.snapshot_version,
                    state.time_floor,
                ],
                &digest,
            )),
        )?;
        // The new payload is durable before its hash is committed in Trusty.
        // Only then may obsolete payloads be removed; a crash at any point is safe.
        let transaction = self.database.begin_write().map_err(|_| Error::Io)?;
        {
            let mut table = transaction.open_table(STATES).map_err(|_| Error::Io)?;
            let prefix = format!("{repository}/");
            let obsolete = table
                .iter()
                .map_err(|_| Error::Io)?
                .map(|entry| {
                    entry
                        .map(|(k, _)| k.value().to_string())
                        .map_err(|_| Error::Io)
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|old| old.starts_with(&prefix) && *old != key)
                .collect::<Vec<_>>();
            for old in obsolete {
                table.remove(old.as_str()).map_err(|_| Error::Io)?;
            }
        }
        transaction.commit().map_err(|_| Error::Io)
    }
}
