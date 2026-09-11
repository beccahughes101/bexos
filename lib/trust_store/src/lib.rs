#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const TLS_ROOT_KIND: &str = "tls";
pub const APP_ROOT_KIND: &str = "app";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustTier {
    Tier0BaseSystem = 0,
    Tier1PlatformApp = 1,
    Tier2VerifiedEco = 2,
    Tier3Enterprise = 3,
    Tier4WebOrigin = 4,
}

impl TrustTier {
    pub fn from_u64(value: u64) -> Result<Self, TrustStoreError> {
        match value {
            0 => Ok(Self::Tier0BaseSystem),
            1 => Ok(Self::Tier1PlatformApp),
            2 => Ok(Self::Tier2VerifiedEco),
            3 => Ok(Self::Tier3Enterprise),
            4 => Ok(Self::Tier4WebOrigin),
            _ => Err(TrustStoreError::InvalidTier),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsRootAnchor {
    pub root_id: [u8; 32],
    pub source_path: String,
    pub der_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppSigningRootAnchor {
    pub anchor_id: String,
    pub tier: TrustTier,
    pub algorithm: String,
    pub public_key_bytes: Vec<u8>,
    pub certificate_der: Vec<u8>,
    pub permitted_package_prefixes: Vec<String>,
    pub valid_from: u64,
    pub valid_until: u64,
    pub is_hardware_anchored: bool,
    pub immutable: bool,
    pub enterprise: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RevocationPayload {
    pub generation: u64,
    pub valid_from: u64,
    pub valid_until: u64,
    pub revoked_cert_fingerprints: Vec<[u8; 32]>,
    pub revoked_spki_fingerprints: Vec<[u8; 32]>,
    pub revoked_serials: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrustStoreError {
    EmptyAnchorId,
    EmptyBytes,
    InvalidTier,
    InvalidAlgorithm,
    InvalidPrefix,
    InvalidValidity,
    InvalidUtf8,
    InvalidWireType(u8),
    InvalidVarint,
    LengthOverflow,
    CorruptRecord,
    Storage,
    DuplicateAnchor,
    NotFound,
}

pub fn root_id(bytes: &[u8]) -> Result<[u8; 32], TrustStoreError> {
    if bytes.is_empty() {
        return Err(TrustStoreError::EmptyBytes);
    }
    Ok(blake3::hash(bytes).into())
}

pub fn validate_app_anchor(anchor: &AppSigningRootAnchor) -> Result<(), TrustStoreError> {
    if anchor.anchor_id.is_empty() {
        return Err(TrustStoreError::EmptyAnchorId);
    }
    if anchor.algorithm != "Ed25519" && anchor.algorithm != "ECDSA_P256_SHA256" {
        return Err(TrustStoreError::InvalidAlgorithm);
    }
    if anchor.certificate_der.is_empty() && anchor.public_key_bytes.is_empty() {
        return Err(TrustStoreError::EmptyBytes);
    }
    if anchor.algorithm == "Ed25519"
        && !anchor.public_key_bytes.is_empty()
        && anchor.public_key_bytes.len() != 32
    {
        return Err(TrustStoreError::EmptyBytes);
    }
    if anchor.permitted_package_prefixes.is_empty()
        || anchor
            .permitted_package_prefixes
            .iter()
            .any(|prefix| prefix.is_empty())
    {
        return Err(TrustStoreError::InvalidPrefix);
    }
    if anchor.valid_until != 0 && anchor.valid_until < anchor.valid_from {
        return Err(TrustStoreError::InvalidValidity);
    }
    Ok(())
}

pub fn package_permitted(anchor: &AppSigningRootAnchor, package_id: &str) -> bool {
    anchor.permitted_package_prefixes.iter().any(|prefix| {
        if prefix == "*" {
            return true;
        }
        if let Some(base) = prefix.strip_suffix('*') {
            return package_id.starts_with(base);
        }
        package_id == prefix
    })
}

pub fn validate_direct_app_signature(
    anchors: &[AppSigningRootAnchor],
    package_id: &str,
    now: u64,
    public_key: &[u8],
) -> Result<AppSigningRootAnchor, TrustStoreError> {
    for anchor in anchors {
        if anchor.public_key_bytes != public_key {
            continue;
        }
        if !package_permitted(anchor, package_id) {
            return Err(TrustStoreError::InvalidPrefix);
        }
        if now < anchor.valid_from || (anchor.valid_until != 0 && now > anchor.valid_until) {
            return Err(TrustStoreError::InvalidValidity);
        }
        return Ok(anchor.clone());
    }
    Err(TrustStoreError::NotFound)
}

pub fn encode_tls_anchor(anchor: &TlsRootAnchor) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &anchor.root_id);
    put_string(&mut out, 2, &anchor.source_path);
    put_bytes(&mut out, 3, &anchor.der_bytes);
    out
}

pub fn decode_tls_anchor(bytes: &[u8]) -> Result<TlsRootAnchor, TrustStoreError> {
    let mut anchor = TlsRootAnchor {
        root_id: [0; 32],
        source_path: String::new(),
        der_bytes: Vec::new(),
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => {
                let raw = field.bytes()?;
                if raw.len() != 32 {
                    return Err(TrustStoreError::CorruptRecord);
                }
                anchor.root_id.copy_from_slice(raw);
            }
            2 => anchor.source_path = field.string()?,
            3 => anchor.der_bytes = field.bytes()?.to_vec(),
            _ => {}
        }
    }
    if anchor.der_bytes.is_empty() {
        return Err(TrustStoreError::EmptyBytes);
    }
    Ok(anchor)
}

pub fn encode_app_anchor(anchor: &AppSigningRootAnchor) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &anchor.anchor_id);
    put_varint(&mut out, 2, anchor.tier as u64);
    put_string(&mut out, 3, &anchor.algorithm);
    put_bytes(&mut out, 4, &anchor.public_key_bytes);
    for prefix in &anchor.permitted_package_prefixes {
        put_string(&mut out, 5, prefix);
    }
    put_varint(&mut out, 6, anchor.valid_from);
    put_varint(&mut out, 7, anchor.valid_until);
    put_varint(&mut out, 8, u64::from(anchor.is_hardware_anchored));
    put_bytes(&mut out, 9, &anchor.certificate_der);
    put_varint(&mut out, 10, u64::from(anchor.immutable));
    put_varint(&mut out, 11, u64::from(anchor.enterprise));
    out
}

pub fn decode_app_anchor(bytes: &[u8]) -> Result<AppSigningRootAnchor, TrustStoreError> {
    let mut anchor = AppSigningRootAnchor {
        anchor_id: String::new(),
        tier: TrustTier::Tier4WebOrigin,
        algorithm: String::new(),
        public_key_bytes: Vec::new(),
        certificate_der: Vec::new(),
        permitted_package_prefixes: Vec::new(),
        valid_from: 0,
        valid_until: 0,
        is_hardware_anchored: false,
        immutable: false,
        enterprise: false,
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => anchor.anchor_id = field.string()?,
            2 => anchor.tier = TrustTier::from_u64(field.varint()?)?,
            3 => anchor.algorithm = field.string()?,
            4 => anchor.public_key_bytes = field.bytes()?.to_vec(),
            5 => anchor.permitted_package_prefixes.push(field.string()?),
            6 => anchor.valid_from = field.varint()?,
            7 => anchor.valid_until = field.varint()?,
            8 => anchor.is_hardware_anchored = field.varint()? != 0,
            9 => anchor.certificate_der = field.bytes()?.to_vec(),
            10 => anchor.immutable = field.varint()? != 0,
            11 => anchor.enterprise = field.varint()? != 0,
            _ => {}
        }
    }
    validate_app_anchor(&anchor)?;
    Ok(anchor)
}

