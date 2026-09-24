use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::routing::{IpAddress, ProviderToken, TableId, normalize_domain};

pub const MAX_DNS_MESSAGE: usize = 4096;
pub const MAX_DNS_RESULTS: usize = 8;
const MAX_CNAME_DEPTH: usize = 8;
const MAX_NEGATIVE_TTL_SECONDS: u32 = 300;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum RecordType {
    A = 1,
    Aaaa = 28,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CacheKey {
    pub table: TableId,
    pub provider: ProviderToken,
    pub hostname: String,
    pub record_type: RecordType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CacheValue {
    Positive(Vec<IpAddress>),
    Negative(ResponseCode),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheEntry {
    pub value: CacheValue,
    pub expires_at_ms: u64,
    pub provider_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingQuery {
    pub key: CacheKey,
    pub dns_id: u16,
    pub deadline_ms: u64,
    pub provider_generation: u64,
    pub upstream_index: usize,
    pub encoded_request: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseCode {
    NoError,
    NameError,
    Refused,
    ServerFailure,
    Other(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DnsError {
    InvalidName,
    InvalidMessage,
    MismatchedResponse,
    Truncated,
    UnsupportedRecord,
    CnameLoop,
    Capacity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedResponse {
    pub addresses: Vec<IpAddress>,
    pub cname: Option<String>,
    pub ttl_seconds: u32,
    pub response_code: ResponseCode,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolverState {
    pub(crate) cache: BTreeMap<CacheKey, CacheEntry>,
    pub(crate) pending: BTreeMap<CacheKey, PendingQuery>,
    pub(crate) capacity: usize,
}

impl ResolverState {
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: BTreeMap::new(),
            pending: BTreeMap::new(),
            capacity,
        }
    }

    pub fn cache(&self) -> &BTreeMap<CacheKey, CacheEntry> {
        &self.cache
    }

    pub fn pending(&self) -> &BTreeMap<CacheKey, PendingQuery> {
        &self.pending
    }

    pub fn lookup(&mut self, key: &CacheKey, now_ms: u64, generation: u64) -> Option<&CacheValue> {
        let expired = self.cache.get(key).is_some_and(|entry| {
            entry.expires_at_ms <= now_ms || entry.provider_generation != generation
        });
        if expired {
            self.cache.remove(key);
        }
        self.cache.get(key).map(|entry| &entry.value)
    }

    pub fn begin(
        &mut self,
        table: TableId,
        provider: ProviderToken,
        provider_generation: u64,
        hostname: &str,
        record_type: RecordType,
        dns_id: u16,
        deadline_ms: u64,
    ) -> Result<&PendingQuery, DnsError> {
        let hostname = normalize_domain(hostname).map_err(|_| DnsError::InvalidName)?;
        let key = CacheKey {
            table,
            provider,
            hostname,
            record_type,
        };
        if !self.pending.contains_key(&key) && self.pending.len() >= self.capacity {
            return Err(DnsError::Capacity);
        }
        let encoded_request = encode_query(dns_id, &key.hostname, record_type)?;
        self.pending.insert(
            key.clone(),
            PendingQuery {
                key: key.clone(),
                dns_id,
                deadline_ms,
                provider_generation,
                upstream_index: 0,
                encoded_request,
            },
        );
        Ok(self.pending.get(&key).expect("inserted query"))
    }

    pub fn retry(&mut self, key: &CacheKey, upstream_count: usize) -> bool {
        self.pending.get_mut(key).is_some_and(|pending| {
            if pending.upstream_index + 1 >= upstream_count {
                false
            } else {
                pending.upstream_index += 1;
                true
            }
        })
    }

    pub fn insert_value(
        &mut self,
        key: CacheKey,
        value: CacheValue,
        ttl_seconds: u32,
        now_ms: u64,
        provider_generation: u64,
    ) {
        if self.cache.len() >= self.capacity {
            if let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at_ms)
                .map(|(key, _)| key.clone())
            {
                self.cache.remove(&oldest);
            }
        }
        let ttl_seconds = if matches!(&value, CacheValue::Negative(_)) {
            ttl_seconds.min(MAX_NEGATIVE_TTL_SECONDS)
        } else {
            ttl_seconds
        };
        self.cache.insert(
            key,
            CacheEntry {
                value,
                expires_at_ms: now_ms.saturating_add(u64::from(ttl_seconds) * 1000),
                provider_generation,
            },
        );
    }

    pub fn finish(
        &mut self,
        key: &CacheKey,
        response: &[u8],
        now_ms: u64,
    ) -> Result<CacheValue, DnsError> {
        let pending = self
            .pending
            .remove(key)
            .ok_or(DnsError::MismatchedResponse)?;
        let parsed = parse_response(
            response,
            pending.dns_id,
            &pending.key.hostname,
            pending.key.record_type,
        )?;
        let ttl = if parsed.response_code == ResponseCode::NoError && !parsed.addresses.is_empty() {
            parsed.ttl_seconds
        } else {
            parsed.ttl_seconds.min(MAX_NEGATIVE_TTL_SECONDS)
        };
        let value = if parsed.response_code == ResponseCode::NoError && !parsed.addresses.is_empty()
        {
            CacheValue::Positive(parsed.addresses)
        } else {
            CacheValue::Negative(parsed.response_code)
        };
        if self.cache.len() >= self.capacity {
            if let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at_ms)
                .map(|(key, _)| key.clone())
            {
                self.cache.remove(&oldest);
            }
        }
        self.cache.insert(
            key.clone(),
            CacheEntry {
                value: value.clone(),
                expires_at_ms: now_ms.saturating_add(u64::from(ttl) * 1000),
                provider_generation: pending.provider_generation,
            },
        );
        Ok(value)
    }

    pub fn expire(&mut self, now_ms: u64) {
        self.cache.retain(|_, entry| entry.expires_at_ms > now_ms);
        self.pending.retain(|_, query| query.deadline_ms > now_ms);
    }
}

pub fn encode_query(id: u16, hostname: &str, record_type: RecordType) -> Result<Vec<u8>, DnsError> {
    let hostname = normalize_domain(hostname).map_err(|_| DnsError::InvalidName)?;
    let mut out = Vec::with_capacity(12 + hostname.len() + 6);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    encode_name(&mut out, &hostname)?;
    out.extend_from_slice(&(record_type as u16).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    Ok(out)
}

fn encode_name(out: &mut Vec<u8>, hostname: &str) -> Result<(), DnsError> {
    for label in hostname.split('.') {
        let len = u8::try_from(label.len()).map_err(|_| DnsError::InvalidName)?;
        if len == 0 || len > 63 {
            return Err(DnsError::InvalidName);
        }
        out.push(len);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

pub fn dot_frame(query: &[u8]) -> Result<Vec<u8>, DnsError> {
    let len = u16::try_from(query.len()).map_err(|_| DnsError::Capacity)?;
    if query.len() > MAX_DNS_MESSAGE {
        return Err(DnsError::Capacity);
    }
    let mut out = Vec::with_capacity(query.len() + 2);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(query);
    Ok(out)
}

pub fn parse_dot_frame(frame: &[u8]) -> Result<&[u8], DnsError> {
    if frame.len() < 2 {
        return Err(DnsError::Truncated);
    }
    let len = usize::from(u16::from_be_bytes([frame[0], frame[1]]));
    if len > MAX_DNS_MESSAGE || frame.len() != len + 2 {
        return Err(DnsError::InvalidMessage);
    }
    Ok(&frame[2..])
}

pub fn doh_request(host: &str, path: &str, query: &[u8]) -> Result<Vec<u8>, DnsError> {
    if host.is_empty() || !path.starts_with('/') || query.len() > MAX_DNS_MESSAGE {
        return Err(DnsError::InvalidMessage);
    }
    let mut out = alloc::format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/dns-message\r\nAccept: application/dns-message\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        query.len()
    )
    .into_bytes();
    out.extend_from_slice(query);
    Ok(out)
}

pub fn parse_response(
    message: &[u8],
    expected_id: u16,
    expected_name: &str,
    expected_type: RecordType,
) -> Result<ParsedResponse, DnsError> {
    if message.len() < 12 || message.len() > MAX_DNS_MESSAGE {
        return Err(DnsError::InvalidMessage);
    }
    let id = read_u16(message, 0)?;
    let flags = read_u16(message, 2)?;
    if id != expected_id || flags & 0x8000 == 0 {
        return Err(DnsError::MismatchedResponse);
    }
    if flags & 0x0200 != 0 {
        return Err(DnsError::Truncated);
    }
    let qd = usize::from(read_u16(message, 4)?);
    let an = usize::from(read_u16(message, 6)?);
    let ns = usize::from(read_u16(message, 8)?);
    if qd != 1 || an > 64 || ns > 32 {
        return Err(DnsError::InvalidMessage);
    }
    let expected_name = normalize_domain(expected_name).map_err(|_| DnsError::InvalidName)?;
    let mut offset = 12;
    let question = read_name(message, &mut offset, 0)?;
    if question != expected_name
        || read_u16(message, offset)? != expected_type as u16
        || read_u16(message, offset + 2)? != 1
    {
        return Err(DnsError::MismatchedResponse);
    }
    offset += 4;
    let response_code = match flags & 0xf {
        0 => ResponseCode::NoError,
        2 => ResponseCode::ServerFailure,
        3 => ResponseCode::NameError,
        5 => ResponseCode::Refused,
        code => ResponseCode::Other(code as u8),
    };
    let mut addresses = Vec::new();
    let mut cname = None;
    let mut ttl = u32::MAX;
    for _ in 0..an {
        let owner = read_name(message, &mut offset, 0)?;
        let kind = read_u16(message, offset)?;
        let class = read_u16(message, offset + 2)?;
        let record_ttl = read_u32(message, offset + 4)?;
        let length = usize::from(read_u16(message, offset + 8)?);
        offset = offset.checked_add(10).ok_or(DnsError::InvalidMessage)?;
        let end = offset.checked_add(length).ok_or(DnsError::InvalidMessage)?;
        if end > message.len() {
            return Err(DnsError::Truncated);
        }
        if class == 1 && (owner == expected_name || cname.as_ref() == Some(&owner)) {
            match (kind, length) {
                (1, 4) if expected_type == RecordType::A => {
                    if addresses.len() < MAX_DNS_RESULTS {
                        addresses.push(IpAddress::V4(message[offset..end].try_into().unwrap()));
                    }
                    ttl = ttl.min(record_ttl);
                }
                (28, 16) if expected_type == RecordType::Aaaa => {
                    if addresses.len() < MAX_DNS_RESULTS {
                        addresses.push(IpAddress::V6(message[offset..end].try_into().unwrap()));
                    }
                    ttl = ttl.min(record_ttl);
                }
                (5, _) => {
                    let mut cname_offset = offset;
                    cname = Some(read_name(message, &mut cname_offset, 0)?);
                    ttl = ttl.min(record_ttl);
                }
                _ => {}
            }
        }
        offset = end;
    }
    let mut negative_ttl = 0;
    for _ in 0..ns {
        let _ = read_name(message, &mut offset, 0)?;
        let kind = read_u16(message, offset)?;
        let _class = read_u16(message, offset + 2)?;
        let record_ttl = read_u32(message, offset + 4)?;
        let length = usize::from(read_u16(message, offset + 8)?);
        offset += 10;
        let end = offset.checked_add(length).ok_or(DnsError::InvalidMessage)?;
        if end > message.len() {
            return Err(DnsError::Truncated);
        }
        if kind == 6 {
            // RFC 2308 uses the smaller of the SOA RR TTL and the SOA
            // MINIMUM field for negative caching.
            let minimum = end
                .checked_sub(4)
                .filter(|minimum| *minimum >= offset)
                .and_then(|minimum| read_u32(message, minimum).ok())
                .unwrap_or(0);
            negative_ttl = record_ttl.min(minimum).min(MAX_NEGATIVE_TTL_SECONDS);
        }
        offset = end;
    }
    if cname.is_some() && addresses.is_empty() && an > MAX_CNAME_DEPTH {
        return Err(DnsError::CnameLoop);
    }
    Ok(ParsedResponse {
        addresses,
        cname,
        ttl_seconds: if ttl == u32::MAX { negative_ttl } else { ttl },
        response_code,
    })
}

fn read_name(message: &[u8], offset: &mut usize, depth: usize) -> Result<String, DnsError> {
    if depth > MAX_CNAME_DEPTH || *offset >= message.len() {
        return Err(DnsError::InvalidMessage);
    }
    let mut labels = Vec::new();
    let mut cursor = *offset;
    let mut jumped = false;
    loop {
        let length = *message.get(cursor).ok_or(DnsError::Truncated)?;
        if length & 0xc0 == 0xc0 {
            let low = *message.get(cursor + 1).ok_or(DnsError::Truncated)?;
            let pointer = usize::from(u16::from_be_bytes([length & 0x3f, low]));
            if !jumped {
                *offset = cursor + 2;
            }
            let mut pointer_offset = pointer;
            labels.push(read_name(message, &mut pointer_offset, depth + 1)?);
            break;
        }
        if length == 0 {
            if !jumped {
                *offset = cursor + 1;
            }
            break;
        }
        if length > 63 {
            return Err(DnsError::InvalidMessage);
        }
        cursor += 1;
        let end = cursor
            .checked_add(usize::from(length))
            .ok_or(DnsError::InvalidMessage)?;
        let label = core::str::from_utf8(message.get(cursor..end).ok_or(DnsError::Truncated)?)
            .map_err(|_| DnsError::InvalidName)?;
        if !label.is_ascii() {
            return Err(DnsError::InvalidName);
        }
        labels.push(label.to_ascii_lowercase());
        cursor = end;
        if !jumped {
            *offset = cursor;
        }
        jumped = jumped || false;
    }
    let name = labels.join(".");
    normalize_domain(&name).map_err(|_| DnsError::InvalidName)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, DnsError> {
    Ok(u16::from_be_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(DnsError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, DnsError> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(DnsError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_header(
        id: u16,
        flags: u16,
        answers: u16,
        authority: u16,
        question: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&flags.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&answers.to_be_bytes());
        out.extend_from_slice(&authority.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(question);
        out
    }

    fn append_rr(out: &mut Vec<u8>, owner: &[u8], kind: u16, ttl: u32, rdata: &[u8]) {
        out.extend_from_slice(owner);
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&ttl.to_be_bytes());
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(rdata);
    }

    #[test]
    fn query_and_secure_transport_frames_are_bounded() {
        let query = encode_query(7, "Example.COM.", RecordType::Aaaa).unwrap();
        assert_eq!(parse_dot_frame(&dot_frame(&query).unwrap()).unwrap(), query);
        let http = doh_request("dns.example", "/dns-query", &query).unwrap();
        assert!(http.starts_with(b"POST /dns-query HTTP/1.1\r\n"));
        assert!(http.ends_with(&query));
    }

    #[test]
    fn cache_is_provider_table_and_type_scoped() {
        let mut state = ResolverState::new(8);
        let a = state
            .begin(1, 7, 1, "host.test", RecordType::A, 1, 1000)
            .unwrap()
            .key
            .clone();
        let aaaa = state
            .begin(1, 7, 1, "host.test", RecordType::Aaaa, 2, 1000)
            .unwrap()
            .key
            .clone();
        let other = state
            .begin(2, 7, 1, "host.test", RecordType::A, 3, 1000)
            .unwrap()
            .key
            .clone();
        assert_ne!(a, aaaa);
        assert_ne!(a, other);
        assert_eq!(state.pending().len(), 3);
    }

    #[test]
    fn validates_id_question_type_and_parses_a_and_aaaa() {
        for (record_type, rdata, expected) in [
            (
                RecordType::A,
                alloc::vec![192, 0, 2, 9],
                IpAddress::V4([192, 0, 2, 9]),
            ),
            (
                RecordType::Aaaa,
                alloc::vec![0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9],
                IpAddress::V6([0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9]),
            ),
        ] {
            let query = encode_query(0x1234, "host.example", record_type).unwrap();
            let mut response = response_header(0x1234, 0x8180, 1, 0, &query[12..]);
            append_rr(
                &mut response,
                &[0xc0, 0x0c],
                record_type as u16,
                600,
                &rdata,
            );
            let parsed = parse_response(&response, 0x1234, "host.example", record_type).unwrap();
            assert_eq!(parsed.addresses, alloc::vec![expected]);
            assert_eq!(parsed.ttl_seconds, 600);
            assert_eq!(
                parse_response(&response, 0x1235, "host.example", record_type),
                Err(DnsError::MismatchedResponse)
            );
            assert_eq!(
                parse_response(&response, 0x1234, "other.example", record_type),
                Err(DnsError::MismatchedResponse)
            );
        }
    }

    #[test]
    fn follows_cname_answer_and_bounds_negative_soa_ttl() {
        let query = encode_query(7, "alias.example", RecordType::A).unwrap();
        let mut cname = Vec::new();
        encode_name(&mut cname, "target.example").unwrap();
        let mut response = response_header(7, 0x8180, 2, 0, &query[12..]);
        append_rr(&mut response, &[0xc0, 0x0c], 5, 90, &cname);
        append_rr(&mut response, &cname, 1, 60, &[203, 0, 113, 4]);
        let parsed = parse_response(&response, 7, "alias.example", RecordType::A).unwrap();
        assert_eq!(parsed.cname.as_deref(), Some("target.example"));
        assert_eq!(
            parsed.addresses,
            alloc::vec![IpAddress::V4([203, 0, 113, 4])]
        );
        assert_eq!(parsed.ttl_seconds, 60);

        let mut nxdomain = response_header(7, 0x8183, 0, 1, &query[12..]);
        let mut soa = Vec::new();
        encode_name(&mut soa, "ns.example").unwrap();
        encode_name(&mut soa, "hostmaster.example").unwrap();
        for value in [1u32, 60, 60, 600, 120] {
            soa.extend_from_slice(&value.to_be_bytes());
        }
        append_rr(&mut nxdomain, &[0xc0, 0x0c], 6, 500, &soa);
        let parsed = parse_response(&nxdomain, 7, "alias.example", RecordType::A).unwrap();
        assert_eq!(parsed.response_code, ResponseCode::NameError);
        assert_eq!(parsed.ttl_seconds, 120);
    }

    #[test]
    fn positive_ttl_is_preserved_but_negative_ttl_is_bounded() {
        let mut state = ResolverState::new(4);
        let positive = CacheKey {
            table: 1,
            provider: 2,
            hostname: "positive.example".into(),
            record_type: RecordType::A,
        };
        state.insert_value(
            positive.clone(),
            CacheValue::Positive(alloc::vec![IpAddress::V4([192, 0, 2, 1])]),
            3_600,
            10,
            1,
        );
        assert_eq!(state.cache()[&positive].expires_at_ms, 3_600_010);
        let negative = CacheKey {
            hostname: "negative.example".into(),
            ..positive
        };
        state.insert_value(
            negative.clone(),
            CacheValue::Negative(ResponseCode::NameError),
            3_600,
            10,
            1,
        );
        assert_eq!(state.cache()[&negative].expires_at_ms, 300_010);
    }
}
