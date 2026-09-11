use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use bexos_crypto::blake3_256;

const MAGIC: &[u8; 8] = b"BXRPMB01";
const TAG_LEN: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecureStateBackendKind {
    HardwareRpmb,
    QemuPflashRpmb,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecureStateError {
    Missing,
    Replay,
    Corrupt,
    InvalidArgs,
    Io,
}

pub trait SecureBlockStorage {
    fn kind(&self) -> SecureStateBackendKind;
    fn read_record_copies(&self, name: &str) -> Result<[Option<Vec<u8>>; 2], SecureStateError>;
    fn write_record_copies(
        &mut self,
        name: &str,
        copies: [Vec<u8>; 2],
    ) -> Result<(), SecureStateError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureRecord {
    pub counter: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedSecureState<S> {
    storage: S,
    key: [u8; 32],
    observed: BTreeMap<String, u64>,
}

impl<S: SecureBlockStorage> AuthenticatedSecureState<S> {
    pub fn new(storage: S, key: [u8; 32]) -> Self {
        Self {
            storage,
            key,
            observed: BTreeMap::new(),
        }
    }

    pub fn backend_kind(&self) -> SecureStateBackendKind {
        self.storage.kind()
    }

    pub fn storage(&self) -> &S {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    pub fn read(&mut self, name: &str) -> Result<SecureRecord, SecureStateError> {
        validate_name(name)?;
        let copies = self.storage.read_record_copies(name)?;
        let mut best: Option<SecureRecord> = None;
        for copy in copies.into_iter().flatten() {
            let Ok(record) = decode_record(&self.key, name, &copy) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|current| record.counter > current.counter)
            {
                best = Some(record);
            }
        }
        let record = best.ok_or(SecureStateError::Corrupt)?;
        let last = self.observed.get(name).copied().unwrap_or(0);
        if record.counter < last {
            return Err(SecureStateError::Replay);
        }
        self.observed.insert(String::from(name), record.counter);
        Ok(record)
    }

    pub fn write(&mut self, name: &str, payload: &[u8]) -> Result<u64, SecureStateError> {
        validate_name(name)?;
        if payload.is_empty() || payload.len() > 1024 * 1024 {
            return Err(SecureStateError::InvalidArgs);
        }
        let next = match self.read(name) {
            Ok(record) => record.counter.saturating_add(1),
            Err(SecureStateError::Missing | SecureStateError::Corrupt) => 1,
            Err(error) => return Err(error),
        };
        let encoded = encode_record(&self.key, name, next, payload);
        self.storage
            .write_record_copies(name, [encoded.clone(), encoded])
            .map_err(|_| SecureStateError::Io)?;
        self.observed.insert(String::from(name), next);
        Ok(next)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QemuPflashRpmb {
    records: BTreeMap<String, [Option<Vec<u8>>; 2]>,
}

impl QemuPflashRpmb {
    pub fn new() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }

    pub fn corrupt_copy(&mut self, name: &str, copy: usize) -> Result<(), SecureStateError> {
        let Some(copies) = self.records.get_mut(name) else {
            return Err(SecureStateError::Missing);
        };
        let Some(record) = copies.get_mut(copy) else {
            return Err(SecureStateError::InvalidArgs);
        };
        let Some(bytes) = record else {
            return Err(SecureStateError::Missing);
        };
        if let Some(first) = bytes.first_mut() {
            *first ^= 0x80;
        }
        Ok(())
    }

    pub fn replace_copies(
        &mut self,
        name: &str,
        copies: [Option<Vec<u8>>; 2],
    ) -> Result<(), SecureStateError> {
        validate_name(name)?;
        self.records.insert(String::from(name), copies);
        Ok(())
    }
}

impl Default for QemuPflashRpmb {
    fn default() -> Self {
        Self::new()
    }
}

impl SecureBlockStorage for QemuPflashRpmb {
    fn kind(&self) -> SecureStateBackendKind {
        SecureStateBackendKind::QemuPflashRpmb
    }

    fn read_record_copies(&self, name: &str) -> Result<[Option<Vec<u8>>; 2], SecureStateError> {
        Ok(self.records.get(name).cloned().unwrap_or([None, None]))
    }

    fn write_record_copies(
        &mut self,
        name: &str,
        copies: [Vec<u8>; 2],
    ) -> Result<(), SecureStateError> {
        validate_name(name)?;
        self.records.insert(
            String::from(name),
            [Some(copies[0].clone()), Some(copies[1].clone())],
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareRpmbStorage;

impl SecureBlockStorage for HardwareRpmbStorage {
    fn kind(&self) -> SecureStateBackendKind {
        SecureStateBackendKind::HardwareRpmb
    }

    fn read_record_copies(&self, _name: &str) -> Result<[Option<Vec<u8>>; 2], SecureStateError> {
        Err(SecureStateError::Io)
    }

    fn write_record_copies(
        &mut self,
        _name: &str,
        _copies: [Vec<u8>; 2],
    ) -> Result<(), SecureStateError> {
        Err(SecureStateError::Io)
    }
}

fn validate_name(name: &str) -> Result<(), SecureStateError> {
    if name.is_empty()
        || name.len() > 96
        || name
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_' && b != b':')
    {
        Err(SecureStateError::InvalidArgs)
    } else {
        Ok(())
    }
}

fn encode_record(key: &[u8; 32], name: &str, counter: u64, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(MAGIC.len() + 8 + 4 + payload.len() + TAG_LEN);
    body.extend_from_slice(MAGIC);
    body.extend_from_slice(&counter.to_le_bytes());
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.extend_from_slice(payload);
    let tag = record_tag(key, name, &body);
    body.extend_from_slice(&tag);
    body
}

fn decode_record(
    key: &[u8; 32],
    name: &str,
    bytes: &[u8],
) -> Result<SecureRecord, SecureStateError> {
    if bytes.len() < MAGIC.len() + 8 + 4 + TAG_LEN || &bytes[..MAGIC.len()] != MAGIC {
        return Err(SecureStateError::Corrupt);
    }
    let tag_offset = bytes.len() - TAG_LEN;
    let body = &bytes[..tag_offset];
    if record_tag(key, name, body) != bytes[tag_offset..] {
        return Err(SecureStateError::Corrupt);
    }
    let mut cursor = MAGIC.len();
    let counter = read_u64(bytes, &mut cursor)?;
    let payload_len = read_u32(bytes, &mut cursor)? as usize;
    if cursor + payload_len != tag_offset {
        return Err(SecureStateError::Corrupt);
    }
    Ok(SecureRecord {
        counter,
        payload: bytes[cursor..cursor + payload_len].to_vec(),
    })
}

fn record_tag(key: &[u8; 32], name: &str, body: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(32 + name.len() + body.len());
    input.extend_from_slice(key);
    input.extend_from_slice(name.as_bytes());
    input.extend_from_slice(body);
    blake3_256(&input)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, SecureStateError> {
    if bytes.len().saturating_sub(*cursor) < 4 {
        return Err(SecureStateError::Corrupt);
    }
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[*cursor..*cursor + 4]);
    *cursor += 4;
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, SecureStateError> {
    if bytes.len().saturating_sub(*cursor) < 8 {
        return Err(SecureStateError::Corrupt);
    }
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[*cursor..*cursor + 8]);
    *cursor += 8;
    Ok(u64::from_le_bytes(value))
}