pub fn encode_revocation_payload(payload: &RevocationPayload) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, payload.generation);
    put_varint(&mut out, 2, payload.valid_from);
    put_varint(&mut out, 3, payload.valid_until);
    for fingerprint in &payload.revoked_cert_fingerprints {
        put_bytes(&mut out, 4, fingerprint);
    }
    for fingerprint in &payload.revoked_spki_fingerprints {
        put_bytes(&mut out, 5, fingerprint);
    }
    for serial in &payload.revoked_serials {
        put_bytes(&mut out, 6, serial);
    }
    out
}

pub fn decode_revocation_payload(bytes: &[u8]) -> Result<RevocationPayload, TrustStoreError> {
    let mut payload = RevocationPayload::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => payload.generation = field.varint()?,
            2 => payload.valid_from = field.varint()?,
            3 => payload.valid_until = field.varint()?,
            4 => payload
                .revoked_cert_fingerprints
                .push(fingerprint(field.bytes()?)?),
            5 => payload
                .revoked_spki_fingerprints
                .push(fingerprint(field.bytes()?)?),
            6 => {
                let serial = field.bytes()?;
                if serial.is_empty() || serial.len() > 32 {
                    return Err(TrustStoreError::CorruptRecord);
                }
                payload.revoked_serials.push(serial.to_vec());
            }
            _ => {}
        }
    }
    if payload.generation == 0 || payload.valid_until < payload.valid_from {
        return Err(TrustStoreError::InvalidValidity);
    }
    Ok(payload)
}

