//! Incremental versioned record copy with bounded, ordered mutation replay.
//!
//! The bulk copy may observe newer versions while it advances. A receiver keeps
//! the newest version of each record, so replay cannot regress a bulk record.
//! The receiver is not usable until it has consumed the complete delta prefix.
use crate::{
    Error, VERSION,
    codec::{Decoder, Encoder, checksum},
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec::Vec,
};
use core::ops::Bound::{Excluded, Unbounded};

const MAGIC: u64 = u64::from_le_bytes(*b"BEXMIG01");
const OVERHEAD: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub key: u64,
    pub sequence: u64,
    pub data: Option<Arc<[u8]>>,
}
impl Record {
    pub fn encode(&self, generation: u64) -> Vec<u8> {
        let mut w = Encoder::new();
        for n in [
            MAGIC,
            VERSION,
            generation,
            self.key,
            self.sequence,
            self.data.is_some() as u64,
        ] {
            w.word(n);
        }
        w.bytes(self.data.as_deref().unwrap_or(&[]));
        let mut bytes = w.finish();
        bytes.extend_from_slice(&checksum(&bytes).to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8], generation: u64, limit: usize) -> Result<Self, Error> {
        if bytes.len() < OVERHEAD || bytes.len() > limit.saturating_add(OVERHEAD) {
            return Err(Error::Capacity);
        }
        let (payload, tail) = bytes.split_at(bytes.len() - 8);
        if checksum(payload) != u64::from_le_bytes(tail.try_into().unwrap()) {
            return Err(Error::Checksum);
        }
        let mut r = Decoder::new(payload);
        if r.word()? != MAGIC {
            return Err(Error::InvalidData);
        }
        if r.word()? != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if r.word()? != generation {
            return Err(Error::Sequence);
        }
        let key = r.word()?;
        let sequence = r.word()?;
        let present = r.flag()?;
        let data = r.bytes(limit)?;
        if !present && !data.is_empty() {
            return Err(Error::InvalidData);
        }
        r.finish()?;
        Ok(Self {
            key,
            sequence,
            data: present.then(|| Arc::from(data)),
        })
    }
    fn cost(&self) -> usize {
        OVERHEAD + self.data.as_ref().map_or(0, |data| data.len())
    }
}

pub struct Store {
    records: BTreeMap<u64, Record>,
    sequence: u64,
    journal: Option<VecDeque<Record>>,
    journal_bytes: usize,
    limit: usize,
    failed: bool,
}
pub struct BulkCursor {
    next: Option<u64>,
    upper: Option<u64>,
    pub sequence: u64,
    done: bool,
}
impl Store {
    pub fn new(journal_limit: usize) -> Self {
        Self {
            records: BTreeMap::new(),
            sequence: 0,
            journal: None,
            journal_bytes: 0,
            limit: journal_limit,
            failed: false,
        }
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn records(&self) -> &BTreeMap<u64, Record> {
        &self.records
    }
    /// An overflow aborts migration, but never drops the authoritative mutation.
    pub fn set(&mut self, key: u64, data: Option<Arc<[u8]>>) -> Result<(), Error> {
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Sequence)?;
        let record = Record {
            key,
            sequence: self.sequence,
            data,
        };
        if record.data.is_some() {
            self.records.insert(key, record.clone());
        } else {
            self.records.remove(&key);
        }
        if let Some(journal) = &mut self.journal {
            if record.cost() > self.limit.saturating_sub(self.journal_bytes) {
                self.failed = true;
                self.journal = None;
                self.journal_bytes = 0;
                return Err(Error::Capacity);
            }
            self.journal_bytes += record.cost();
            journal.push_back(record);
        }
        Ok(())
    }
    pub fn begin(&mut self) -> Result<BulkCursor, Error> {
        if self.journal.is_some() {
            return Err(Error::BadState);
        }
        self.failed = false;
        self.journal = Some(VecDeque::new());
        self.journal_bytes = 0;
        Ok(BulkCursor {
            next: None,
            upper: self.records.last_key_value().map(|(key, _)| *key),
            sequence: self.sequence,
            done: false,
        })
    }
    pub fn bulk_next(&self, cursor: &mut BulkCursor) -> Result<Option<Record>, Error> {
        if self.failed {
            return Err(Error::Capacity);
        }
        if self.journal.is_none() {
            return Err(Error::BadState);
        }
        if cursor.done {
            return Ok(None);
        }
        let next = match cursor.next {
            None => self.records.first_key_value(),
            Some(key) => self.records.range((Excluded(key), Unbounded)).next(),
        };
        match next {
            Some((key, record)) if cursor.upper.is_some_and(|upper| *key <= upper) => {
                cursor.next = Some(*key);
                Ok(Some(record.clone()))
            }
            _ => {
                cursor.done = true;
                Ok(None)
            }
        }
    }
    pub fn delta_next(&mut self) -> Result<Option<Record>, Error> {
        if self.failed {
            return Err(Error::Capacity);
        }
        let record = self.journal.as_mut().ok_or(Error::BadState)?.pop_front();
        if let Some(record) = &record {
            self.journal_bytes -= record.cost();
        }
        Ok(record)
    }
    pub fn end(&mut self) {
        self.journal = None;
        self.journal_bytes = 0;
        self.failed = false;
    }
}

