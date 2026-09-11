use alloc::vec;
use alloc::vec::Vec;

use aes::Aes256;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use block_fidl::Status;

use crate::block::{BackingFile, DISKIMAGE_BLOCK_SIZE};

const HEADER_MAGIC: [u8; 8] = *b"BEXDI002";
const HEADER_VERSION: u32 = 2;
const HEADER_SIZE: usize = DISKIMAGE_BLOCK_SIZE as usize;
const SECTOR_TAG_SIZE: usize = 32;
const SECTOR_RECORD_SIZE: u64 = DISKIMAGE_BLOCK_SIZE as u64 + SECTOR_TAG_SIZE as u64;
const CONTEXT_DISKIMAGE_XTS_A: &str = "bexos.diskimage.v2.xts.a";
const CONTEXT_DISKIMAGE_XTS_B: &str = "bexos.diskimage.v2.xts.b";
const CONTEXT_DISKIMAGE_TAG: &str = "bexos.diskimage.v2.tag";
const CONTEXT_DISKIMAGE_HEADER: &str = "bexos.diskimage.v2.header";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncryptedImageInfo {
    pub image_uuid: [u8; 16],
    pub virtual_size_bytes: u64,
}

pub struct EncryptedImage<F> {
    file: F,
    keys: ImageKeys,
    image_uuid: [u8; 16],
    virtual_size_bytes: u64,
    block_count: u64,
    bitmap_offset: u64,
    data_offset: u64,
    bitmap: Vec<u8>,
    dirty_bitmap: bool,
}

impl<F: BackingFile> EncryptedImage<F> {
    pub fn create(
        mut file: F,
        ukek: &[u8; 32],
        image_uuid: [u8; 16],
        virtual_size_bytes: u64,
    ) -> Result<Self, Status> {
        if virtual_size_bytes == 0
            || !virtual_size_bytes.is_multiple_of(u64::from(DISKIMAGE_BLOCK_SIZE))
        {
            return Err(Status::ErrInvalidArgs);
        }
        let block_count = virtual_size_bytes / u64::from(DISKIMAGE_BLOCK_SIZE);
        let bitmap_len = block_count.div_ceil(8);
        let bitmap_offset = HEADER_SIZE as u64;
        let data_offset = align_up(
            bitmap_offset
                .checked_add(bitmap_len)
                .ok_or(Status::ErrInvalidArgs)?,
            u64::from(DISKIMAGE_BLOCK_SIZE),
        )?;
        let keys = ImageKeys::derive(ukek, &image_uuid);
        let bitmap = vec![0u8; bitmap_len as usize];
        let header = Header {
            image_uuid,
            virtual_size_bytes,
            block_count,
            bitmap_offset,
            bitmap_len,
            data_offset,
        };
        let encoded = encode_header(&header, &keys.header_key);
        file.write_at(0, &encoded)?;
        if !bitmap.is_empty() {
            file.write_at(bitmap_offset, &bitmap)?;
        }
        file.sync()?;
        Ok(Self {
            file,
            keys,
            image_uuid,
            virtual_size_bytes,
            block_count,
            bitmap_offset,
            data_offset,
            bitmap,
            dirty_bitmap: false,
        })
    }

    pub fn open(mut file: F, ukek: &[u8; 32]) -> Result<Self, Status> {
        let mut encoded = vec![0u8; HEADER_SIZE];
        file.read_at(0, &mut encoded)?;
        let header = decode_header_without_mac(&encoded)?;
        let keys = ImageKeys::derive(ukek, &header.image_uuid);
        if encode_header(&header, &keys.header_key) != encoded {
            return Err(Status::ErrAccessDenied);
        }
        if header.bitmap_len > usize::MAX as u64 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut bitmap = vec![0u8; header.bitmap_len as usize];
        if !bitmap.is_empty() {
            file.read_at(header.bitmap_offset, &mut bitmap)?;
        }
        Ok(Self {
            file,
            keys,
            image_uuid: header.image_uuid,
            virtual_size_bytes: header.virtual_size_bytes,
            block_count: header.block_count,
            bitmap_offset: header.bitmap_offset,
            data_offset: header.data_offset,
            bitmap,
            dirty_bitmap: false,
        })
    }

    pub const fn info(&self) -> EncryptedImageInfo {
        EncryptedImageInfo {
            image_uuid: self.image_uuid,
            virtual_size_bytes: self.virtual_size_bytes,
        }
    }

    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    pub fn file(&self) -> &F {
        &self.file
    }

    pub fn file_mut(&mut self) -> &mut F {
        &mut self.file
    }

