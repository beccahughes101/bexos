use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

use crate::controller::LazyServiceSnapshot;

const SNAPSHOT_VERSION: u64 = 1;

pub fn encode_snapshot(snapshot: LazyServiceSnapshot) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(SNAPSHOT_VERSION);
    w.word(snapshot.generation);
    w.word(snapshot.connection_count as u64);
    w.word(snapshot.keep_alive_count as u64);
    w.word(snapshot.idle_timeout_ns);
    match snapshot.idle_deadline_ns {
        Some(deadline) => {
            w.word(1);
            w.word(deadline);
        }
        None => {
            w.word(0);
            w.word(0);
        }
    }
    w.word(snapshot.stopping as u64);
    w.word(snapshot.suspended as u64);
    w.finish()
}

pub fn decode_snapshot(bytes: &[u8]) -> Result<LazyServiceSnapshot, Error> {
    let mut r = Decoder::new(bytes);
    if r.word()? != SNAPSHOT_VERSION {
        return Err(Error::UnsupportedVersion);
    }
    let generation = r.word()?;
    let connection_count = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let keep_alive_count = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let idle_timeout_ns = r.word()?;
    let idle_deadline_ns = if r.flag()? {
        Some(r.word()?)
    } else {
        let _ = r.word()?;
        None
    };
    let stopping = r.flag()?;
    let suspended = r.flag()?;
    r.finish()?;
    Ok(LazyServiceSnapshot {
        generation,
        connection_count,
        keep_alive_count,
        idle_timeout_ns,
        idle_deadline_ns,
        stopping,
        suspended,
    })
}
