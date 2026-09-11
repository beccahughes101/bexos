use alloc::vec;
use alloc::vec::Vec;

pub const BEXFS_MAGIC: [u8; 8] = *b"BEXFS001";
pub const BEXFS_VERSION: u32 = 1;
pub const BEXFS_HEADER_BYTES: usize = 160;
pub const KEY_CHECK_PLAINTEXT: [u8; 16] = *b"BEXFS-KEY-CHECK!";
const CHECKSUM_OFFSET: usize = 156;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotDescriptor {
    pub lba: u64,
    pub blocks: u64,
    pub bytes: u64,
    pub nonce: [u8; 12],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BexfsHeader {
    pub generation: u64,
    pub active_slot: u8,
    pub slots: [SlotDescriptor; 2],
    pub key_check: [u8; 32],
    pub volume_uuid: [u8; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeaderError {
    InvalidLength,
    BadMagic,
    UnsupportedVersion,
    BadChecksum,
    InvalidSlot,
}

impl BexfsHeader {
    pub fn encode(&self, block_size: usize) -> Vec<u8> {
        let mut out = vec![0u8; block_size];
        out[0..8].copy_from_slice(&BEXFS_MAGIC);
        out[8..12].copy_from_slice(&BEXFS_VERSION.to_le_bytes());
        out[12..20].copy_from_slice(&self.generation.to_le_bytes());
        out[20] = self.active_slot;
        encode_slot(&mut out[24..60], &self.slots[0]);
        encode_slot(&mut out[60..96], &self.slots[1]);
        out[96..128].copy_from_slice(&self.key_check);
        out[128..144].copy_from_slice(&self.volume_uuid);
        let checksum = rosefs_core::journal::crc32c::crc32c(&out[..CHECKSUM_OFFSET]);
        out[CHECKSUM_OFFSET..BEXFS_HEADER_BYTES].copy_from_slice(&checksum.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, HeaderError> {
        if bytes.len() < BEXFS_HEADER_BYTES {
            return Err(HeaderError::InvalidLength);
        }
        if bytes[0..8] != BEXFS_MAGIC {
            return Err(HeaderError::BadMagic);
        }
        if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != BEXFS_VERSION {
            return Err(HeaderError::UnsupportedVersion);
        }
        let expected = u32::from_le_bytes(
            bytes[CHECKSUM_OFFSET..BEXFS_HEADER_BYTES]
                .try_into()
                .unwrap(),
        );
        if rosefs_core::journal::crc32c::crc32c(&bytes[..CHECKSUM_OFFSET]) != expected {
            return Err(HeaderError::BadChecksum);
        }
        let active_slot = bytes[20];
        if active_slot > 1 {
            return Err(HeaderError::InvalidSlot);
        }
        Ok(Self {
            generation: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            active_slot,
            slots: [decode_slot(&bytes[24..60]), decode_slot(&bytes[60..96])],
            key_check: bytes[96..128].try_into().unwrap(),
            volume_uuid: bytes[128..144].try_into().unwrap(),
        })
    }
}

pub fn nonce(generation: u64, slot: u8) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&generation.to_le_bytes());
    nonce[8] = slot;
    nonce[9..12].copy_from_slice(b"BEX");
    nonce
}

fn encode_slot(out: &mut [u8], slot: &SlotDescriptor) {
    out[0..8].copy_from_slice(&slot.lba.to_le_bytes());
    out[8..16].copy_from_slice(&slot.blocks.to_le_bytes());
    out[16..24].copy_from_slice(&slot.bytes.to_le_bytes());
    out[24..36].copy_from_slice(&slot.nonce);
}

fn decode_slot(bytes: &[u8]) -> SlotDescriptor {
    SlotDescriptor {
        lba: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
        blocks: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        bytes: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        nonce: bytes[24..36].try_into().unwrap(),
    }
}