    pub fn read_sector(&mut self, sector: u64, out: &mut [u8]) -> Result<(), Status> {
        if out.len() != DISKIMAGE_BLOCK_SIZE as usize || sector >= self.block_count {
            return Err(Status::ErrInvalidArgs);
        }
        if !self.is_allocated(sector)? {
            out.fill(0);
            return Ok(());
        }
        let mut stored = vec![0u8; SECTOR_RECORD_SIZE as usize];
        self.file
            .read_at(self.sector_record_offset(sector)?, &mut stored)?;
        let (ciphertext, tag) = stored.split_at(DISKIMAGE_BLOCK_SIZE as usize);
        if sector_tag(&self.keys.tag_key, &self.image_uuid, sector, ciphertext) != tag {
            return Err(Status::ErrAccessDenied);
        }
        out.copy_from_slice(ciphertext);
        xts_decrypt(
            &self.keys.xts_a,
            &self.keys.xts_b,
            &self.image_uuid,
            sector,
            out,
        );
        Ok(())
    }

    pub fn write_sector(&mut self, sector: u64, data: &[u8]) -> Result<(), Status> {
        if data.len() != DISKIMAGE_BLOCK_SIZE as usize || sector >= self.block_count {
            return Err(Status::ErrInvalidArgs);
        }
        let mut ciphertext = data.to_vec();
        xts_encrypt(
            &self.keys.xts_a,
            &self.keys.xts_b,
            &self.image_uuid,
            sector,
            &mut ciphertext,
        );
        let tag = sector_tag(&self.keys.tag_key, &self.image_uuid, sector, &ciphertext);
        let mut stored = Vec::with_capacity(SECTOR_RECORD_SIZE as usize);
        stored.extend_from_slice(&ciphertext);
        stored.extend_from_slice(&tag);
        self.file
            .write_at(self.sector_record_offset(sector)?, &stored)?;
        self.set_allocated(sector)?;
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), Status> {
        if self.dirty_bitmap {
            self.file.write_at(self.bitmap_offset, &self.bitmap)?;
            self.dirty_bitmap = false;
        }
        self.file.sync()
    }

    fn is_allocated(&self, sector: u64) -> Result<bool, Status> {
        let byte = usize::try_from(sector / 8).map_err(|_| Status::ErrInvalidArgs)?;
        let bit = (sector % 8) as u8;
        Ok(self
            .bitmap
            .get(byte)
            .is_some_and(|value| value & (1u8 << bit) != 0))
    }

    fn set_allocated(&mut self, sector: u64) -> Result<(), Status> {
        let byte = usize::try_from(sector / 8).map_err(|_| Status::ErrInvalidArgs)?;
        let bit = (sector % 8) as u8;
        let value = self.bitmap.get_mut(byte).ok_or(Status::ErrInvalidArgs)?;
        *value |= 1u8 << bit;
        self.dirty_bitmap = true;
        Ok(())
    }

    fn sector_record_offset(&self, sector: u64) -> Result<u64, Status> {
        self.data_offset
            .checked_add(
                sector
                    .checked_mul(SECTOR_RECORD_SIZE)
                    .ok_or(Status::ErrInvalidArgs)?,
            )
            .ok_or(Status::ErrInvalidArgs)
    }
}

impl<F> Drop for EncryptedImage<F> {
    fn drop(&mut self) {
        self.keys.zeroize();
        self.bitmap.fill(0);
    }
}

#[derive(Clone)]
struct ImageKeys {
    xts_a: [u8; 32],
    xts_b: [u8; 32],
    tag_key: [u8; 32],
    header_key: [u8; 32],
}

impl ImageKeys {
    fn derive(ukek: &[u8; 32], image_uuid: &[u8; 16]) -> Self {
        Self {
            xts_a: derive_key(CONTEXT_DISKIMAGE_XTS_A, ukek, image_uuid),
            xts_b: derive_key(CONTEXT_DISKIMAGE_XTS_B, ukek, image_uuid),
            tag_key: derive_key(CONTEXT_DISKIMAGE_TAG, ukek, image_uuid),
            header_key: derive_key(CONTEXT_DISKIMAGE_HEADER, ukek, image_uuid),
        }
    }

    fn zeroize(&mut self) {
        self.xts_a.fill(0);
        self.xts_b.fill(0);
        self.tag_key.fill(0);
        self.header_key.fill(0);
    }
}

#[derive(Clone, Copy)]
struct Header {
    image_uuid: [u8; 16],
    virtual_size_bytes: u64,
    block_count: u64,
    bitmap_offset: u64,
    bitmap_len: u64,
    data_offset: u64,
}

fn encode_header(header: &Header, header_key: &[u8; 32]) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_SIZE];
    out[..8].copy_from_slice(&HEADER_MAGIC);
    out[8..12].copy_from_slice(&HEADER_VERSION.to_le_bytes());
    out[16..32].copy_from_slice(&header.image_uuid);
    out[32..40].copy_from_slice(&header.virtual_size_bytes.to_le_bytes());
    out[40..48].copy_from_slice(&header.block_count.to_le_bytes());
    out[48..56].copy_from_slice(&header.bitmap_offset.to_le_bytes());
    out[56..64].copy_from_slice(&header.bitmap_len.to_le_bytes());
    out[64..72].copy_from_slice(&header.data_offset.to_le_bytes());
    let mac = keyed_hash(header_key, &out[..HEADER_SIZE - 32]);
    out[HEADER_SIZE - 32..].copy_from_slice(&mac);
    out
}

