use crate::*;

pub fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or(ElfError::ProgramHeadersOutOfBounds)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

pub fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or(ElfError::ProgramHeadersOutOfBounds)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

pub fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or(ElfError::ProgramHeadersOutOfBounds)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

pub fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, ElfError> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or(ElfError::ProgramHeadersOutOfBounds)?;
    Ok(i64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

pub fn dynamic_segment_rights(flags: u32) -> u32 {
    let mut rights = 0;
    if flags & 0x4 != 0 {
        rights |= RIGHTS_READ;
    }
    if flags & 0x2 != 0 {
        rights |= RIGHTS_WRITE;
    }
    if flags & 0x1 != 0 {
        rights |= RIGHTS_EXECUTE;
    }
    rights
}
pub fn page_round(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|n| n & !(PAGE_SIZE - 1))
}

pub fn align_down(value: u64) -> u64 {
    value & !(PAGE_SIZE - 1)
}

pub fn align_up_to(value: u64, align: u64) -> Option<u64> {
    if align <= 1 {
        return Some(value);
    }
    value.checked_add(align - 1).map(|n| n & !(align - 1))
}
