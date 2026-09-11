use alloc::collections::BTreeMap;

use block_fidl::{BlockFlags, BlockInfo, BlockOpcode, BlockRequest, BlockResponse, Status};

use crate::crypto::{EncryptedImage, EncryptedImageInfo};

pub const DISKIMAGE_BLOCK_SIZE: u32 = 4096;
pub const DISKIMAGE_MAX_TRANSFER_BLOCKS: u32 = 1;

pub trait BackingFile {
    fn len(&self) -> Result<u64, Status>;
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status>;
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), Status>;
    fn sync(&mut self) -> Result<(), Status>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisteredBuffer {
    pub handle: u64,
    pub blocks: u64,
}

pub struct DiskImageServer<F> {
    image: Image<F>,
    info: BlockInfo,
    next_vmo_id: u32,
    buffers: BTreeMap<u32, RegisteredBuffer>,
}

impl<F: BackingFile> DiskImageServer<F> {
    pub fn attach(file: F, read_only: bool) -> Result<Self, Status> {
        let bytes = file.len()?;
        if bytes == 0 || bytes % u64::from(DISKIMAGE_BLOCK_SIZE) != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(Self {
            image: Image::Raw(file),
            info: BlockInfo {
                block_size: DISKIMAGE_BLOCK_SIZE,
                block_count: bytes / u64::from(DISKIMAGE_BLOCK_SIZE),
                max_transfer_blocks: DISKIMAGE_MAX_TRANSFER_BLOCKS,
                flags: if read_only {
                    BlockFlags::READ_ONLY
                } else {
                    BlockFlags(0)
                },
            },
            next_vmo_id: 1,
            buffers: BTreeMap::new(),
        })
    }

    pub fn create_encrypted(
        file: F,
        key: &[u8; 32],
        image_uuid: [u8; 16],
        virtual_size_bytes: u64,
    ) -> Result<Self, Status> {
        let image = EncryptedImage::create(file, key, image_uuid, virtual_size_bytes)?;
        Ok(Self::from_encrypted(image, false))
    }

    pub fn open_encrypted(file: F, key: &[u8; 32], read_only: bool) -> Result<Self, Status> {
        let image = EncryptedImage::open(file, key)?;
        Ok(Self::from_encrypted(image, read_only))
    }

    fn from_encrypted(image: EncryptedImage<F>, read_only: bool) -> Self {
        let block_count = image.block_count();
        Self {
            image: Image::Encrypted(image),
            info: BlockInfo {
                block_size: DISKIMAGE_BLOCK_SIZE,
                block_count,
                max_transfer_blocks: DISKIMAGE_MAX_TRANSFER_BLOCKS,
                flags: if read_only {
                    BlockFlags::READ_ONLY
                } else {
                    BlockFlags(0)
                },
            },
            next_vmo_id: 1,
            buffers: BTreeMap::new(),
        }
    }

    pub const fn info(&self) -> BlockInfo {
        self.info
    }

    pub fn file(&self) -> &F {
        self.image.file()
    }

    pub fn file_mut(&mut self) -> &mut F {
        self.image.file_mut()
    }

