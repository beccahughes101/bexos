pub const MAGIC: &[u8; 8] = b"BEXBOOT\0";
pub const VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 24;
pub const ENTRY_SIZE: usize = 272;
pub const PATH_SIZE: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootfsEntry<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub payload_offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bootfs<'a> {
    bytes: &'a [u8],
    count: u32,
    table_size: u32,
    payload_offset: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootfsError {
    TooSmall,
    BadMagic,
    UnsupportedVersion,
    TableOutOfBounds,
    EntryOutOfBounds,
    BadPath,
    PayloadOutOfBounds,
}

impl<'a> Bootfs<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, BootfsError> {
        if bytes.len() < HEADER_SIZE {
            return Err(BootfsError::TooSmall);
        }
        if &bytes[0..8] != MAGIC {
            return Err(BootfsError::BadMagic);
        }
        let version = read_u32(bytes, 8)?;
        if version != VERSION {
            return Err(BootfsError::UnsupportedVersion);
        }
        let count = read_u32(bytes, 12)?;
        let table_size = read_u32(bytes, 16)?;
        let payload_offset = read_u32(bytes, 20)?;
        let expected_table_size = count
            .checked_mul(ENTRY_SIZE as u32)
            .ok_or(BootfsError::TableOutOfBounds)?;
        if table_size != expected_table_size {
            return Err(BootfsError::TableOutOfBounds);
        }
        let table_end = HEADER_SIZE
            .checked_add(table_size as usize)
            .ok_or(BootfsError::TableOutOfBounds)?;
        if table_end > bytes.len() || payload_offset as usize > bytes.len() {
            return Err(BootfsError::TableOutOfBounds);
        }
        Ok(Self {
            bytes,
            count,
            table_size,
            payload_offset,
        })
    }

    pub const fn count(self) -> u32 {
        self.count
    }

    pub const fn payload_offset(self) -> u32 {
        self.payload_offset
    }

    pub fn find(self, path: &str) -> Result<Option<BootfsEntry<'a>>, BootfsError> {
        for index in 0..self.count {
            let entry = self.entry(index)?;
            if entry.path == path {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    pub fn entry(self, index: u32) -> Result<BootfsEntry<'a>, BootfsError> {
        if index >= self.count {
            return Err(BootfsError::EntryOutOfBounds);
        }
        let offset = HEADER_SIZE + index as usize * ENTRY_SIZE;
        let path_bytes = &self.bytes[offset..offset + PATH_SIZE];
        let path_len = path_bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(BootfsError::BadPath)?;
        let path =
            core::str::from_utf8(&path_bytes[..path_len]).map_err(|_| BootfsError::BadPath)?;
        let payload_offset = read_u64(self.bytes, offset + PATH_SIZE)? as usize;
        let payload_size = read_u64(self.bytes, offset + PATH_SIZE + 8)? as usize;
        let payload_end = payload_offset
            .checked_add(payload_size)
            .ok_or(BootfsError::PayloadOutOfBounds)?;
        if payload_offset < self.payload_offset as usize || payload_end > self.bytes.len() {
            return Err(BootfsError::PayloadOutOfBounds);
        }
        Ok(BootfsEntry {
            path,
            bytes: &self.bytes[payload_offset..payload_end],
            payload_offset: payload_offset as u64,
        })
    }

    pub const fn table_size(self) -> u32 {
        self.table_size
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, BootfsError> {
    let end = offset.checked_add(4).ok_or(BootfsError::TableOutOfBounds)?;
    let raw = bytes
        .get(offset..end)
        .ok_or(BootfsError::TableOutOfBounds)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, BootfsError> {
    let end = offset.checked_add(8).ok_or(BootfsError::EntryOutOfBounds)?;
    let raw = bytes
        .get(offset..end)
        .ok_or(BootfsError::EntryOutOfBounds)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}