pub struct Receiver {
    records: BTreeMap<u64, Record>,
    tracking: Option<Vec<TrackedRecord>>,
    sequence: u64,
    bytes: usize,
    limit: usize,
    bulk_done: bool,
    bulk_key: Option<u64>,
}
struct TrackedRecord {
    key: u64,
    sequence: u64,
    cost: usize,
    present: bool,
}
impl Receiver {
    pub fn new(base_sequence: u64, limit: usize) -> Self {
        Self {
            records: BTreeMap::new(),
            tracking: None,
            sequence: base_sequence,
            bytes: 0,
            limit,
            bulk_done: false,
            bulk_key: None,
        }
    }
    /// Tracks ordering, capacity, and final key presence without retaining
    /// payload bytes that the caller has already adopted into live state.
    pub fn new_tracking(base_sequence: u64, limit: usize) -> Self {
        Self {
            records: BTreeMap::new(),
            // Bulk records are ordered, so a compact vector avoids interleaving
            // thousands of long-lived tree-node allocations with transient IPC
            // buffers in a fixed userspace heap.
            tracking: Some(Vec::with_capacity(2048)),
            sequence: base_sequence,
            bytes: 0,
            limit,
            bulk_done: false,
            bulk_key: None,
        }
    }
    fn adopt(&mut self, record: Record) -> Result<(), Error> {
        let tracked_index = self
            .tracking
            .as_ref()
            .map(|records| records.binary_search_by_key(&record.key, |old| old.key));
        let old_sequence = match tracked_index {
            Some(Ok(index)) => Some(self.tracking.as_ref().unwrap()[index].sequence),
            Some(Err(_)) => None,
            None => self.records.get(&record.key).map(|old| old.sequence),
        };
        if old_sequence.is_some_and(|old| old >= record.sequence) {
            return Ok(());
        }
        let old_cost = match tracked_index {
            Some(Ok(index)) => self.tracking.as_ref().unwrap()[index].cost,
            Some(Err(_)) => 0,
            None => self.records.get(&record.key).map_or(0, Record::cost),
        };
        let record_cost = record.cost();
        let new_bytes = self
            .bytes
            .saturating_sub(old_cost)
            .checked_add(record_cost)
            .ok_or(Error::Capacity)?;
        if new_bytes > self.limit {
            return Err(Error::Capacity);
        }
        self.bytes = new_bytes;
        if let Some(records) = &mut self.tracking {
            let tracked = TrackedRecord {
                key: record.key,
                sequence: record.sequence,
                cost: record_cost,
                present: record.data.is_some(),
            };
            match tracked_index.unwrap() {
                Ok(index) => records[index] = tracked,
                Err(index) => records.insert(index, tracked),
            }
        } else {
            self.records.insert(record.key, record);
        }
        Ok(())
    }
    pub fn bulk(&mut self, record: Record) -> Result<(), Error> {
        if self.bulk_done || self.bulk_key.is_some_and(|key| record.key <= key) {
            return Err(Error::Sequence);
        }
        let key = record.key;
        self.adopt(record)?;
        self.bulk_key = Some(key);
        Ok(())
    }
    pub fn finish_bulk(&mut self) -> Result<(), Error> {
        if self.bulk_done {
            return Err(Error::BadState);
        }
        self.bulk_done = true;
        Ok(())
    }
    pub fn delta(&mut self, record: Record) -> Result<(), Error> {
        if !self.bulk_done || Some(record.sequence) != self.sequence.checked_add(1) {
            return Err(Error::Sequence);
        }
        let sequence = record.sequence;
        self.adopt(record)?;
        self.sequence = sequence;
        Ok(())
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    fn validate_final(&self, final_sequence: u64) -> Result<(), Error> {
        if !self.bulk_done
            || self.sequence != final_sequence
            || self
                .tracking
                .as_ref()
                .is_some_and(|records| records.iter().any(|r| r.sequence > final_sequence))
            || self.records.values().any(|r| r.sequence > final_sequence)
        {
            return Err(Error::Sequence);
        }
        Ok(())
    }
    pub fn finish(self, final_sequence: u64) -> Result<BTreeMap<u64, Record>, Error> {
        self.validate_final(final_sequence)?;
        if let Some(records) = self.tracking {
            return Ok(records
                .into_iter()
                .filter(|record| record.present)
                .map(|record| {
                    (
                        record.key,
                        Record {
                            key: record.key,
                            sequence: record.sequence,
                            data: Some(Arc::from([])),
                        },
                    )
                })
                .collect());
        }
        Ok(self
            .records
            .into_iter()
            .filter(|(_, record)| record.data.is_some())
            .collect())
    }
    pub fn finish_keys(self, final_sequence: u64) -> Result<Vec<u64>, Error> {
        self.validate_final(final_sequence)?;
        if let Some(records) = self.tracking {
            return Ok(records
                .into_iter()
                .filter(|record| record.present)
                .map(|record| record.key)
                .collect());
        }
        Ok(self
            .records
            .into_iter()
            .filter(|(_, record)| record.data.is_some())
            .map(|(key, _)| key)
            .collect())
    }
}
