//! Authenticated replacement commitment, reached only by the resident owner.
//! The generic normal-world transport rejects operation numbers in this range.
use crate::ql::Transport;
pub const OPERATION: u32 = 0x8000_0001;
pub const BYTES: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub architecture: u32,
    pub component: u32,
    pub slot: u32,
    pub generation: u64,
    pub previous: u64,
    pub image_hash: [u8; 32],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Rollback,
    Unavailable,
    Uncertain,
}

fn request(architecture: u32, component: u32) -> Result<[u8; BYTES], Error> {
    if !matches!(architecture, 1 | 2) || !(component == 1 || (architecture == 2 && component == 2))
    {
        return Err(Error::Invalid);
    }
    let mut bytes = [0; BYTES];
    bytes[..8].copy_from_slice(b"BEXJR001");
    bytes[12..16].copy_from_slice(&architecture.to_le_bytes());
    bytes[16..20].copy_from_slice(&component.to_le_bytes());
    Ok(bytes)
}
pub fn query(
    transport: &mut impl Transport,
    architecture: u32,
    component: u32,
) -> Result<Record, Error> {
    let mut bytes = request(architecture, component)?;
    bytes[8] = 1;
    if transport
        .exchange(OPERATION, BYTES, &mut bytes)
        .map_err(|_| Error::Unavailable)?
        != BYTES as i64
    {
        return Err(Error::Unavailable);
    }
    decode(&bytes, architecture, component)
}
/// The caller must already have validated candidate health and fenced writes.
/// Any lost/malformed response after publishing requires an authenticated query
/// before choosing either owner; retrying the identical record is idempotent.
pub fn commit(transport: &mut impl Transport, record: Record) -> Result<Record, Error> {
    let mut bytes = request(record.architecture, record.component)?;
    bytes[20..24].copy_from_slice(&record.slot.to_le_bytes());
    bytes[24..32].copy_from_slice(&record.generation.to_le_bytes());
    bytes[32..40].copy_from_slice(&record.previous.to_le_bytes());
    bytes[40..72].copy_from_slice(&record.image_hash);
    if record.generation <= 1
        || record.previous == 0
        || decode(&bytes, record.architecture, record.component)? != record
    {
        return Err(Error::Invalid);
    }
    bytes[8] = 2;
    if transport
        .exchange(OPERATION, BYTES, &mut bytes)
        .map_err(|_| Error::Uncertain)?
        != BYTES as i64
    {
        return Err(Error::Uncertain);
    }
    match decode(&bytes, record.architecture, record.component) {
        Ok(committed) if committed == record => Ok(committed),
        // Invalid or unavailable may be an unrelated/stale reply. Once the
        // request left this owner, only exact commitment resolves retirement.
        _ => Err(Error::Uncertain),
    }
}
fn decode(bytes: &[u8; BYTES], architecture: u32, component: u32) -> Result<Record, Error> {
    if &bytes[..8] != b"BEXJR001" {
        return Err(Error::Invalid);
    }
    let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let wide = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    match word(8) {
        0 => {}
        1 => return Err(Error::Invalid),
        2 => return Err(Error::Rollback),
        3 => return Err(Error::Unavailable),
        4 => return Err(Error::Uncertain),
        _ => return Err(Error::Invalid),
    }
    if word(12) != architecture
        || word(16) != component
        || bytes[72..] != [0; 24]
        || !matches!(word(20), 1 | 2)
        || wide(24) == 0
    {
        return Err(Error::Invalid);
    }
    let record = Record {
        architecture,
        component,
        slot: word(20),
        generation: wide(24),
        previous: wide(32),
        image_hash: bytes[40..72].try_into().unwrap(),
    };
    if record.generation == 1 {
        if record.slot != 1 || record.previous != 0 || record.image_hash != [0; 32] {
            return Err(Error::Invalid);
        }
    } else if record.previous == 0
        || record.previous >= record.generation
        || record.image_hash == [0; 32]
    {
        return Err(Error::Invalid);
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Lost;
    impl Transport for Lost {
        fn exchange(
            &mut self,
            op: u32,
            size: usize,
            bytes: &mut [u8],
        ) -> Result<i64, crate::ql::Error> {
            assert_eq!((op, size, bytes.len()), (OPERATION, BYTES, BYTES));
            Err(crate::ql::Error::Timeout)
        }
        fn now_ns(&self) -> u64 {
            0
        }
    }
    #[test]
    fn commitment_loss_never_authorizes_rollback() {
        let record = Record {
            architecture: 2,
            component: 1,
            slot: 2,
            generation: 7,
            previous: 1,
            image_hash: [0x5a; 32],
        };
        assert_eq!(commit(&mut Lost, record), Err(Error::Uncertain));
        assert_eq!(query(&mut Lost, 2, 1), Err(Error::Unavailable));
        assert_eq!(
            commit(
                &mut Lost,
                Record {
                    previous: 7,
                    ..record
                }
            ),
            Err(Error::Invalid)
        );
        assert_eq!(
            commit(
                &mut Lost,
                Record {
                    image_hash: [0; 32],
                    ..record
                }
            ),
            Err(Error::Invalid)
        );
        assert_eq!(query(&mut Lost, 1, 2), Err(Error::Invalid));
    }
}
