#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const DEFAULT_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DomainPolicyRecord {
    pub origin: String,
    pub allowed_package_prefixes: Vec<String>,
    pub trusted_distribution_origins: Vec<String>,
    pub trusted_peer_domains: Vec<String>,
    pub signing_certificate_fingerprints: Vec<String>,
    pub fetched_timestamp: u64,
    pub ttl_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainAssociationError {
    EmptyDomain,
    InvalidDomain,
    InvalidOrigin,
    InvalidPackagePrefix,
    InvalidFingerprint,
    InvalidJson,
    InvalidUtf8,
    InvalidWireType,
    InvalidVarint,
    LengthOverflow,
    CorruptRecord,
    NotFound,
    Storage,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryDomainAssociationCache {
    records: Vec<(String, DomainPolicyRecord)>,
}

impl MemoryDomainAssociationCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(
        &mut self,
        domain: &str,
        record: DomainPolicyRecord,
    ) -> Result<(), DomainAssociationError> {
        let domain = canonical_domain(domain)?;
        validate_record_for_domain(&domain, &record)?;
        if let Some((_, existing)) = self.records.iter_mut().find(|(key, _)| key == &domain) {
            *existing = record;
        } else {
            self.records.push((domain, record));
            self.records.sort_by(|left, right| left.0.cmp(&right.0));
        }
        Ok(())
    }

    pub fn get(&self, domain: &str) -> Result<&DomainPolicyRecord, DomainAssociationError> {
        let domain = canonical_domain(domain)?;
        self.records
            .iter()
            .find(|(key, _)| key == &domain)
            .map(|(_, record)| record)
            .ok_or(DomainAssociationError::NotFound)
    }

    pub fn remove(&mut self, domain: &str) -> Result<(), DomainAssociationError> {
        let domain = canonical_domain(domain)?;
        self.records.retain(|(key, _)| key != &domain);
        Ok(())
    }

    pub fn records(&self) -> &[(String, DomainPolicyRecord)] {
        &self.records
    }

    pub fn find_package(&self, package_id: &str) -> Option<(&str, &DomainPolicyRecord)> {
        self.records
            .iter()
            .find(|(_, record)| record_allows_package(record, package_id))
            .map(|(domain, record)| (domain.as_str(), record))
    }
}

pub fn canonical_domain(input: &str) -> Result<String, DomainAssociationError> {
    let mut value = input.trim();
    if let Some(rest) = value.strip_prefix("https://") {
        value = rest;
    } else if value.contains("://") {
        return Err(DomainAssociationError::InvalidDomain);
    }
    value = value.split('/').next().unwrap_or(value);
    value = value.split('?').next().unwrap_or(value);
    value = value.split('#').next().unwrap_or(value);
    if let Some((host, port)) = value.rsplit_once(':') {
        if !host.contains(']') && port.bytes().all(|b| b.is_ascii_digit()) {
            value = host;
        }
    }
    let value = value.trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty() {
        return Err(DomainAssociationError::EmptyDomain);
    }
    if value.len() > 255
        || value.starts_with('.')
        || value.contains("..")
        || value
            .bytes()
            .any(|b| !b.is_ascii_lowercase() && !b.is_ascii_digit() && b != b'.' && b != b'-')
    {
        return Err(DomainAssociationError::InvalidDomain);
    }
    Ok(value)
}

pub fn reversed_domain_prefix(domain: &str) -> Result<String, DomainAssociationError> {
    let domain = canonical_domain(domain)?;
    Ok(domain.split('.').rev().collect::<Vec<_>>().join("."))
}

pub fn authoritative_domain_for_package(
    package_id: &str,
) -> Result<String, DomainAssociationError> {
    let (prefix, _) = package_id
        .split_once(':')
        .ok_or(DomainAssociationError::InvalidPackagePrefix)?;
    if prefix.is_empty() {
        return Err(DomainAssociationError::InvalidPackagePrefix);
    }
    canonical_domain(&prefix.split('.').rev().collect::<Vec<_>>().join("."))
}

