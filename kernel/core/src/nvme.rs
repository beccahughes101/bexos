pub const NVME_CLASS_CODE: u8 = 0x01;
pub const NVME_SUBCLASS: u8 = 0x08;
pub const NVME_PROG_IF: u8 = 0x02;

pub const REG_CAP: usize = 0x0000;
pub const REG_CC: usize = 0x0014;
pub const REG_CSTS: usize = 0x001c;
pub const REG_AQA: usize = 0x0024;
pub const REG_ASQ: usize = 0x0028;
pub const REG_ACQ: usize = 0x0030;

pub const ADMIN_IDENTIFY: u8 = 0x06;
pub const IO_WRITE: u8 = 0x01;
pub const IO_READ: u8 = 0x02;
pub const ADMIN_CREATE_IO_CQ: u8 = 0x05;
pub const ADMIN_CREATE_IO_SQ: u8 = 0x01;

pub const CNS_IDENTIFY_NAMESPACE: u32 = 0x00;
pub const CNS_IDENTIFY_CONTROLLER: u32 = 0x01;

pub const CSTS_READY: u32 = 1;
pub const CC_ENABLE: u32 = 1;
pub const CC_IOCQES_16: u32 = 4 << 20;
pub const CC_IOSQES_64: u32 = 6 << 16;

pub const fn queue_doorbell_stride(cap: u64) -> usize {
    4usize << ((cap >> 32) & 0xf)
}

pub const fn queue_entries_minus_one(depth: u16) -> u32 {
    depth as u32 - 1
}

pub const fn admin_queue_attrs(depth: u16) -> u32 {
    let entries = queue_entries_minus_one(depth);
    entries | (entries << 16)
}

pub fn namespace_block_count(identify_namespace: &[u8]) -> Option<u64> {
    let bytes = identify_namespace.get(0..8)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

pub fn namespace_block_size(identify_namespace: &[u8]) -> Option<u32> {
    let flbas = *identify_namespace.get(26)? & 0xf;
    let lbaf_offset = 128 + flbas as usize * 4;
    let lbads = *identify_namespace.get(lbaf_offset + 2)?;
    if lbads >= 32 {
        None
    } else {
        Some(1u32 << lbads)
    }
}