    pub fn encrypted_info(&self) -> Option<EncryptedImageInfo> {
        self.image.encrypted_info()
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

    pub fn next_vmo_id(&self) -> u32 {
        self.next_vmo_id
    }

    pub fn restore_registrations(
        &mut self,
        next_vmo_id: u32,
        buffers: BTreeMap<u32, RegisteredBuffer>,
    ) {
        self.next_vmo_id = next_vmo_id.max(1);
        self.buffers = buffers;
    }

    pub fn registrations_match(&self, mapped: &BTreeMap<u32, (u64, u64, u64)>) -> bool {
        self.buffers.len() == mapped.len()
            && self.buffers.iter().all(|(id, buffer)| {
                mapped.get(id).is_some_and(|(handle, _, blocks)| {
                    *handle == buffer.handle && *blocks == buffer.blocks
                })
            })
    }

    pub fn dispatch(&mut self, request: BlockRequest, buffer: &mut [u8]) -> BlockResponse {
        let status = self.transfer(request, buffer).err().unwrap_or(Status::Ok);
        BlockResponse {
            req_id: request.req_id,
            status,
        }
    }

    fn transfer(&mut self, request: BlockRequest, buffer: &mut [u8]) -> Result<(), Status> {
        match request.opcode {
            BlockOpcode::Read | BlockOpcode::Write => self.transfer_blocks(request, buffer),
            BlockOpcode::Flush => {
                if request.vmo_id != 0
                    || request.vmo_offset_blocks != 0
                    || request.device_block_offset != 0
                    || request.block_count != 0
                {
                    return Err(Status::ErrInvalidArgs);
                }
                self.image.sync()
            }
            BlockOpcode::Trim => Err(Status::ErrInvalidArgs),
        }
    }

    fn transfer_blocks(&mut self, request: BlockRequest, buffer: &mut [u8]) -> Result<(), Status> {
        if request.opcode == BlockOpcode::Write && self.info.flags.0 & BlockFlags::READ_ONLY.0 != 0
        {
            return Err(Status::ErrAccessDenied);
        }
        if request.block_count == 0 || request.block_count > self.info.max_transfer_blocks {
            return Err(Status::ErrInvalidArgs);
        }
        let registered = self
            .buffers
            .get(&request.vmo_id)
            .ok_or(Status::ErrInvalidHandle)?;
        let vmo_end = request
            .vmo_offset_blocks
            .checked_add(u64::from(request.block_count))
            .ok_or(Status::ErrInvalidArgs)?;
        if vmo_end > registered.blocks {
            return Err(Status::ErrBufferTooSmall);
        }
        let device_end = request
            .device_block_offset
            .checked_add(u64::from(request.block_count))
            .ok_or(Status::ErrInvalidArgs)?;
        if device_end > self.info.block_count {
            return Err(Status::ErrInvalidArgs);
        }
        let offset = request
            .device_block_offset
            .checked_mul(u64::from(DISKIMAGE_BLOCK_SIZE))
            .ok_or(Status::ErrInvalidArgs)?;
        let byte_offset = request
            .vmo_offset_blocks
            .checked_mul(u64::from(DISKIMAGE_BLOCK_SIZE))
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or(Status::ErrInvalidArgs)?;
        let byte_count =
            usize::try_from(u64::from(request.block_count) * u64::from(DISKIMAGE_BLOCK_SIZE))
                .map_err(|_| Status::ErrInvalidArgs)?;
        let byte_end = byte_offset
            .checked_add(byte_count)
            .ok_or(Status::ErrInvalidArgs)?;
        if byte_end > buffer.len() {
            return Err(Status::ErrBufferTooSmall);
        }
        let window = &mut buffer[byte_offset..byte_end];
        if request.opcode == BlockOpcode::Read {
            self.image
                .read_at(request.device_block_offset, offset, window)
        } else {
            self.image
                .write_at(request.device_block_offset, offset, window)
        }
    }
}

enum Image<F> {
    Raw(F),
    Encrypted(EncryptedImage<F>),
}

impl<F: BackingFile> Image<F> {
    fn file(&self) -> &F {
        match self {
            Self::Raw(file) => file,
            Self::Encrypted(image) => image.file(),
        }
    }

    fn file_mut(&mut self) -> &mut F {
        match self {
            Self::Raw(file) => file,
            Self::Encrypted(image) => image.file_mut(),
        }
    }

    fn encrypted_info(&self) -> Option<EncryptedImageInfo> {
        match self {
            Self::Raw(_) => None,
            Self::Encrypted(image) => Some(image.info()),
        }
    }

    fn read_at(&mut self, sector: u64, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        match self {
            Self::Raw(file) => file.read_at(offset, out),
            Self::Encrypted(image) => image.read_sector(sector, out),
        }
    }

    fn write_at(&mut self, sector: u64, offset: u64, data: &[u8]) -> Result<(), Status> {
        match self {
            Self::Raw(file) => file.write_at(offset, data),
            Self::Encrypted(image) => image.write_sector(sector, data),
        }
    }

    fn sync(&mut self) -> Result<(), Status> {
        match self {
            Self::Raw(file) => file.sync(),
            Self::Encrypted(image) => image.sync(),
        }
    }
}
