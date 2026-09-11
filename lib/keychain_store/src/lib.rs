#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
mod checkpoint;

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

pub const MAX_ALIAS_LEN: usize = 128;
pub const MAX_SECRET_LEN: usize = 4096;
pub const MAX_DIGEST_LEN: usize = 64;
pub const MAX_KEY_CHARACTERISTICS_LEN: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeychainScope {
    System,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAlgorithm {
    Ed25519,
    EcdsaP256,
    Aes256Gcm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyFlags(pub u16);

impl KeyFlags {
    pub const REQUIRE_USER_AUTH: Self = Self(0x0001);
    pub const EXPORTABLE: Self = Self(0x0002);
    pub const HARDWARE_BACKED: Self = Self(0x0004);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretRecord {
    pub alias: String,
    /// Plaintext for legacy/software-only records, ciphertext for
    /// HARDWARE_BACKED envelope records.
    pub secret: Vec<u8>,
    pub flags: KeyFlags,
    pub generation: u64,
    pub envelope_key_blob: Vec<u8>,
    pub nonce: Vec<u8>,
    pub characteristics: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRecord {
    pub alias: String,
    pub algorithm: KeyAlgorithm,
    pub flags: KeyFlags,
    pub public_key: Vec<u8>,
    pub opaque_key_blob: Vec<u8>,
    /// Canonical CBOR returned by KeyMint. This is metadata only; private key
    /// material remains inside the opaque blob protected by Trusty storage.
    pub characteristics: Vec<u8>,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeychainStoreError {
    InvalidAlias,
    InvalidSecret,
    InvalidDigest,
    AlreadyExists,
    NotFound,
    Unsupported,
    AccessDenied,
    CorruptRecord,
    Storage,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryKeychainStore {
    secrets: Vec<SecretRecord>,
    keys: Vec<KeyRecord>,
    generation: u64,
}

impl MemoryKeychainStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list_secrets(&self) -> &[SecretRecord] {
        &self.secrets
    }

    pub fn store_secret(
        &mut self,
        alias: impl Into<String>,
        secret: &[u8],
        flags: KeyFlags,
    ) -> Result<(), KeychainStoreError> {
        let alias = alias.into();
        validate_alias(&alias)?;
        validate_secret(secret)?;
        if flags.contains(KeyFlags::HARDWARE_BACKED) {
            return Err(KeychainStoreError::InvalidSecret);
        }
        self.generation = self.generation.saturating_add(1);
        if let Some(existing) = self.secrets.iter_mut().find(|entry| entry.alias == alias) {
            existing.secret = secret.to_vec();
            existing.flags = flags;
            existing.generation = self.generation;
        } else {
            self.secrets.push(SecretRecord {
                alias,
                secret: secret.to_vec(),
                flags,
                generation: self.generation,
                envelope_key_blob: Vec::new(),
                nonce: Vec::new(),
                characteristics: Vec::new(),
            });
            self.secrets
                .sort_by(|left, right| left.alias.cmp(&right.alias));
        }
        Ok(())
    }

    pub fn get_secret(&self, alias: &str) -> Result<&[u8], KeychainStoreError> {
        validate_alias(alias)?;
        self.secrets
            .iter()
            .find(|entry| entry.alias == alias)
            .map(|entry| entry.secret.as_slice())
            .ok_or(KeychainStoreError::NotFound)
    }

    pub fn put_secret_record(
        &mut self,
        mut record: SecretRecord,
    ) -> Result<(), KeychainStoreError> {
        validate_secret_record(&record)?;
        self.generation = self.generation.saturating_add(1);
        record.generation = self.generation;
        if let Some(existing) = self
            .secrets
            .iter_mut()
            .find(|entry| entry.alias == record.alias)
        {
            *existing = record;
        } else {
            self.secrets.push(record);
            self.secrets
                .sort_by(|left, right| left.alias.cmp(&right.alias));
        }
        Ok(())
    }

    pub fn secret_record(&self, alias: &str) -> Result<SecretRecord, KeychainStoreError> {
        validate_alias(alias)?;
        self.secrets
            .iter()
            .find(|entry| entry.alias == alias)
            .cloned()
            .ok_or(KeychainStoreError::NotFound)
    }

    pub fn delete_secret(&mut self, alias: &str) -> Result<(), KeychainStoreError> {
        validate_alias(alias)?;
        let Some(index) = self.secrets.iter().position(|entry| entry.alias == alias) else {
            return Err(KeychainStoreError::NotFound);
        };
        self.secrets.remove(index);
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }

    pub fn generate_key(
        &mut self,
        alias: impl Into<String>,
        algorithm: KeyAlgorithm,
        flags: KeyFlags,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        let alias = alias.into();
        validate_alias(&alias)?;
        if flags.contains(KeyFlags::HARDWARE_BACKED) || algorithm != KeyAlgorithm::Ed25519 {
            return Err(KeychainStoreError::Unsupported);
        }
        Err(KeychainStoreError::Unsupported)
    }

    pub fn register_hardware_key(
        &mut self,
        alias: impl Into<String>,
        algorithm: KeyAlgorithm,
        flags: KeyFlags,
        public_key: &[u8],
        opaque_key_blob: &[u8],
        characteristics: &[u8],
    ) -> Result<(), KeychainStoreError> {
        let alias = alias.into();
        validate_alias(&alias)?;
        if !flags.contains(KeyFlags::HARDWARE_BACKED)
            || !valid_hardware_public_key(algorithm, public_key)
            || opaque_key_blob.is_empty()
            || opaque_key_blob.len() > 4096
            || characteristics.is_empty()
            || characteristics.len() > MAX_KEY_CHARACTERISTICS_LEN
        {
            return Err(KeychainStoreError::InvalidSecret);
        }
        if self.keys.iter().any(|entry| entry.alias == alias) {
            return Err(KeychainStoreError::AlreadyExists);
        }
        self.generation = self.generation.saturating_add(1);
        self.keys.push(KeyRecord {
            alias,
            algorithm,
            flags,
            public_key: public_key.to_vec(),
            opaque_key_blob: opaque_key_blob.to_vec(),
            characteristics: characteristics.to_vec(),
            generation: self.generation,
        });
        self.keys
            .sort_by(|left, right| left.alias.cmp(&right.alias));
        Ok(())
    }

    pub fn key_flags(&self, alias: &str) -> Result<KeyFlags, KeychainStoreError> {
        validate_alias(alias)?;
        self.keys
            .iter()
            .find(|entry| entry.alias == alias)
            .map(|entry| entry.flags)
            .ok_or(KeychainStoreError::NotFound)
    }

    pub fn key_algorithm(&self, alias: &str) -> Result<KeyAlgorithm, KeychainStoreError> {
        validate_alias(alias)?;
        self.keys
            .iter()
            .find(|entry| entry.alias == alias)
            .map(|entry| entry.algorithm)
            .ok_or(KeychainStoreError::NotFound)
    }

    pub fn key_record(&self, alias: &str) -> Result<KeyRecord, KeychainStoreError> {
        validate_alias(alias)?;
        self.keys
            .iter()
            .find(|entry| entry.alias == alias)
            .cloned()
            .ok_or(KeychainStoreError::NotFound)
    }

    pub fn sign(&self, alias: &str, digest: &[u8]) -> Result<Vec<u8>, KeychainStoreError> {
        validate_alias(alias)?;
        validate_digest(digest)?;
        Err(KeychainStoreError::Unsupported)
    }
}

pub fn validate_alias(alias: &str) -> Result<(), KeychainStoreError> {
    if alias.is_empty()
        || alias.len() > MAX_ALIAS_LEN
        || alias
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_' && b != b':')
    {
        Err(KeychainStoreError::InvalidAlias)
    } else {
        Ok(())
    }
}

pub fn validate_secret(secret: &[u8]) -> Result<(), KeychainStoreError> {
    if secret.is_empty() || secret.len() > MAX_SECRET_LEN {
        Err(KeychainStoreError::InvalidSecret)
    } else {
        Ok(())
    }
}

pub fn validate_secret_record(record: &SecretRecord) -> Result<(), KeychainStoreError> {
    validate_alias(&record.alias)?;
    if record.flags.contains(KeyFlags::HARDWARE_BACKED) {
        if record.secret.len() < 16
            || record.secret.len() > MAX_SECRET_LEN + 16
            || record.envelope_key_blob.is_empty()
            || record.envelope_key_blob.len() > 4096
            || record.nonce.len() != 12
            || record.characteristics.is_empty()
            || record.characteristics.len() > MAX_KEY_CHARACTERISTICS_LEN
        {
            return Err(KeychainStoreError::InvalidSecret);
        }
    } else {
        validate_secret(&record.secret)?;
        if !record.envelope_key_blob.is_empty()
            || !record.nonce.is_empty()
            || !record.characteristics.is_empty()
        {
            return Err(KeychainStoreError::InvalidSecret);
        }
    }
    Ok(())
}

pub fn validate_digest(digest: &[u8]) -> Result<(), KeychainStoreError> {
    if digest.is_empty() || digest.len() > MAX_DIGEST_LEN {
        Err(KeychainStoreError::InvalidDigest)
    } else {
        Ok(())
    }
}

fn encode_secret(record: &SecretRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &record.alias);
    put_bytes(&mut out, 2, &record.secret);
    put_varint(&mut out, 3, record.flags.0 as u64);
    put_varint(&mut out, 4, record.generation);
    put_bytes(&mut out, 5, &record.envelope_key_blob);
    put_bytes(&mut out, 6, &record.nonce);
    put_bytes(&mut out, 7, &record.characteristics);
    out
}

fn decode_secret(bytes: &[u8]) -> Result<SecretRecord, KeychainStoreError> {
    let mut record = SecretRecord {
        alias: String::new(),
        secret: Vec::new(),
        flags: KeyFlags::empty(),
        generation: 0,
        envelope_key_blob: Vec::new(),
        nonce: Vec::new(),
        characteristics: Vec::new(),
    };
    read_fields(bytes, |field| {
        match field.number {
            1 => record.alias = field.string()?,
            2 => record.secret = field.bytes()?.to_vec(),
            3 => {
                record.flags = KeyFlags(
                    u16::try_from(field.varint()?)
                        .map_err(|_| KeychainStoreError::CorruptRecord)?,
                )
            }
            4 => record.generation = field.varint()?,
            5 => record.envelope_key_blob = field.bytes()?.to_vec(),
            6 => record.nonce = field.bytes()?.to_vec(),
            7 => record.characteristics = field.bytes()?.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    validate_secret_record(&record)?;
    Ok(record)
}

fn encode_key(record: &KeyRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &record.alias);
    put_varint(&mut out, 2, key_algorithm_code(record.algorithm));
    put_varint(&mut out, 3, record.flags.0 as u64);
    put_bytes(&mut out, 4, &record.public_key);
    put_varint(&mut out, 5, record.generation);
    put_bytes(&mut out, 6, &record.opaque_key_blob);
    put_bytes(&mut out, 7, &record.characteristics);
    out
}

fn decode_key(bytes: &[u8]) -> Result<KeyRecord, KeychainStoreError> {
    let mut record = KeyRecord {
        alias: String::new(),
        algorithm: KeyAlgorithm::Ed25519,
        flags: KeyFlags::empty(),
        public_key: Vec::new(),
        opaque_key_blob: Vec::new(),
        characteristics: Vec::new(),
        generation: 0,
    };
    read_fields(bytes, |field| {
        match field.number {
            1 => record.alias = field.string()?,
            2 => record.algorithm = key_algorithm_from_code(field.varint()?)?,
            3 => {
                record.flags = KeyFlags(
                    u16::try_from(field.varint()?)
                        .map_err(|_| KeychainStoreError::CorruptRecord)?,
                )
            }
            4 => record.public_key = field.bytes()?.to_vec(),
            5 => record.generation = field.varint()?,
            6 => record.opaque_key_blob = field.bytes()?.to_vec(),
            7 => record.characteristics = field.bytes()?.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    validate_alias(&record.alias)?;
    if !valid_hardware_public_key(record.algorithm, &record.public_key)
        || record.opaque_key_blob.is_empty()
        || record.opaque_key_blob.len() > 4096
        || record.characteristics.is_empty()
        || record.characteristics.len() > MAX_KEY_CHARACTERISTICS_LEN
    {
        return Err(KeychainStoreError::CorruptRecord);
    }
    Ok(record)
}

fn valid_hardware_public_key(algorithm: KeyAlgorithm, public_key: &[u8]) -> bool {
    match algorithm {
        // KeyMint exports the leaf X.509 certificate rather than raw points.
        KeyAlgorithm::Ed25519 | KeyAlgorithm::EcdsaP256 => {
            public_key.len() >= 32 && public_key.len() <= 16 * 1024
        }
        KeyAlgorithm::Aes256Gcm => public_key.is_empty(),
    }
}

fn key_algorithm_code(algorithm: KeyAlgorithm) -> u64 {
    match algorithm {
        KeyAlgorithm::Ed25519 => 1,
        KeyAlgorithm::EcdsaP256 => 2,
        KeyAlgorithm::Aes256Gcm => 3,
    }
}

fn key_algorithm_from_code(code: u64) -> Result<KeyAlgorithm, KeychainStoreError> {
    match code {
        1 => Ok(KeyAlgorithm::Ed25519),
        2 => Ok(KeyAlgorithm::EcdsaP256),
        3 => Ok(KeyAlgorithm::Aes256Gcm),
        _ => Err(KeychainStoreError::CorruptRecord),
    }
}

#[derive(Clone, Copy)]
struct Field<'a> {
    number: u32,
    wire_type: u8,
    value: &'a [u8],
}

impl<'a> Field<'a> {
    fn string(self) -> Result<String, KeychainStoreError> {
        if self.wire_type != 2 {
            return Err(KeychainStoreError::CorruptRecord);
        }
        core::str::from_utf8(self.value)
            .map(str::to_string)
            .map_err(|_| KeychainStoreError::CorruptRecord)
    }

    fn bytes(self) -> Result<&'a [u8], KeychainStoreError> {
        if self.wire_type != 2 {
            return Err(KeychainStoreError::CorruptRecord);
        }
        Ok(self.value)
    }

    fn varint(self) -> Result<u64, KeychainStoreError> {
        if self.wire_type != 0 {
            return Err(KeychainStoreError::CorruptRecord);
        }
        decode_raw_varint(self.value).map(|(value, _)| value)
    }
}

fn read_fields<F>(bytes: &[u8], mut f: F) -> Result<(), KeychainStoreError>
where
    F: FnMut(Field<'_>) -> Result<(), KeychainStoreError>,
{
    let mut offset = 0;
    while offset < bytes.len() {
        let (key, key_len) = decode_raw_varint(&bytes[offset..])?;
        offset += key_len;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x7) as u8;
        match wire_type {
            0 => {
                let start = offset;
                let (_, len) = decode_raw_varint(&bytes[offset..])?;
                offset += len;
                f(Field {
                    number,
                    wire_type,
                    value: &bytes[start..offset],
                })?;
            }
            2 => {
                let (len, len_len) = decode_raw_varint(&bytes[offset..])?;
                offset += len_len;
                let len = usize::try_from(len).map_err(|_| KeychainStoreError::CorruptRecord)?;
                let end = offset
                    .checked_add(len)
                    .ok_or(KeychainStoreError::CorruptRecord)?;
                if end > bytes.len() {
                    return Err(KeychainStoreError::CorruptRecord);
                }
                f(Field {
                    number,
                    wire_type,
                    value: &bytes[offset..end],
                })?;
                offset = end;
            }
            _ => return Err(KeychainStoreError::CorruptRecord),
        }
    }
    Ok(())
}

fn put_string(out: &mut Vec<u8>, field: u32, value: &str) {
    put_bytes(out, field, value.as_bytes());
}

fn put_bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_raw_varint(out, u64::from(field << 3 | 2));
    put_raw_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint(out: &mut Vec<u8>, field: u32, value: u64) {
    put_raw_varint(out, u64::from(field << 3));
    put_raw_varint(out, value);
}

fn put_raw_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn decode_raw_varint(bytes: &[u8]) -> Result<(u64, usize), KeychainStoreError> {
    let mut value = 0u64;
    for (index, shift) in (0..64).step_by(7).enumerate() {
        let byte = *bytes.get(index).ok_or(KeychainStoreError::CorruptRecord)?;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(KeychainStoreError::CorruptRecord)
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{
        KeyAlgorithm, KeyFlags, KeyRecord, KeychainStoreError, SecretRecord, decode_key,
        decode_secret, encode_key, encode_secret, validate_alias, validate_secret_record,
    };
    use alloc::vec::Vec;
    use bexos_redb::{BlockStore, open_or_create_with_store};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

    const SECRETS: TableDefinition<&str, &[u8]> = TableDefinition::new("secrets");
    const KEYS: TableDefinition<&str, &[u8]> = TableDefinition::new("keys");

    pub struct KeychainStoreDb {
        db: Database,
    }

    impl KeychainStoreDb {
        pub fn open<S: BlockStore>(store: S) -> Result<Self, KeychainStoreError> {
            Ok(Self {
                db: open_or_create_with_store(store).map_err(|_| KeychainStoreError::Storage)?,
            })
        }

        pub fn list_secrets(&self) -> Result<Vec<SecretRecord>, KeychainStoreError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| KeychainStoreError::Storage)?;
            let Ok(table) = tx.open_table(SECRETS) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| KeychainStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| KeychainStoreError::Storage)?;
                    decode_secret(value.value())
                })
                .collect()
        }

        pub fn put_secret(&self, record: &SecretRecord) -> Result<(), KeychainStoreError> {
            validate_secret_record(record)?;
            let bytes = encode_secret(record);
            let tx = self
                .db
                .begin_write()
                .map_err(|_| KeychainStoreError::Storage)?;
            {
                let mut table = tx
                    .open_table(SECRETS)
                    .map_err(|_| KeychainStoreError::Storage)?;
                table
                    .insert(record.alias.as_str(), bytes.as_slice())
                    .map_err(|_| KeychainStoreError::Storage)?;
            }
            tx.commit().map_err(|_| KeychainStoreError::Storage)
        }

        pub fn get_secret(&self, alias: &str) -> Result<SecretRecord, KeychainStoreError> {
            validate_alias(alias)?;
            let tx = self
                .db
                .begin_read()
                .map_err(|_| KeychainStoreError::Storage)?;
            let table = tx
                .open_table(SECRETS)
                .map_err(|_| KeychainStoreError::NotFound)?;
            table
                .get(alias)
                .map_err(|_| KeychainStoreError::Storage)?
                .map(|value| decode_secret(value.value()))
                .ok_or(KeychainStoreError::NotFound)?
        }

        pub fn delete_secret(&self, alias: &str) -> Result<(), KeychainStoreError> {
            validate_alias(alias)?;
            let tx = self
                .db
                .begin_write()
                .map_err(|_| KeychainStoreError::Storage)?;
            {
                let mut table = tx
                    .open_table(SECRETS)
                    .map_err(|_| KeychainStoreError::Storage)?;
                table
                    .remove(alias)
                    .map_err(|_| KeychainStoreError::Storage)?
                    .ok_or(KeychainStoreError::NotFound)?;
            }
            tx.commit().map_err(|_| KeychainStoreError::Storage)
        }

        pub fn put_hardware_key(
            &self,
            alias: &str,
            algorithm: KeyAlgorithm,
            flags: KeyFlags,
            public_key: &[u8],
            opaque_key_blob: &[u8],
            characteristics: &[u8],
        ) -> Result<(), KeychainStoreError> {
            validate_alias(alias)?;
            if !flags.contains(KeyFlags::HARDWARE_BACKED)
                || !super::valid_hardware_public_key(algorithm, public_key)
                || opaque_key_blob.is_empty()
                || opaque_key_blob.len() > 4096
                || characteristics.is_empty()
                || characteristics.len() > super::MAX_KEY_CHARACTERISTICS_LEN
            {
                return Err(KeychainStoreError::InvalidSecret);
            }
            let tx = self
                .db
                .begin_write()
                .map_err(|_| KeychainStoreError::Storage)?;
            {
                let mut table = tx
                    .open_table(KEYS)
                    .map_err(|_| KeychainStoreError::Storage)?;
                if table
                    .get(alias)
                    .map_err(|_| KeychainStoreError::Storage)?
                    .is_some()
                {
                    return Err(KeychainStoreError::AlreadyExists);
                }
                let record = KeyRecord {
                    alias: alias.into(),
                    algorithm,
                    flags,
                    public_key: public_key.to_vec(),
                    opaque_key_blob: opaque_key_blob.to_vec(),
                    characteristics: characteristics.to_vec(),
                    generation: 0,
                };
                let bytes = encode_key(&record);
                table
                    .insert(alias, bytes.as_slice())
                    .map_err(|_| KeychainStoreError::Storage)?;
            }
            tx.commit().map_err(|_| KeychainStoreError::Storage)
        }

        pub fn get_key(&self, alias: &str) -> Result<KeyRecord, KeychainStoreError> {
            validate_alias(alias)?;
            let tx = self
                .db
                .begin_read()
                .map_err(|_| KeychainStoreError::Storage)?;
            let table = tx
                .open_table(KEYS)
                .map_err(|_| KeychainStoreError::NotFound)?;
            table
                .get(alias)
                .map_err(|_| KeychainStoreError::Storage)?
                .map(|value| decode_key(value.value()))
                .ok_or(KeychainStoreError::NotFound)?
        }
    }
}