fn fingerprint(bytes: &[u8]) -> Result<[u8; 32], TrustStoreError> {
    if bytes.len() != 32 {
        return Err(TrustStoreError::CorruptRecord);
    }
    let mut out = [0; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}

struct Field<'a> {
    number: u32,
    wire_type: u8,
    bytes: &'a [u8],
}

impl<'a> Field<'a> {
    fn string(&self) -> Result<String, TrustStoreError> {
        Ok(core::str::from_utf8(self.bytes()?)
            .map_err(|_| TrustStoreError::InvalidUtf8)?
            .to_string())
    }

    fn bytes(&self) -> Result<&'a [u8], TrustStoreError> {
        if self.wire_type != 2 {
            return Err(TrustStoreError::InvalidWireType(self.wire_type));
        }
        Ok(self.bytes)
    }

    fn varint(&self) -> Result<u64, TrustStoreError> {
        if self.wire_type != 0 {
            return Err(TrustStoreError::InvalidWireType(self.wire_type));
        }
        let mut value = 0u64;
        for (shift, byte) in self.bytes.iter().enumerate() {
            value |= u64::from(byte & 0x7f) << (shift * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(TrustStoreError::InvalidVarint)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn next_field(&mut self) -> Result<Option<Field<'a>>, TrustStoreError> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }
        let key = self.read_varint()?;
        let wire_type = (key & 0x7) as u8;
        let number = (key >> 3) as u32;
        let (value_start, value_end) = match wire_type {
            0 => {
                let value_start = self.pos;
                self.read_varint()?;
                (value_start, self.pos)
            }
            2 => {
                let len = self.read_varint()? as usize;
                let value_start = self.pos;
                self.pos = self
                    .pos
                    .checked_add(len)
                    .ok_or(TrustStoreError::LengthOverflow)?;
                if self.pos > self.bytes.len() {
                    return Err(TrustStoreError::LengthOverflow);
                }
                (value_start, self.pos)
            }
            other => return Err(TrustStoreError::InvalidWireType(other)),
        };
        Ok(Some(Field {
            number,
            wire_type,
            bytes: &self.bytes[value_start..value_end],
        }))
    }

    fn read_varint(&mut self) -> Result<u64, TrustStoreError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or(TrustStoreError::InvalidVarint)?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(TrustStoreError::InvalidVarint)
    }
}

fn put_string(out: &mut Vec<u8>, number: u32, value: &str) {
    put_bytes(out, number, value.as_bytes());
}