pub fn record_allows_package(record: &DomainPolicyRecord, package_id: &str) -> bool {
    let Some((package_prefix, suffix)) = package_id.split_once(':') else {
        return false;
    };
    if suffix.is_empty() {
        return false;
    }
    record.allowed_package_prefixes.iter().any(|allowed| {
        package_prefix == allowed
            || package_prefix
                .strip_prefix(allowed.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

pub fn validate_record_for_domain(
    domain: &str,
    record: &DomainPolicyRecord,
) -> Result<(), DomainAssociationError> {
    let domain = canonical_domain(domain)?;
    let record_domain = origin_domain(&record.origin)?;
    if record_domain != domain {
        return Err(DomainAssociationError::InvalidOrigin);
    }
    if record.allowed_package_prefixes.is_empty() {
        return Err(DomainAssociationError::InvalidPackagePrefix);
    }
    for prefix in &record.allowed_package_prefixes {
        validate_package_prefix(prefix)?;
    }
    for origin in &record.trusted_distribution_origins {
        let _ = origin_domain(origin)?;
    }
    for origin in &record.trusted_peer_domains {
        let _ = origin_domain(origin)?;
    }
    if record.signing_certificate_fingerprints.is_empty() {
        return Err(DomainAssociationError::InvalidFingerprint);
    }
    for fingerprint in &record.signing_certificate_fingerprints {
        validate_fingerprint(fingerprint)?;
    }
    Ok(())
}

pub fn origin_domain(origin: &str) -> Result<String, DomainAssociationError> {
    let rest = origin
        .trim()
        .strip_prefix("https://")
        .ok_or(DomainAssociationError::InvalidOrigin)?;
    canonical_domain(rest)
}

pub fn origin_from_domain(domain: &str) -> Result<String, DomainAssociationError> {
    let domain = canonical_domain(domain)?;
    let mut origin = String::from("https://");
    origin.push_str(&domain);
    Ok(origin)
}

pub fn validate_distribution_origin(
    policy_domain: &str,
    download_origin: &str,
    record: &DomainPolicyRecord,
) -> Result<(), DomainAssociationError> {
    let policy_domain = canonical_domain(policy_domain)?;
    let download_domain = origin_domain(download_origin)?;
    if download_domain == policy_domain {
        return Ok(());
    }
    if record
        .trusted_distribution_origins
        .iter()
        .any(|origin| origin_domain(origin).ok().as_deref() == Some(download_domain.as_str()))
    {
        Ok(())
    } else {
        Err(DomainAssociationError::InvalidOrigin)
    }
}

pub fn parse_well_known_json(
    bytes: &[u8],
    fetched_timestamp: u64,
) -> Result<DomainPolicyRecord, DomainAssociationError> {
    let text = core::str::from_utf8(bytes).map_err(|_| DomainAssociationError::InvalidUtf8)?;
    let mut record = DomainPolicyRecord {
        origin: json_string(text, "origin")?,
        allowed_package_prefixes: json_string_array(text, "allowed_package_prefixes")?,
        trusted_distribution_origins: json_string_array(text, "trusted_distribution_origins")?,
        trusted_peer_domains: json_string_array(text, "trusted_peer_domains")?,
        signing_certificate_fingerprints: json_string_array(
            text,
            "signing_certificate_fingerprints",
        )?,
        fetched_timestamp,
        ttl_seconds: json_u64(text, "ttl_seconds").unwrap_or(DEFAULT_TTL_SECONDS),
    };
    if record.ttl_seconds == 0 {
        record.ttl_seconds = DEFAULT_TTL_SECONDS;
    }
    Ok(record)
}

pub fn record_expired(record: &DomainPolicyRecord, now: u64) -> bool {
    now >= record.fetched_timestamp.saturating_add(record.ttl_seconds)
}

pub fn encode_record(record: &DomainPolicyRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &record.origin);
    for value in &record.allowed_package_prefixes {
        put_string(&mut out, 2, value);
    }
    for value in &record.trusted_distribution_origins {
        put_string(&mut out, 3, value);
    }
    for value in &record.trusted_peer_domains {
        put_string(&mut out, 4, value);
    }
    for value in &record.signing_certificate_fingerprints {
        put_string(&mut out, 5, value);
    }
    put_varint(&mut out, 6, record.fetched_timestamp);
    put_varint(&mut out, 7, record.ttl_seconds);
    out
}

pub fn decode_record(bytes: &[u8]) -> Result<DomainPolicyRecord, DomainAssociationError> {
    let mut record = DomainPolicyRecord::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => record.origin = field.string()?,
            2 => record.allowed_package_prefixes.push(field.string()?),
            3 => record.trusted_distribution_origins.push(field.string()?),
            4 => record.trusted_peer_domains.push(field.string()?),
            5 => record
                .signing_certificate_fingerprints
                .push(field.string()?),
            6 => record.fetched_timestamp = field.varint()?,
            7 => record.ttl_seconds = field.varint()?,
            _ => {}
        }
    }
    if record.origin.is_empty() {
        return Err(DomainAssociationError::CorruptRecord);
    }
    Ok(record)
}

fn validate_package_prefix(prefix: &str) -> Result<(), DomainAssociationError> {
    if prefix.is_empty()
        || prefix.len() > 128
        || prefix.starts_with('.')
        || prefix.contains("..")
        || prefix.bytes().any(|b| {
            !b.is_ascii_lowercase() && !b.is_ascii_digit() && b != b'.' && b != b'-' && b != b'_'
        })
    {
        Err(DomainAssociationError::InvalidPackagePrefix)
    } else {
        Ok(())
    }
}

fn validate_fingerprint(value: &str) -> Result<(), DomainAssociationError> {
    let Some(hex) = value.strip_prefix("SHA256:") else {
        return Err(DomainAssociationError::InvalidFingerprint);
    };
    let digits = hex.bytes().filter(|b| *b != b':').collect::<Vec<_>>();
    if digits.len() != 64 || digits.iter().any(|b| !b.is_ascii_hexdigit()) {
        return Err(DomainAssociationError::InvalidFingerprint);
    }
    Ok(())
}

fn json_string(text: &str, key: &str) -> Result<String, DomainAssociationError> {
    let value = json_value_after_key(text, key)?;
    if !value.starts_with('"') {
        return Err(DomainAssociationError::InvalidJson);
    }
    parse_json_string(value).map(|(value, _)| value)
}

fn json_string_array(text: &str, key: &str) -> Result<Vec<String>, DomainAssociationError> {
    let value = json_value_after_key(text, key)?;
    let mut rest = value
        .strip_prefix('[')
        .ok_or(DomainAssociationError::InvalidJson)?;
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(']') {
            let _ = after;
            return Ok(out);
        }
        let (item, after_string) = parse_json_string(rest)?;
        out.push(item);
        rest = after_string.trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after;
        } else if rest.starts_with(']') {
        } else {
            return Err(DomainAssociationError::InvalidJson);
        }
    }
}

