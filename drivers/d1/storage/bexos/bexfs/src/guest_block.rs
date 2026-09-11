mod migration;
use crate::block::{BlockFifoBackend, FidlBlockDevice};
use alloc::rc::Rc;
use alloc::vec::Vec;
use bexos_userspace::{Channel, Memory, Rpc};
use block_fidl::*;
use rosefs_core::block::{BlockDevice, BlockIoError};

const TRANSFER_BYTES: u64 = 512 * 1024;
pub struct RpcBlock {
    control: Rpc,
    fifo: Channel,
    handle: u64,
    va: u64,
    id: u32,
    info: BlockInfo,
    owned: bool,
}
impl RpcBlock {
    pub fn connect(channel: Channel) -> Self {
        let control = Rpc(channel);
        let info = call(&control, 1, &BlockDeviceGetInfoRequest {});
        let mut info = BlockDeviceGetInfoResponse::decode(&info.0, &info.1)
            .unwrap()
            .info;
        info.max_transfer_blocks = info
            .max_transfer_blocks
            .min((TRANSFER_BYTES as u32) / info.block_size);
        let fifo = call(&control, 4, &BlockDeviceGetFifoRequest {});
        let fifo = BlockDeviceGetFifoResponse::decode(&fifo.0, &fifo.1).unwrap();
        assert_eq!(fifo.status, Status::Ok);
        let fifo = Channel(fifo.fifo_handle.raw);
        let handle = Memory::create(TRANSFER_BYTES, 0).unwrap();
        let va = Memory::map(handle, TRANSFER_BYTES, 6).unwrap();
        Memory::commit_range(va, TRANSFER_BYTES).unwrap();
        let shared = Memory::duplicate(handle, 1 | 2 | 4 | 16 | 32).unwrap();
        let response = call(
            &control,
            2,
            &BlockDeviceRegisterBufferRequest {
                vmo: HandleRef { raw: shared },
            },
        );
        let response = BlockDeviceRegisterBufferResponse::decode(&response.0, &response.1).unwrap();
        assert_eq!(response.status, Status::Ok);
        Self {
            control,
            fifo,
            handle,
            va,
            id: response.vmo_id,
            info,
            owned: true,
        }
    }

