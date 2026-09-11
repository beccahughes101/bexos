pub const CBW_SIGNATURE: u32 = 0x4342_5355;
pub const CSW_SIGNATURE: u32 = 0x5342_5355;
pub const CBW_LEN: usize = 31;
pub const CSW_LEN: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotError {
    BadLength,
    BadSignature,
    BadFlags,
    BadStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommandBlockWrapper {
    pub tag: u32,
    pub data_transfer_length: u32,
    pub flags: u8,
    pub lun: u8,
    pub cb_len: u8,
    pub cb: [u8; 16],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommandStatusWrapper {
    pub tag: u32,
    pub residue: u32,
    pub status: u8,
}

impl CommandBlockWrapper {
    pub fn encode(self, out: &mut [u8]) -> Result<(), BotError> {
        let bytes = out.get_mut(..CBW_LEN).ok_or(BotError::BadLength)?;
        bytes[0..4].copy_from_slice(&CBW_SIGNATURE.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.tag.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.data_transfer_length.to_le_bytes());
        bytes[12] = self.flags;
        bytes[13] = self.lun;
        bytes[14] = self.cb_len;
        bytes[15..31].copy_from_slice(&self.cb);
        Ok(())
    }
}

impl CommandStatusWrapper {
    pub fn decode(bytes: &[u8], expected_tag: u32) -> Result<Self, BotError> {
        if bytes.len() < CSW_LEN {
            return Err(BotError::BadLength);
        }
        if u32::from_le_bytes(bytes[0..4].try_into().unwrap()) != CSW_SIGNATURE {
            return Err(BotError::BadSignature);
        }
        let tag = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if tag != expected_tag {
            return Err(BotError::BadStatus);
        }
        let status = bytes[12];
        if status > 2 {
            return Err(BotError::BadStatus);
        }
        Ok(Self {
            tag,
            residue: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            status,
        })
    }
}

pub fn scsi_read_capacity10(cb: &mut [u8; 16]) -> u8 {
    *cb = [0; 16];
    cb[0] = 0x25;
    10
}

pub fn scsi_read10(cb: &mut [u8; 16], lba: u32, blocks: u16) -> u8 {
    *cb = [0; 16];
    cb[0] = 0x28;
    cb[2..6].copy_from_slice(&lba.to_be_bytes());
    cb[7..9].copy_from_slice(&blocks.to_be_bytes());
    10
}

pub fn scsi_write10(cb: &mut [u8; 16], lba: u32, blocks: u16) -> u8 {
    *cb = [0; 16];
    cb[0] = 0x2a;
    cb[2..6].copy_from_slice(&lba.to_be_bytes());
    cb[7..9].copy_from_slice(&blocks.to_be_bytes());
    10
}

pub fn scsi_synchronize_cache10(cb: &mut [u8; 16]) -> u8 {
    *cb = [0; 16];
    cb[0] = 0x35;
    10
}