fn json_u64(text: &str, key: &str) -> Result<u64, DomainAssociationError> {
    let value = json_value_after_key(text, key)?;
    let digits = value
        .bytes()
        .take_while(|b| b.is_ascii_digit())
        .collect::<Vec<_>>();
    if digits.is_empty() {
        return Err(DomainAssociationError::InvalidJson);
    }
    core::str::from_utf8(&digits)
        .map_err(|_| DomainAssociationError::InvalidJson)?
        .parse()
        .map_err(|_| DomainAssociationError::InvalidJson)
}

fn json_value_after_key<'a>(text: &'a str, key: &str) -> Result<&'a str, DomainAssociationError> {
    let needle = {
        let mut s = String::from("\"");
        s.push_str(key);
        s.push('"');
        s
    };
    let after_key = text
        .find(&needle)
        .and_then(|index| text.get(index + needle.len()..))
        .ok_or(DomainAssociationError::InvalidJson)?;
    let after_colon = after_key
        .trim_start()
        .strip_prefix(':')
        .ok_or(DomainAssociationError::InvalidJson)?;
    Ok(after_colon.trim_start())
}

fn parse_json_string(input: &str) -> Result<(String, &str), DomainAssociationError> {
    let mut chars = input.chars();
    if chars.next() != Some('"') {
        return Err(DomainAssociationError::InvalidJson);
    }
    let mut out = String::new();
    let mut escaped = false;
    for (index, ch) in input[1..].char_indices() {
        if escaped {
            match ch {
                '"' | '\\' | '/' => out.push(ch),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                _ => return Err(DomainAssociationError::InvalidJson),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            let end = 1 + index + ch.len_utf8();
            return Ok((out, &input[end..]));
        } else {
            out.push(ch);
        }
    }
    Err(DomainAssociationError::InvalidJson)
}

#[derive(Clone, Copy)]
struct Field<'a> {
    number: u32,
    wire_type: u8,
    value: &'a [u8],
}

