mod migration;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use block_fidl::{BlockInfo, BlockOpcode, BlockRequest, BlockResponse, Status};
use rosefs_core::block::{BlockDevice, BlockIoError};

pub const BEXFS_BLOCK_SIZE: u32 = 4096;

pub trait BlockFifoBackend {
    fn info(&self) -> Result<BlockInfo, Status>;
    fn register_buffer(&mut self, bytes: usize) -> Result<u32, Status>;
    fn transfer(
        &mut self,
        request: BlockRequest,
        buffer: &mut [u8],
        write: bool,
    ) -> Result<BlockResponse, Status>;
    fn unregister_buffer(&mut self, vmo_id: u32) -> Result<(), Status>;
}

pub struct FidlBlockDevice<B> {
    backend: RefCell<B>,
    info: BlockInfo,
    next_request_id: RefCell<u64>,
}

impl<B: BlockFifoBackend> FidlBlockDevice<B> {
    pub fn connect(backend: B) -> Result<Self, Status> {
        let info = backend.info()?;
        if info.block_size == 0 || info.block_count == 0 || info.max_transfer_blocks == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(Self {
            backend: RefCell::new(backend),
            info,
            next_request_id: RefCell::new(1),
        })
    }

    fn execute(
        &self,
        opcode: BlockOpcode,
        lba: u64,
        buffer: &mut [u8],
    ) -> Result<(), BlockIoError> {
        self.check_request(lba, buffer.len())?;
        let block_count = (buffer.len() / self.info.block_size as usize) as u32;
        if block_count > self.info.max_transfer_blocks {
            let chunk_bytes =
                self.info.max_transfer_blocks as usize * self.info.block_size as usize;
            for (index, chunk) in buffer.chunks_mut(chunk_bytes).enumerate() {
                self.execute(
                    opcode,
                    lba + index as u64 * u64::from(self.info.max_transfer_blocks),
                    chunk,
                )?;
            }
            return Ok(());
        }
        let mut backend = self.backend.borrow_mut();
        let vmo_id = backend.register_buffer(buffer.len()).map_err(map_status)?;
        let req_id = *self.next_request_id.borrow();
        *self.next_request_id.borrow_mut() = req_id.saturating_add(1);
        let request = BlockRequest {
            req_id,
            opcode,
            vmo_id,
            vmo_offset_blocks: 0,
            device_block_offset: lba,
            block_count,
        };
        let result = backend
            .transfer(request, buffer, opcode == BlockOpcode::Write)
            .map_err(map_status);
        let unregister = backend.unregister_buffer(vmo_id).map_err(map_status);
        let response = result?;
        unregister?;
        if response.req_id != req_id || response.status != Status::Ok {
            return Err(map_status(response.status));
        }
        Ok(())
    }
}

impl<B: BlockFifoBackend> BlockDevice for FidlBlockDevice<B> {
    fn num_blocks(&self) -> u64 {
        self.info.block_count
    }
    fn block_size(&self) -> u32 {
        self.info.block_size
    }
    fn read_at(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockIoError> {
        self.execute(BlockOpcode::Read, lba, buf)
    }
    fn write_at(&self, lba: u64, buf: &[u8]) -> Result<(), BlockIoError> {
        let mut staging = buf.to_vec();
        self.execute(BlockOpcode::Write, lba, &mut staging)
    }
    fn flush(&self) -> Result<(), BlockIoError> {
        let mut backend = self.backend.borrow_mut();
        let req_id = *self.next_request_id.borrow();
        *self.next_request_id.borrow_mut() = req_id.saturating_add(1);
        let response = backend
            .transfer(
                BlockRequest {
                    req_id,
                    opcode: BlockOpcode::Flush,
                    vmo_id: 0,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 0,
                },
                &mut [],
                false,
            )
            .map_err(map_status)?;
        if response.req_id == req_id && response.status == Status::Ok {
            Ok(())
        } else {
            Err(map_status(response.status))
        }
    }
}

fn map_status(status: Status) -> BlockIoError {
    match status {
        Status::ErrBufferTooSmall | Status::ErrInvalidArgs => BlockIoError::BadGeometry,
        _ => BlockIoError::Other,
    }
}

pub struct SectorPartitionDevice<'a> {
    inner: &'a dyn BlockDevice,
    first_sector: u64,
    sector_count: u64,
}

impl<'a> SectorPartitionDevice<'a> {
    pub fn new(
        inner: &'a dyn BlockDevice,
        first_sector: u64,
        sector_count: u64,
    ) -> Result<Self, BlockIoError> {
        if inner.block_size() != 512
            || !first_sector.is_multiple_of(8)
            || !sector_count.is_multiple_of(8)
            || first_sector
                .checked_add(sector_count)
                .is_none_or(|end| end > inner.num_blocks())
        {
            return Err(BlockIoError::BadGeometry);
        }
        Ok(Self {
            inner,
            first_sector,
            sector_count,
        })
    }
}

impl BlockDevice for SectorPartitionDevice<'_> {
    fn num_blocks(&self) -> u64 {
        self.sector_count / 8
    }

    fn block_size(&self) -> u32 {
        BEXFS_BLOCK_SIZE
    }

    fn read_at(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, buf.len())?;
        self.inner.read_at(self.first_sector + lba * 8, buf)
    }

    fn write_at(&self, lba: u64, buf: &[u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, buf.len())?;
        self.inner.write_at(self.first_sector + lba * 8, buf)
    }

    fn flush(&self) -> Result<(), BlockIoError> {
        self.inner.flush()
    }
}

pub struct MemoryBlockDevice {
    block_size: u32,
    bytes: RefCell<Vec<u8>>,
    flushes: RefCell<u64>,
}

impl MemoryBlockDevice {
    pub fn new(block_size: u32, blocks: u64) -> Self {
        Self {
            block_size,
            bytes: RefCell::new(vec![0; (u64::from(block_size) * blocks) as usize]),
            flushes: RefCell::new(0),
        }
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.bytes.borrow().clone()
    }

    pub fn overwrite(&self, offset: usize, bytes: &[u8]) {
        self.bytes.borrow_mut()[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    pub fn flush_count(&self) -> u64 {
        *self.flushes.borrow()
    }
}

impl BlockDevice for MemoryBlockDevice {
    fn num_blocks(&self) -> u64 {
        self.bytes.borrow().len() as u64 / u64::from(self.block_size)
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn read_at(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, buf.len())?;
        let offset = (lba * u64::from(self.block_size)) as usize;
        buf.copy_from_slice(&self.bytes.borrow()[offset..offset + buf.len()]);
        Ok(())
    }

    fn write_at(&self, lba: u64, buf: &[u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, buf.len())?;
        let offset = (lba * u64::from(self.block_size)) as usize;
        self.bytes.borrow_mut()[offset..offset + buf.len()].copy_from_slice(buf);
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockIoError> {
        *self.flushes.borrow_mut() += 1;
        Ok(())
    }
}
