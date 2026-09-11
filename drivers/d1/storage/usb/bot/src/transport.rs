use bexos_usb_host::bot::{
    CommandBlockWrapper, scsi_read_capacity10, scsi_read10, scsi_synchronize_cache10, scsi_write10,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SenseData {
    pub key: u8,
    pub asc: u8,
    pub ascq: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Stall,
    Reset,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportState {
    pub lun: u8,
    pub block_size: u32,
    pub block_count: u64,
    pub tag: u32,
    pub last_sense: SenseData,
    pub bulk_in_stalled: bool,
    pub bulk_out_stalled: bool,
    pub reset_count: u64,
    pub completed_write_watermark: u64,
}

impl Default for TransportState {
    fn default() -> Self {
        Self {
            lun: 0,
            block_size: 512,
            block_count: 0,
            tag: 1,
            last_sense: SenseData::default(),
            bulk_in_stalled: false,
            bulk_out_stalled: false,
            reset_count: 0,
            completed_write_watermark: 0,
        }
    }
}

impl TransportState {
    pub fn read_capacity_cbw(&mut self) -> CommandBlockWrapper {
        let mut cb = [0; 16];
        let cb_len = scsi_read_capacity10(&mut cb);
        self.cbw(8, true, cb_len, cb)
    }

    pub fn read_cbw(&mut self, lba: u32, blocks: u16, bytes: u32) -> CommandBlockWrapper {
        let mut cb = [0; 16];
        let cb_len = scsi_read10(&mut cb, lba, blocks);
        self.cbw(bytes, true, cb_len, cb)
    }

    pub fn write_cbw(&mut self, lba: u32, blocks: u16, bytes: u32) -> CommandBlockWrapper {
        let mut cb = [0; 16];
        let cb_len = scsi_write10(&mut cb, lba, blocks);
        self.cbw(bytes, false, cb_len, cb)
    }

    pub fn flush_cbw(&mut self) -> CommandBlockWrapper {
        let mut cb = [0; 16];
        let cb_len = scsi_synchronize_cache10(&mut cb);
        self.cbw(0, false, cb_len, cb)
    }

    pub fn reset_recovery(&mut self) {
        self.bulk_in_stalled = false;
        self.bulk_out_stalled = false;
        self.reset_count = self.reset_count.wrapping_add(1);
        self.last_sense = SenseData::default();
    }

    fn cbw(&mut self, bytes: u32, input: bool, cb_len: u8, cb: [u8; 16]) -> CommandBlockWrapper {
        let tag = self.tag;
        self.tag = self.tag.wrapping_add(1).max(1);
        CommandBlockWrapper {
            tag,
            data_transfer_length: bytes,
            flags: if input { 0x80 } else { 0 },
            lun: self.lun,
            cb_len,
            cb,
        }
    }
}
