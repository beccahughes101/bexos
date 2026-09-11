use alloc::string::{String, ToString};
use alloc::vec::Vec;
use net_fidl::Status;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DnsCache {
    entries: Vec<DnsEntry>,
    pending: Vec<PendingDnsQuery>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnsEntry {
    pub hostname: String,
    pub records: Vec<DnsRecord>,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DnsRecord {
    A([u8; 4]),
    Aaaa([u8; 16]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DnsTransport {
    Udp53,
    Doh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDnsQuery {
    pub hostname: String,
    pub id: u16,
    pub transport: DnsTransport,
}

impl DnsCache {
    pub fn entries(&self) -> &[DnsEntry] {
        &self.entries
    }

    pub fn replace_entries(&mut self, entries: Vec<DnsEntry>) {
        self.entries = entries;
    }

    pub fn pending(&self) -> &[PendingDnsQuery] {
        &self.pending
    }

    pub fn replace_pending(&mut self, pending: Vec<PendingDnsQuery>) {
        self.pending = pending;
    }

    pub fn lookup(&self, hostname: &str, now_ms: u64) -> Option<&[DnsRecord]> {
        self.entries
            .iter()
            .find(|entry| entry.hostname == hostname && entry.expires_at_ms > now_ms)
            .map(|entry| entry.records.as_slice())
    }

    pub fn insert(&mut self, hostname: &str, records: &[DnsRecord], expires_at_ms: u64) -> Status {
        if hostname.is_empty() || hostname.len() > 255 || records.len() > 8 {
            return Status::ErrInvalidArgs;
        }
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.hostname == hostname)
        {
            entry.records.clear();
            entry.records.extend_from_slice(records);
            entry.expires_at_ms = expires_at_ms;
            return Status::Ok;
        }
        if self.entries.len() >= 64 {
            return Status::ErrResourceExhausted;
        }
        self.entries.push(DnsEntry {
            hostname: hostname.to_string(),
            records: records.to_vec(),
            expires_at_ms,
        });
        Status::Ok
    }

    pub fn begin_query(
        &mut self,
        hostname: &str,
        transport: DnsTransport,
        out: &mut [u8],
    ) -> Result<usize, Status> {
        if self.pending.iter().any(|query| query.hostname == hostname) {
            return Err(Status::ErrShouldWait);
        }
        if self.pending.len() >= 32 {
            return Err(Status::ErrResourceExhausted);
        }
        let id = query_id(hostname);
        let len = encode_query(hostname, id, out)?;
        self.pending.push(PendingDnsQuery {
            hostname: hostname.to_string(),
            id,
            transport,
        });
        Ok(len)
    }

    pub fn complete_response(&mut self, response: &[u8], now_ms: u64) -> Status {
        for i in 0..self.pending.len() {
            let query = self.pending[i].clone();
            if let Ok(records) = parse_records(response, query.id) {
                self.pending.remove(i);
                return self.insert(&query.hostname, &records, now_ms.saturating_add(60_000));
            }
        }
        Status::ErrInvalidArgs
    }
}

fn query_id(hostname: &str) -> u16 {
    hostname
        .bytes()
        .fold(0x4d53u16, |hash, byte| hash.rotate_left(5) ^ byte as u16)
}

pub fn encode_query(hostname: &str, id: u16, out: &mut [u8]) -> Result<usize, Status> {
    if hostname.is_empty() || hostname.len() > 255 || out.len() < 17 {
        return Err(Status::ErrInvalidArgs);
    }
    out[0..2].copy_from_slice(&id.to_be_bytes());
    out[2..4].copy_from_slice(&0x0100u16.to_be_bytes());
    out[4..6].copy_from_slice(&1u16.to_be_bytes());
    out[6..12].fill(0);
    let mut offset = 12;
    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 || offset + 1 + label.len() >= out.len() {
            return Err(Status::ErrInvalidArgs);
        }
        out[offset] = label.len() as u8;
        offset += 1;
        out[offset..offset + label.len()].copy_from_slice(label.as_bytes());
        offset += label.len();
    }
    if offset + 5 > out.len() {
        return Err(Status::ErrBufferTooSmall);
    }
    out[offset] = 0;
    offset += 1;
    out[offset..offset + 2].copy_from_slice(&1u16.to_be_bytes());
    offset += 2;
    out[offset..offset + 2].copy_from_slice(&1u16.to_be_bytes());
    offset += 2;
    Ok(offset)
}

pub fn parse_a_records(response: &[u8], id: u16) -> Result<Vec<[u8; 4]>, Status> {
    Ok(parse_records(response, id)?
        .into_iter()
        .filter_map(|record| match record {
            DnsRecord::A(address) => Some(address),
            DnsRecord::Aaaa(_) => None,
        })
        .collect())
}

pub fn parse_records(response: &[u8], id: u16) -> Result<Vec<DnsRecord>, Status> {
    if response.len() < 12 || u16::from_be_bytes([response[0], response[1]]) != id {
        return Err(Status::ErrInvalidArgs);
    }
    let qd = u16::from_be_bytes([response[4], response[5]]) as usize;
    let an = u16::from_be_bytes([response[6], response[7]]) as usize;
    let mut offset = 12;
    for _ in 0..qd {
        offset = skip_name(response, offset)?;
        offset = offset.checked_add(4).ok_or(Status::ErrInvalidArgs)?;
        if offset > response.len() {
            return Err(Status::ErrInvalidArgs);
        }
    }
    let mut records = Vec::new();
    for _ in 0..an {
        offset = skip_name(response, offset)?;
        if offset + 10 > response.len() {
            return Err(Status::ErrInvalidArgs);
        }
        let ty = u16::from_be_bytes([response[offset], response[offset + 1]]);
        let class = u16::from_be_bytes([response[offset + 2], response[offset + 3]]);
        let len = u16::from_be_bytes([response[offset + 8], response[offset + 9]]) as usize;
        offset += 10;
        if offset + len > response.len() {
            return Err(Status::ErrInvalidArgs);
        }
        if ty == 1 && class == 1 && len == 4 && records.len() < 8 {
            records.push(DnsRecord::A([
                response[offset],
                response[offset + 1],
                response[offset + 2],
                response[offset + 3],
            ]));
        } else if ty == 28 && class == 1 && len == 16 && records.len() < 8 {
            let mut address = [0u8; 16];
            address.copy_from_slice(&response[offset..offset + 16]);
            records.push(DnsRecord::Aaaa(address));
        }
        offset += len;
    }
    Ok(records)
}

fn skip_name(packet: &[u8], mut offset: usize) -> Result<usize, Status> {
    let mut resume = None;
    let mut jumps = 0;
    loop {
        let Some(&len) = packet.get(offset) else {
            return Err(Status::ErrInvalidArgs);
        };
        if len & 0xc0 == 0xc0 {
            if offset + 1 >= packet.len() {
                return Err(Status::ErrInvalidArgs);
            }
            resume.get_or_insert(offset + 2);
            offset = (((len & 0x3f) as usize) << 8) | packet[offset + 1] as usize;
            jumps += 1;
            if jumps > 8 {
                return Err(Status::ErrInvalidArgs);
            }
            continue;
        }
        offset += 1;
        if len == 0 {
            return Ok(resume.unwrap_or(offset));
        }
        offset = offset
            .checked_add(len as usize)
            .ok_or(Status::ErrInvalidArgs)?;
        if offset > packet.len() {
            return Err(Status::ErrInvalidArgs);
        }
    }
}
