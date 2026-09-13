use crate::{Error, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
const BLOBS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("pkg_blobs_v1");
const ACCESSES: TableDefinition<&[u8], u64> = TableDefinition::new("pkg_access_v1");
pub struct Cas {
    database: Database,
    capacity: u64,
    sequence: u64,
}
impl Cas {
    pub fn open(database: Database, capacity: u64) -> Result<Self> {
        let transaction = database.begin_write().map_err(|_| Error::Io)?;
        transaction.open_table(BLOBS).map_err(|_| Error::Io)?;
        transaction.open_table(ACCESSES).map_err(|_| Error::Io)?;
        transaction.commit().map_err(|_| Error::Io)?;
        let read = database.begin_read().map_err(|_| Error::Io)?;
        let table = read.open_table(ACCESSES).map_err(|_| Error::Io)?;
        let mut sequence = 0;
        for entry in table.iter().map_err(|_| Error::Io)? {
            sequence = sequence.max(entry.map_err(|_| Error::Io)?.1.value());
        }
        drop(table);
        drop(read);
        Ok(Self {
            database,
            capacity,
            sequence,
        })
    }
    pub fn get(&mut self, digest: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        let bytes = {
            let read = self.database.begin_read().map_err(|_| Error::Io)?;
            let table = read.open_table(BLOBS).map_err(|_| Error::Io)?;
            table
                .get(digest.as_slice())
                .map_err(|_| Error::Io)?
                .map(|value| value.value().to_vec())
        };
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != *digest {
            self.remove(digest)?;
            return Ok(None);
        }
        self.sequence = self.sequence.saturating_add(1);
        let write = self.database.begin_write().map_err(|_| Error::Io)?;
        write
            .open_table(ACCESSES)
            .map_err(|_| Error::Io)?
            .insert(digest.as_slice(), self.sequence)
            .map_err(|_| Error::Io)?;
        write.commit().map_err(|_| Error::Io)?;
        Ok(Some(bytes))
    }
    pub fn put(
        &mut self,
        digest: &[u8; 32],
        bytes: &[u8],
        pinned: &BTreeSet<[u8; 32]>,
    ) -> Result<()> {
        if <[u8; 32]>::from(Sha256::digest(bytes)) != *digest || bytes.is_empty() {
            return Err(Error::VerifyFailed);
        }
        if bytes.len() as u64 > self.capacity {
            return Err(Error::ResourceExhausted);
        }
        self.sequence = self.sequence.saturating_add(1);
        let transaction = self.database.begin_write().map_err(|_| Error::Io)?;
        {
            let mut blobs = transaction.open_table(BLOBS).map_err(|_| Error::Io)?;
            let mut accesses = transaction.open_table(ACCESSES).map_err(|_| Error::Io)?;
            let mut used = 0u64;
            let mut entries = Vec::new();
            for entry in blobs.iter().map_err(|_| Error::Io)? {
                let (key, value) = entry.map_err(|_| Error::Io)?;
                let hash: [u8; 32] = key.value().try_into().map_err(|_| Error::VerifyFailed)?;
                if hash == *digest {
                    continue;
                }
                let length = value.value().len() as u64;
                used = used.saturating_add(length);
                if !pinned.contains(&hash) {
                    let access = accesses
                        .get(key.value())
                        .map_err(|_| Error::Io)?
                        .map_or(0, |v| v.value());
                    entries.push((access, hash, length));
                }
            }
            entries.sort();
            for (_, hash, length) in entries {
                if used.saturating_add(bytes.len() as u64) <= self.capacity {
                    break;
                }
                blobs.remove(hash.as_slice()).map_err(|_| Error::Io)?;
                accesses.remove(hash.as_slice()).map_err(|_| Error::Io)?;
                used -= length;
            }
            if used.saturating_add(bytes.len() as u64) > self.capacity {
                return Err(Error::ResourceExhausted);
            }
            blobs
                .insert(digest.as_slice(), bytes)
                .map_err(|_| Error::Io)?;
            accesses
                .insert(digest.as_slice(), self.sequence)
                .map_err(|_| Error::Io)?;
        }
        transaction.commit().map_err(|_| Error::Io)
    }
    fn remove(&mut self, digest: &[u8; 32]) -> Result<()> {
        let transaction = self.database.begin_write().map_err(|_| Error::Io)?;
        transaction
            .open_table(BLOBS)
            .map_err(|_| Error::Io)?
            .remove(digest.as_slice())
            .map_err(|_| Error::Io)?;
        transaction
            .open_table(ACCESSES)
            .map_err(|_| Error::Io)?
            .remove(digest.as_slice())
            .map_err(|_| Error::Io)?;
        transaction.commit().map_err(|_| Error::Io)
    }
}
