#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespaceInfo {
    pub namespace_id: u32,
    pub block_size: u32,
    pub block_count: u64,
    pub max_transfer_blocks: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentifyError {
    BufferTooSmall,
    InvalidLbaFormat,
    Overflow,
}

pub fn parse_identify_namespace(
    namespace_id: u32,
    data: &[u8],
    max_transfer_blocks: u32,
) -> Result<NamespaceInfo, IdentifyError> {
    if data.len() < 132 {
        return Err(IdentifyError::BufferTooSmall);
    }

    let block_count = u64::from_le_bytes(data[0..8].try_into().expect("slice len"));
    let flbas = data[26] & 0x0f;
    let lbaf_offset = 128 + flbas as usize * 4;
    if lbaf_offset + 4 > data.len() {
        return Err(IdentifyError::InvalidLbaFormat);
    }
    let lbads = data[lbaf_offset + 2];
    if lbads > 31 {
        return Err(IdentifyError::Overflow);
    }

    Ok(NamespaceInfo {
        namespace_id,
        block_size: 1u32
            .checked_shl(lbads as u32)
            .ok_or(IdentifyError::Overflow)?,
        block_count,
        max_transfer_blocks,
    })
}