fn put_bytes(out: &mut Vec<u8>, number: u32, value: &[u8]) {
    put_key(out, number, 2);
    put_raw_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint(out: &mut Vec<u8>, number: u32, value: u64) {
    put_key(out, number, 0);
    put_raw_varint(out, value);
}

fn put_key(out: &mut Vec<u8>, number: u32, wire_type: u8) {
    put_raw_varint(out, (u64::from(number) << 3) | u64::from(wire_type));
}

fn put_raw_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{
        AppSigningRootAnchor, TLS_ROOT_KIND, TlsRootAnchor, TrustStoreError, decode_app_anchor,
        decode_tls_anchor, encode_app_anchor, encode_tls_anchor,
    };
    use alloc::vec::Vec;
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
    use std::path::Path;

    const META: TableDefinition<&str, &str> = TableDefinition::new("meta");
    const TLS_ROOTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("tls_roots");
    const APP_ROOTS: TableDefinition<&str, &[u8]> = TableDefinition::new("app_roots");

    pub struct TrustStoreDb {
        db: Database,
    }

    impl TrustStoreDb {
        pub fn create_tls(path: &Path, roots: &[TlsRootAnchor]) -> Result<Self, TrustStoreError> {
            if roots.is_empty() {
                return Err(TrustStoreError::EmptyBytes);
            }
            let db = Database::create(path).map_err(|_| TrustStoreError::Storage)?;
            let this = Self { db };
            let tx = this
                .db
                .begin_write()
                .map_err(|_| TrustStoreError::Storage)?;
            {
                let mut meta = tx.open_table(META).map_err(|_| TrustStoreError::Storage)?;
                meta.insert("kind", TLS_ROOT_KIND)
                    .map_err(|_| TrustStoreError::Storage)?;
                let mut table = tx
                    .open_table(TLS_ROOTS)
                    .map_err(|_| TrustStoreError::Storage)?;
                for root in roots {
                    let bytes = encode_tls_anchor(root);
                    table
                        .insert(root.root_id.as_slice(), bytes.as_slice())
                        .map_err(|_| TrustStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| TrustStoreError::Storage)?;
            Ok(this)
        }

        pub fn create_app(
            path: &Path,
            roots: &[AppSigningRootAnchor],
        ) -> Result<Self, TrustStoreError> {
            if roots.is_empty() {
                return Err(TrustStoreError::EmptyBytes);
            }
            let db = Database::create(path).map_err(|_| TrustStoreError::Storage)?;
            let this = Self { db };
            let tx = this
                .db
                .begin_write()
                .map_err(|_| TrustStoreError::Storage)?;
            {
                let mut meta = tx.open_table(META).map_err(|_| TrustStoreError::Storage)?;
                meta.insert("kind", super::APP_ROOT_KIND)
                    .map_err(|_| TrustStoreError::Storage)?;
                let mut table = tx
                    .open_table(APP_ROOTS)
                    .map_err(|_| TrustStoreError::Storage)?;
                for root in roots {
                    let bytes = encode_app_anchor(root);
                    table
                        .insert(root.anchor_id.as_str(), bytes.as_slice())
                        .map_err(|_| TrustStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| TrustStoreError::Storage)?;
            Ok(this)
        }

        pub fn open(path: &Path) -> Result<Self, TrustStoreError> {
            Ok(Self {
                db: Database::open(path).map_err(|_| TrustStoreError::Storage)?,
            })
        }

        pub fn open_with_backend(
            backend: impl redb::StorageBackend,
        ) -> Result<Self, TrustStoreError> {
            Ok(Self {
                db: redb::Builder::new()
                    .create_with_backend(backend)
                    .map_err(|_| TrustStoreError::Storage)?,
            })
        }

        pub fn list_tls_roots(&self) -> Result<Vec<TlsRootAnchor>, TrustStoreError> {
            let tx = self.db.begin_read().map_err(|_| TrustStoreError::Storage)?;
            let table = tx
                .open_table(TLS_ROOTS)
                .map_err(|_| TrustStoreError::NotFound)?;
            table
                .iter()
                .map_err(|_| TrustStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| TrustStoreError::Storage)?;
                    decode_tls_anchor(value.value())
                })
                .collect()
        }

        pub fn list_app_roots(&self) -> Result<Vec<AppSigningRootAnchor>, TrustStoreError> {
            let tx = self.db.begin_read().map_err(|_| TrustStoreError::Storage)?;
            let table = tx
                .open_table(APP_ROOTS)
                .map_err(|_| TrustStoreError::NotFound)?;
            table
                .iter()
                .map_err(|_| TrustStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| TrustStoreError::Storage)?;
                    decode_app_anchor(value.value())
                })
                .collect()
        }
    }
}
