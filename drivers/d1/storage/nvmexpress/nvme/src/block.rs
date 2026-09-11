mod migration;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use block_fidl::{BlockFlags, BlockInfo, BlockOpcode, BlockRequest, BlockResponse, Status};

use crate::namespace::NamespaceInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisteredBuffer {
    pub handle: u64,
    pub blocks: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockBackendOp {
    Read {
        req_id: u64,
        vmo_id: u32,
        vmo_offset_blocks: u64,
        device_block_offset: u64,
        block_count: u32,
    },
    Write {
        req_id: u64,
        vmo_id: u32,
        vmo_offset_blocks: u64,
        device_block_offset: u64,
        block_count: u32,
    },
    Flush {
        req_id: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockDeviceServer {
    info: BlockInfo,
    next_vmo_id: u32,
    buffers: BTreeMap<u32, RegisteredBuffer>,
    dispatched: Vec<BlockBackendOp>,
}

impl BlockDeviceServer {
    pub fn new(namespace: NamespaceInfo) -> Self {
        Self {
            info: BlockInfo {
                block_size: namespace.block_size,
                block_count: namespace.block_count,
                max_transfer_blocks: namespace.max_transfer_blocks,
                flags: BlockFlags(0),
            },
            next_vmo_id: 1,
            buffers: BTreeMap::new(),
            dispatched: Vec::new(),
        }
    }

    pub const fn info(&self) -> BlockInfo {
        self.info
    }

    pub fn register_buffer(&mut self, handle: u64, blocks: u64) -> (Status, u32) {
        if handle == 0 || blocks == 0 {
            return (Status::ErrInvalidHandle, 0);
        }
        let id = self.next_vmo_id;
        self.next_vmo_id = self.next_vmo_id.saturating_add(1);
        self.buffers.insert(id, RegisteredBuffer { handle, blocks });
        (Status::Ok, id)
    }

    pub fn unregister_buffer(&mut self, vmo_id: u32) -> Status {
        if self.buffers.remove(&vmo_id).is_some() {
            Status::Ok
        } else {
            Status::ErrInvalidHandle
        }
    }

    pub fn dispatch(&mut self, request: BlockRequest) -> BlockResponse {
        let status = self
            .validate_request(request)
            .unwrap_or_else(|status| status);
        if status != Status::Ok {
            return BlockResponse {
                req_id: request.req_id,
                status,
            };
        }

        match request.opcode {
            BlockOpcode::Read => self.dispatched.push(BlockBackendOp::Read {
                req_id: request.req_id,
                vmo_id: request.vmo_id,
                vmo_offset_blocks: request.vmo_offset_blocks,
                device_block_offset: request.device_block_offset,
                block_count: request.block_count,
            }),
            BlockOpcode::Write => self.dispatched.push(BlockBackendOp::Write {
                req_id: request.req_id,
                vmo_id: request.vmo_id,
                vmo_offset_blocks: request.vmo_offset_blocks,
                device_block_offset: request.device_block_offset,
                block_count: request.block_count,
            }),
            BlockOpcode::Flush => self.dispatched.push(BlockBackendOp::Flush {
                req_id: request.req_id,
            }),
            BlockOpcode::Trim => {
                return BlockResponse {
                    req_id: request.req_id,
                    status: Status::ErrInvalidArgs,
                };
            }
        }

        BlockResponse {
            req_id: request.req_id,
            status: Status::Ok,
        }
    }

    pub fn dispatched(&self) -> &[BlockBackendOp] {
        &self.dispatched
    }

    fn validate_request(&self, request: BlockRequest) -> Result<Status, Status> {
        if request.opcode == BlockOpcode::Trim {
            return Err(Status::ErrInvalidArgs);
        }
        if request.opcode == BlockOpcode::Flush {
            return Ok(Status::Ok);
        }
        if request.block_count == 0 || request.block_count > self.info.max_transfer_blocks {
            return Err(Status::ErrInvalidArgs);
        }
        let buffer = self
            .buffers
            .get(&request.vmo_id)
            .ok_or(Status::ErrInvalidHandle)?;
        let buffer_end = request
            .vmo_offset_blocks
            .checked_add(request.block_count as u64)
            .ok_or(Status::ErrInvalidArgs)?;
        if buffer_end > buffer.blocks {
            return Err(Status::ErrBufferTooSmall);
        }
        let device_end = request
            .device_block_offset
            .checked_add(request.block_count as u64)
            .ok_or(Status::ErrInvalidArgs)?;
        if device_end > self.info.block_count {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(Status::Ok)
    }
}
