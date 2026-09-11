use alloc::string::String;
use alloc::vec;

use rosefs_core::block::{BlockDevice, BlockIoError};

const SECTOR_BYTES: usize = 512;
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GptPartition {
    pub first_lba: u64,
    pub last_lba: u64,
    pub label: String,
}

impl GptPartition {
    pub fn sector_count(&self) -> u64 {
        self.last_lba - self.first_lba + 1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GptError {
    Io,
    BadGeometry,
    BadSignature,
    BadHeaderChecksum,
    BadEntryChecksum,
    InvalidHeader,
    InvalidUtf16,
    NotFound,
}

impl From<BlockIoError> for GptError {
    fn from(_: BlockIoError) -> Self {
        Self::Io
    }
}

pub fn find_partition(
    device: &dyn BlockDevice,
    wanted_label: &str,
) -> Result<GptPartition, GptError> {
    if device.block_size() != SECTOR_BYTES as u32 {
        return Err(GptError::BadGeometry);
    }
    let mut header = [0u8; SECTOR_BYTES];
    device.read_at(1, &mut header)?;
    if &header[0..8] != GPT_SIGNATURE {
        return Err(GptError::BadSignature);
    }
    let header_size = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
    if !(92..=SECTOR_BYTES).contains(&header_size) {
        return Err(GptError::InvalidHeader);
    }
    let expected_header_crc = u32::from_le_bytes(header[16..20].try_into().unwrap());
    let mut checked = header;
    checked[16..20].fill(0);
    if crc32_ieee(&checked[..header_size]) != expected_header_crc {
        return Err(GptError::BadHeaderChecksum);
    }
    let entries_lba = u64::from_le_bytes(header[72..80].try_into().unwrap());
    let entry_count = u32::from_le_bytes(header[80..84].try_into().unwrap()) as usize;
    let entry_size = u32::from_le_bytes(header[84..88].try_into().unwrap()) as usize;
    let expected_entries_crc = u32::from_le_bytes(header[88..92].try_into().unwrap());
    if entry_count == 0 || entry_count > 4096 || entry_size < 128 || entry_size > 4096 {
        return Err(GptError::InvalidHeader);
    }
    let entries_len = entry_count
        .checked_mul(entry_size)
        .ok_or(GptError::InvalidHeader)?;
    let transfer_len = entries_len.div_ceil(SECTOR_BYTES) * SECTOR_BYTES;
    let mut entries = vec![0u8; transfer_len];
    device.read_at(entries_lba, &mut entries)?;
    if crc32_ieee(&entries[..entries_len]) != expected_entries_crc {
        return Err(GptError::BadEntryChecksum);
    }
    for entry in entries[..entries_len].chunks_exact(entry_size) {
        if entry[..16].iter().all(|byte| *byte == 0) {
            continue;
        }
        let first_lba = u64::from_le_bytes(entry[32..40].try_into().unwrap());
        let last_lba = u64::from_le_bytes(entry[40..48].try_into().unwrap());
        if first_lba > last_lba || last_lba >= device.num_blocks() {
            return Err(GptError::InvalidHeader);
        }
        let label = decode_label(&entry[56..128])?;
        if label == wanted_label {
            return Ok(GptPartition {
                first_lba,
                last_lba,
                label,
            });
        }
    }
    Err(GptError::NotFound)
}

fn decode_label(bytes: &[u8]) -> Result<String, GptError> {
    let mut words = [0u16; 36];
    let mut len = 0;
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let word = u16::from_le_bytes([pair[0], pair[1]]);
        if word == 0 {
            break;
        }
        words[index] = word;
        len += 1;
    }
    String::from_utf16(&words[..len]).map_err(|_| GptError::InvalidUtf16)
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