fn decode_header_without_mac(bytes: &[u8]) -> Result<Header, Status> {
    if bytes.len() != HEADER_SIZE || bytes.get(..8) != Some(&HEADER_MAGIC) {
        return Err(Status::ErrInvalidArgs);
    }
    if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != HEADER_VERSION {
        return Err(Status::ErrInvalidArgs);
    }
    let image_uuid = bytes[16..32].try_into().unwrap();
    let virtual_size_bytes = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
    let block_count = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
    let bitmap_offset = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
    let bitmap_len = u64::from_le_bytes(bytes[56..64].try_into().unwrap());
    let data_offset = u64::from_le_bytes(bytes[64..72].try_into().unwrap());
    if block_count == 0
        || block_count != virtual_size_bytes / u64::from(DISKIMAGE_BLOCK_SIZE)
        || !virtual_size_bytes.is_multiple_of(u64::from(DISKIMAGE_BLOCK_SIZE))
        || bitmap_offset != HEADER_SIZE as u64
        || bitmap_len != block_count.div_ceil(8)
        || data_offset < bitmap_offset.saturating_add(bitmap_len)
        || !data_offset.is_multiple_of(u64::from(DISKIMAGE_BLOCK_SIZE))
    {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(Header {
        image_uuid,
        virtual_size_bytes,
        block_count,
        bitmap_offset,
        bitmap_len,
        data_offset,
    })
}

fn derive_key(context: &str, ukek: &[u8; 32], image_uuid: &[u8; 16]) -> [u8; 32] {
    let mut input = Vec::with_capacity(48);
    input.extend_from_slice(ukek);
    input.extend_from_slice(image_uuid);
    blake3::derive_key(context, &input)
}

fn sector_tag(key: &[u8; 32], image_uuid: &[u8; 16], sector: u64, ciphertext: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(image_uuid);
    hasher.update(&sector.to_le_bytes());
    hasher.update(ciphertext);
    *hasher.finalize().as_bytes()
}

fn keyed_hash(key: &[u8; 32], bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}

fn align_up(value: u64, align: u64) -> Result<u64, Status> {
    value
        .checked_add(align - 1)
        .map(|value| value / align * align)
        .ok_or(Status::ErrInvalidArgs)
}

fn xts_encrypt(
    data_key: &[u8; 32],
    tweak_key: &[u8; 32],
    image_uuid: &[u8; 16],
    sector: u64,
    block: &mut [u8],
) {
    let data_cipher = Aes256::new(GenericArray::from_slice(data_key));
    let tweak_cipher = Aes256::new(GenericArray::from_slice(tweak_key));
    let mut tweak = GenericArray::clone_from_slice(&sector_tweak(image_uuid, sector));
    tweak_cipher.encrypt_block(&mut tweak);
    let mut tweak_bytes = [0u8; 16];
    tweak_bytes.copy_from_slice(&tweak);
    for chunk in block.chunks_exact_mut(16) {
        for i in 0..16 {
            chunk[i] ^= tweak_bytes[i];
        }
        let mut aes_block = GenericArray::clone_from_slice(chunk);
        data_cipher.encrypt_block(&mut aes_block);
        chunk.copy_from_slice(&aes_block);
        for i in 0..16 {
            chunk[i] ^= tweak_bytes[i];
        }
        multiply_alpha(&mut tweak_bytes);
    }
}

fn xts_decrypt(
    data_key: &[u8; 32],
    tweak_key: &[u8; 32],
    image_uuid: &[u8; 16],
    sector: u64,
    block: &mut [u8],
) {
    let data_cipher = Aes256::new(GenericArray::from_slice(data_key));
    let tweak_cipher = Aes256::new(GenericArray::from_slice(tweak_key));
    let mut tweak = GenericArray::clone_from_slice(&sector_tweak(image_uuid, sector));
    tweak_cipher.encrypt_block(&mut tweak);
    let mut tweak_bytes = [0u8; 16];
    tweak_bytes.copy_from_slice(&tweak);
    for chunk in block.chunks_exact_mut(16) {
        for i in 0..16 {
            chunk[i] ^= tweak_bytes[i];
        }
        let mut aes_block = GenericArray::clone_from_slice(chunk);
        data_cipher.decrypt_block(&mut aes_block);
        chunk.copy_from_slice(&aes_block);
        for i in 0..16 {
            chunk[i] ^= tweak_bytes[i];
        }
        multiply_alpha(&mut tweak_bytes);
    }
}

fn sector_tweak(image_uuid: &[u8; 16], sector: u64) -> [u8; 16] {
    let mut tweak = *image_uuid;
    for (dst, src) in tweak[..8].iter_mut().zip(sector.to_le_bytes()) {
        *dst ^= src;
    }
    tweak
}

fn multiply_alpha(tweak: &mut [u8; 16]) {
    let mut carry = 0u8;
    for byte in tweak.iter_mut() {
        let next = *byte >> 7;
        *byte = (*byte << 1) | carry;
        carry = next;
    }
    if carry != 0 {
        tweak[0] ^= 0x87;
    }
}