impl<'a> Field<'a> {
    fn string(self) -> Result<String, DomainAssociationError> {
        if self.wire_type != 2 {
            return Err(DomainAssociationError::InvalidWireType);
        }
        core::str::from_utf8(self.value)
            .map(str::to_string)
            .map_err(|_| DomainAssociationError::InvalidUtf8)
    }

    fn varint(self) -> Result<u64, DomainAssociationError> {
        if self.wire_type != 0 {
            return Err(DomainAssociationError::InvalidWireType);
        }
        decode_varint(self.value)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn next_field(&mut self) -> Result<Option<Field<'a>>, DomainAssociationError> {
        if self.offset == self.bytes.len() {
            return Ok(None);
        }
        let key_start = self.offset;
        let key = self.next_varint()?;
        let wire_type = (key & 0x7) as u8;
        let number = (key >> 3) as u32;
        let value_start = self.offset;
        match wire_type {
            0 => {
                self.next_varint()?;
                Ok(Some(Field {
                    number,
                    wire_type,
                    value: &self.bytes[value_start..self.offset],
                }))
            }
            2 => {
                let len = self.next_varint()? as usize;
                let end = self
                    .offset
                    .checked_add(len)
                    .ok_or(DomainAssociationError::LengthOverflow)?;
                let value = self
                    .bytes
                    .get(self.offset..end)
                    .ok_or(DomainAssociationError::CorruptRecord)?;
                self.offset = end;
                Ok(Some(Field {
                    number,
                    wire_type,
                    value,
                }))
            }
            _ => {
                let _ = key_start;
                Err(DomainAssociationError::InvalidWireType)
            }
        }
    }

    fn next_varint(&mut self) -> Result<u64, DomainAssociationError> {
        let mut value = 0u64;
        let mut shift = 0;
        let start = self.offset;
        loop {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(DomainAssociationError::CorruptRecord)?;
            self.offset += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 64 || self.offset - start > 10 {
                return Err(DomainAssociationError::InvalidVarint);
            }
        }
    }
}

fn put_string(out: &mut Vec<u8>, number: u32, value: &str) {
    put_bytes(out, number, value.as_bytes());
}

