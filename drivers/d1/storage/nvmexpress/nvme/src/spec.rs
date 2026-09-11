pub const NVME_PCI_VENDOR_ANY: u16 = 0xffff;
pub const ADMIN_QUEUE_ID: u16 = 0;
pub const DEFAULT_BLOCK_SIZE: u32 = 512;
pub const DEFAULT_MAX_TRANSFER_BLOCKS: u32 = 1024;

pub const REG_CAP: usize = 0x0000;
pub const REG_CC: usize = 0x0014;
pub const REG_CSTS: usize = 0x001c;
pub const REG_AQA: usize = 0x0024;
pub const REG_ASQ: usize = 0x0028;
pub const REG_ACQ: usize = 0x0030;

pub const CC_ENABLE: u32 = 1;
pub const CSTS_READY: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NvmeStatus {
    Success,
    InvalidOpcode,
    InvalidField,
    DataTransferError,
    Aborted,
    Unknown(u16),
}

impl NvmeStatus {
    pub const fn from_completion(status_field: u16) -> Self {
        match (status_field >> 1) & 0x7ff {
            0x0 => Self::Success,
            0x1 => Self::InvalidOpcode,
            0x2 => Self::InvalidField,
            0x4 => Self::DataTransferError,
            0x7 => Self::Aborted,
            code => Self::Unknown(code),
        }
    }
}
