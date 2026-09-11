//! Two-slot BEXCFG storage. Candidate snapshots never overwrite the committed slot.
use crate::{ConfigTable, schema::Error};
use alloc::vec::Vec;

pub trait Files {
    /// Missing is distinct from unreadable. Reads must return the entire file.
    fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, Error>;
    fn write_sync(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error>;
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub generation: u64,
    pub bytes: Vec<u8>,
    pub slot: usize,
}
const SNAPSHOTS: [&str; 2] = ["0.bexpref", "1.bexpref"];
const COMMITS: [&str; 2] = ["0.commit", "1.commit"];
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
pub fn load(files: &mut impl Files) -> Result<Option<Record>, Error> {
    let mut best: Option<Record> = None;
    let mut saw_commit = false;
    for slot in 0..2 {
        let Some(commit) = files.read(COMMITS[slot])? else {
            continue;
        };
        saw_commit = true;
        if commit.len() != 32 || &commit[..8] != b"BEXPREF2" {
            continue;
        }
        let generation = u64::from_le_bytes(commit[8..16].try_into().unwrap());
        let length = u64::from_le_bytes(commit[16..24].try_into().unwrap());
        let hash = u64::from_le_bytes(commit[24..32].try_into().unwrap());
        let Some(bytes) = files.read(SNAPSHOTS[slot])? else {
            continue;
        };
        if length != bytes.len() as u64 || hash != checksum(&bytes) {
            continue;
        }
        let Ok(table) = ConfigTable::parse(&bytes) else {
            continue;
        };
        if table.version() != 2 || table.generation() != generation {
            continue;
        }
        if best.as_ref().is_none_or(|old| generation > old.generation) {
            best = Some(Record {
                generation,
                bytes,
                slot,
            });
        }
    }
    if best.is_none() && saw_commit {
        return Err(Error::Storage);
    }
    Ok(best)
}
pub fn commit(files: &mut impl Files, expected: u64, bytes: &[u8]) -> Result<Record, Error> {
    let table = ConfigTable::parse(bytes).map_err(|_| Error::Malformed)?;
    let generation = table.generation();
    if table.version() != 2 || generation <= expected {
        return Err(Error::Conflict);
    }
    let old = load(files)?;
    let replay = old
        .as_ref()
        .is_some_and(|r| r.generation == generation && r.bytes == bytes);
    if !replay && old.as_ref().map_or(0, |r| r.generation) != expected {
        return Err(Error::Conflict);
    }
    let slot = if replay {
        old.as_ref().unwrap().slot
    } else {
        old.as_ref().map_or(0, |r| 1 - r.slot)
    };
    if !replay {
        files.write_sync(SNAPSHOTS[slot], bytes)?;
    }
    let mut marker = Vec::from(&b"BEXPREF2"[..]);
    marker.extend_from_slice(&generation.to_le_bytes());
    marker.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    marker.extend_from_slice(&checksum(bytes).to_le_bytes());
    // A failed synchronization can have committed. Retrying the exact marker is
    // safe; callers must never turn this uncertainty into an abort decision.
    files
        .write_sync(COMMITS[slot], &marker)
        .map_err(|_| Error::CommitUncertain)?;
    Ok(Record {
        generation,
        bytes: bytes.to_vec(),
        slot,
    })
}
