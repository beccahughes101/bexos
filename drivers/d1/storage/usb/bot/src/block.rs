use alloc::collections::BTreeMap;
use block_fidl::{BlockFlags, BlockInfo, BlockOpcode, BlockRequest, BlockResponse, Status};

pub const REGISTERED_BUFFER_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    pub handle: u64,
    pub mapped: u64,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct BlockServer {
    pub next_buffer_id: u32,
    pub buffers: BTreeMap<u32, Buffer>,
    pub in_flight: u32,
}

impl BlockServer {
    pub fn new() -> Self {
        Self {
            next_buffer_id: 1,
            buffers: BTreeMap::new(),
            in_flight: 0,
        }
    }

    pub fn info(block_size: u32, block_count: u64) -> BlockInfo {
        BlockInfo {
            block_size,
            block_count,
            max_transfer_blocks: (REGISTERED_BUFFER_BYTES / u64::from(block_size)) as u32,
            flags: BlockFlags(BlockFlags::REMOVABLE.0),
        }
    }

    pub fn validate(&self, request: &BlockRequest, info: BlockInfo) -> BlockResponse {
        let mut status = Status::Ok;
        if !matches!(
            request.opcode,
            BlockOpcode::Read | BlockOpcode::Write | BlockOpcode::Flush
        ) {
            status = Status::ErrInvalidArgs;
        } else if request.opcode != BlockOpcode::Flush {
            if request.block_count == 0
                || request.block_count > info.max_transfer_blocks
                || !self.buffers.contains_key(&request.vmo_id)
                || request
                    .device_block_offset
                    .checked_add(u64::from(request.block_count))
                    .is_none_or(|end| end > info.block_count)
            {
                status = Status::ErrInvalidArgs;
            }
        }
        BlockResponse {
            req_id: request.req_id,
            status,
        }
    }
}