    fn reconnect_data_plane(&mut self) -> Result<(), Status> {
        let fifo = call(&self.control, 4, &BlockDeviceGetFifoRequest {});
        let fifo = BlockDeviceGetFifoResponse::decode(&fifo.0, &fifo.1)
            .map_err(|_| Status::ErrInvalidArgs)?;
        if fifo.status != Status::Ok {
            return Err(fifo.status);
        }
        let shared = Memory::duplicate(self.handle, 1 | 2 | 4 | 16 | 32)
            .map_err(|_| Status::ErrInvalidHandle)?;
        let response = call(
            &self.control,
            2,
            &BlockDeviceRegisterBufferRequest {
                vmo: HandleRef { raw: shared },
            },
        );
        let response = BlockDeviceRegisterBufferResponse::decode(&response.0, &response.1)
            .map_err(|_| Status::ErrInvalidArgs)?;
        if response.status != Status::Ok {
            return Err(response.status);
        }
        self.fifo = Channel(fifo.fifo_handle.raw);
        self.id = response.vmo_id;
        Ok(())
    }
}
impl BlockFifoBackend for RpcBlock {
    fn info(&self) -> Result<BlockInfo, Status> {
        Ok(self.info)
    }
    fn register_buffer(&mut self, bytes: usize) -> Result<u32, Status> {
        if bytes > TRANSFER_BYTES as usize {
            Err(Status::ErrBufferTooSmall)
        } else {
            Ok(self.id)
        }
    }
    fn unregister_buffer(&mut self, _id: u32) -> Result<(), Status> {
        Ok(())
    }
    fn transfer(
        &mut self,
        request: BlockRequest,
        buffer: &mut [u8],
        write: bool,
    ) -> Result<BlockResponse, Status> {
        if buffer.len() > TRANSFER_BYTES as usize {
            return Err(Status::ErrBufferTooSmall);
        }
        if write {
            unsafe {
                core::ptr::copy_nonoverlapping(buffer.as_ptr(), self.va as *mut u8, buffer.len());
            }
        }
        let mut bytes = [0; 64];
        let e = request
            .encode(&mut bytes, &mut [])
            .map_err(|_| Status::ErrInvalidArgs)?;
        if self.fifo.send(&bytes[..e.bytes], &[]).is_err() {
            self.reconnect_data_plane()?;
            self.fifo
                .send(&bytes[..e.bytes], &[])
                .map_err(|_| Status::ErrPeerClosed)?;
        }
        // A diskimage flush synchronizes its backing BexFS namespace. Match the
        // filesystem storage allowance instead of the generic 60-second IPC
        // deadline, which can expire during encrypted-user commits under TCG.
        let m = self.fifo.recv_with_timeout(300).map_err(|error| {
            bexos_userspace::log(&alloc::format!(
                "bexfs: block request={} opcode={:?} receive failed {error:?}\n",
                request.req_id,
                request.opcode
            ));
            Status::ErrTimedOut
        })?;
        let response = BlockResponse::decode(&m.bytes, &[]).map_err(|_| Status::ErrInvalidArgs)?;
        if response.status != Status::Ok {
            bexos_userspace::log(&alloc::format!(
                "bexfs: block request={} opcode={:?} lba={} blocks={} failed {:?}\n",
                request.req_id,
                request.opcode,
                request.device_block_offset,
                request.block_count,
                response.status
            ));
        }
        if !write && request.opcode == BlockOpcode::Read && response.status == Status::Ok {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.va as *const u8,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                );
            }
        }
        Ok(response)
    }
}
impl Drop for RpcBlock {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let _ = call(
            &self.control,
            3,
            &BlockDeviceUnregisterBufferRequest { vmo_id: self.id },
        );
        let _ = Memory::unmap(self.va, TRANSFER_BYTES);
        let _ = Memory::close(self.handle);
    }
}
fn call<Q: FidlEncode>(rpc: &Rpc, ordinal: u64, q: &Q) -> (Vec<u8>, Vec<HandleRef>) {
    let mut bytes = [0; 256];
    let mut hs = [HandleRef { raw: 0 }; 4];
    let e = q.encode(&mut bytes, &mut hs).unwrap();
    let m = rpc
        .call_raw(
            ordinal,
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
            true,
        )
        .unwrap();
    (
        m.bytes,
        m.handles.iter().map(|h| HandleRef { raw: *h }).collect(),
    )
}
#[derive(Clone)]
pub struct SharedBlock(pub Rc<FidlBlockDevice<RpcBlock>>);
impl BlockDevice for SharedBlock {
    fn num_blocks(&self) -> u64 {
        self.0.num_blocks()
    }
    fn block_size(&self) -> u32 {
        self.0.block_size()
    }
    fn read_at(&self, lba: u64, b: &mut [u8]) -> Result<(), BlockIoError> {
        self.0.read_at(lba, b)
    }
    fn write_at(&self, lba: u64, b: &[u8]) -> Result<(), BlockIoError> {
        self.0.write_at(lba, b)
    }
    fn flush(&self) -> Result<(), BlockIoError> {
        self.0.flush()
    }
}
pub struct Partition {
    pub disk: SharedBlock,
    pub first: u64,
    pub sectors: u64,
}
impl BlockDevice for Partition {
    fn num_blocks(&self) -> u64 {
        self.sectors / 8
    }
    fn block_size(&self) -> u32 {
        4096
    }
    fn read_at(&self, lba: u64, b: &mut [u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, b.len())?;
        self.disk.read_at(self.first + lba * 8, b)
    }
    fn write_at(&self, lba: u64, b: &[u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, b.len())?;
        self.disk.write_at(self.first + lba * 8, b)
    }
    fn flush(&self) -> Result<(), BlockIoError> {
        self.disk.flush()
    }
}

pub enum VolumeDevice {
    Partition(Partition),
    Raw4k(SharedBlock),
}

impl From<Partition> for VolumeDevice {
    fn from(value: Partition) -> Self {
        Self::Partition(value)
    }
}

impl VolumeDevice {
    pub fn resources(&self) -> Vec<bexos_userspace::live_migration::Resource> {
        match self {
            Self::Partition(partition) => partition.disk.resources(),
            Self::Raw4k(disk) => disk.resources(),
        }
    }
}

impl BlockDevice for VolumeDevice {
    fn num_blocks(&self) -> u64 {
        match self {
            Self::Partition(partition) => partition.num_blocks(),
            Self::Raw4k(disk) => disk.num_blocks(),
        }
    }

    fn block_size(&self) -> u32 {
        4096
    }

    fn read_at(&self, lba: u64, b: &mut [u8]) -> Result<(), BlockIoError> {
        match self {
            Self::Partition(partition) => partition.read_at(lba, b),
            Self::Raw4k(disk) => disk.read_at(lba, b),
        }
    }

    fn write_at(&self, lba: u64, b: &[u8]) -> Result<(), BlockIoError> {
        match self {
            Self::Partition(partition) => partition.write_at(lba, b),
            Self::Raw4k(disk) => disk.write_at(lba, b),
        }
    }

    fn flush(&self) -> Result<(), BlockIoError> {
        match self {
            Self::Partition(partition) => partition.flush(),
            Self::Raw4k(disk) => disk.flush(),
        }
    }
}