fn put_bytes(out: &mut Vec<u8>, number: u32, value: &[u8]) {
    put_varint_raw(out, u64::from(number << 3 | 2));
    put_varint_raw(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint(out: &mut Vec<u8>, number: u32, value: u64) {
    put_varint_raw(out, u64::from(number << 3));
    put_varint_raw(out, value);
}

fn put_varint_raw(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn decode_varint(bytes: &[u8]) -> Result<u64, DomainAssociationError> {
    let mut value = 0u64;
    for (index, byte) in bytes.iter().enumerate() {
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(DomainAssociationError::InvalidVarint)
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{
        DomainAssociationError, DomainPolicyRecord, MemoryDomainAssociationCache, canonical_domain,
        decode_record, encode_record, validate_record_for_domain,
    };
    use alloc::vec::Vec;
    use bexos_redb::{BlockStore, open_or_create_with_store};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

    const DOMAIN_POLICIES: TableDefinition<&str, &[u8]> = TableDefinition::new("domain_policies");

    pub struct DomainAssociationDb {
        db: Database,
    }

    impl DomainAssociationDb {
        pub fn open<S: BlockStore>(store: S) -> Result<Self, DomainAssociationError> {
            Ok(Self {
                db: open_or_create_with_store(store)
                    .map_err(|_| DomainAssociationError::Storage)?,
            })
        }

        pub fn put(
            &self,
            domain: &str,
            record: &DomainPolicyRecord,
        ) -> Result<(), DomainAssociationError> {
            let domain = canonical_domain(domain)?;
            validate_record_for_domain(&domain, record)?;
            let bytes = encode_record(record);
            let tx = self
                .db
                .begin_write()
                .map_err(|_| DomainAssociationError::Storage)?;
            {
                let mut table = tx
                    .open_table(DOMAIN_POLICIES)
                    .map_err(|_| DomainAssociationError::Storage)?;
                table
                    .insert(domain.as_str(), bytes.as_slice())
                    .map_err(|_| DomainAssociationError::Storage)?;
            }
            tx.commit().map_err(|_| DomainAssociationError::Storage)
        }

        pub fn get(&self, domain: &str) -> Result<DomainPolicyRecord, DomainAssociationError> {
            let domain = canonical_domain(domain)?;
            let tx = self
                .db
                .begin_read()
                .map_err(|_| DomainAssociationError::Storage)?;
            let table = tx
                .open_table(DOMAIN_POLICIES)
                .map_err(|_| DomainAssociationError::NotFound)?;
            let value = table
                .get(domain.as_str())
                .map_err(|_| DomainAssociationError::Storage)?
                .ok_or(DomainAssociationError::NotFound)?;
            decode_record(value.value())
        }

        pub fn snapshot_memory(
            &self,
        ) -> Result<MemoryDomainAssociationCache, DomainAssociationError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| DomainAssociationError::Storage)?;
            let Ok(table) = tx.open_table(DOMAIN_POLICIES) else {
                return Ok(MemoryDomainAssociationCache::new());
            };
            let mut cache = MemoryDomainAssociationCache::new();
            for entry in table.iter().map_err(|_| DomainAssociationError::Storage)? {
                let (key, value) = entry.map_err(|_| DomainAssociationError::Storage)?;
                cache.upsert(key.value(), decode_record(value.value())?)?;
            }
            Ok(cache)
        }

        pub fn replace_from_memory(
            &self,
            cache: &MemoryDomainAssociationCache,
        ) -> Result<(), DomainAssociationError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| DomainAssociationError::Storage)?;
            {
                let mut table = tx
                    .open_table(DOMAIN_POLICIES)
                    .map_err(|_| DomainAssociationError::Storage)?;
                table
                    .retain(|_, _| false)
                    .map_err(|_| DomainAssociationError::Storage)?;
                for (domain, record) in cache.records() {
                    let bytes = encode_record(record);
                    table
                        .insert(domain.as_str(), bytes.as_slice())
                        .map_err(|_| DomainAssociationError::Storage)?;
                }
            }
            tx.commit().map_err(|_| DomainAssociationError::Storage)
        }

        pub fn list(&self) -> Result<Vec<(String, DomainPolicyRecord)>, DomainAssociationError> {
            Ok(self.snapshot_memory()?.records().to_vec())
        }
    }
}
