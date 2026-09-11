extern crate alloc;

use alloc::{string::String, vec::Vec};
use bexos_crypto::{blake3_256, verify_ed25519};
use tee_manager_fidl::TeeStatus;

pub const BUNDLE_MAGIC: &[u8; 8] = b"BEXTEEAB";
pub const BUNDLE_VERSION: u32 = 2;
pub const ABI_VERSION: u32 = 1;
const SIGNATURE_LEN: usize = 64;
const QEMU_TRUSTY_UPDATE_PUBLIC_KEY: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeeUpdateBundle {
    pub generation: u64,
    pub target: String,
    pub abi_version: u32,
    pub metadata_hash: [u8; 32],
    pub signer: String,
    pub slot_a_hash: [u8; 32],
    pub slot_a_load_base: u64,
    pub slot_a_load_size: u64,
    pub slot_a: Vec<u8>,
    pub slot_b_hash: [u8; 32],
    pub slot_b_load_base: u64,
    pub slot_b_load_size: u64,
    pub slot_b: Vec<u8>,
}

impl TeeUpdateBundle {
    pub fn parse(bytes: &[u8], expected_hash: &[u8]) -> Result<Self, TeeStatus> {
        if expected_hash.len() != 32 {
            return Err(TeeStatus::ErrInvalidArgs);
        }
        if blake3_256(bytes).as_slice() != expected_hash {
            return Err(TeeStatus::ErrVerifyFailed);
        }
        if bytes.len()
            < 8 + 4 + 4 + 8 + 2 + 32 + 2 + 32 + 8 + 8 + 8 + 32 + 8 + 8 + 8 + SIGNATURE_LEN
            || &bytes[..8] != BUNDLE_MAGIC
        {
            return Err(TeeStatus::ErrVerifyFailed);
        }
        let mut offset = 8;
        let version = take_u32(bytes, &mut offset)?;
        if version != BUNDLE_VERSION {
            return Err(TeeStatus::ErrVerifyFailed);
        }
        let abi_version = take_u32(bytes, &mut offset)?;
        if abi_version != ABI_VERSION {
            return Err(TeeStatus::ErrVerifyFailed);
        }
        let generation = take_u64(bytes, &mut offset)?;
        let target_len = take_u16(bytes, &mut offset)? as usize;
        let target_bytes = take(bytes, &mut offset, target_len)?;
        let target = core::str::from_utf8(target_bytes)
            .map_err(|_| TeeStatus::ErrVerifyFailed)?
            .to_string();
        let metadata_hash: [u8; 32] = take(bytes, &mut offset, 32)?
            .try_into()
            .map_err(|_| TeeStatus::ErrVerifyFailed)?;
        let signer_len = take_u16(bytes, &mut offset)? as usize;
        let signer_bytes = take(bytes, &mut offset, signer_len)?;
        let signer = core::str::from_utf8(signer_bytes)
            .map_err(|_| TeeStatus::ErrVerifyFailed)?
            .to_string();
        let slot_a_hash: [u8; 32] = take(bytes, &mut offset, 32)?
            .try_into()
            .map_err(|_| TeeStatus::ErrVerifyFailed)?;
        let slot_a_load_base = take_u64(bytes, &mut offset)?;
        let slot_a_load_size = take_u64(bytes, &mut offset)?;
        let slot_a_len = take_u64(bytes, &mut offset)? as usize;
        let slot_b_hash: [u8; 32] = take(bytes, &mut offset, 32)?
            .try_into()
            .map_err(|_| TeeStatus::ErrVerifyFailed)?;
        let slot_b_load_base = take_u64(bytes, &mut offset)?;
        let slot_b_load_size = take_u64(bytes, &mut offset)?;
        let slot_b_len = take_u64(bytes, &mut offset)? as usize;
        let slot_a = take(bytes, &mut offset, slot_a_len)?.to_vec();
        let slot_b = take(bytes, &mut offset, slot_b_len)?.to_vec();
        let signature_offset = offset;
        let signature: [u8; SIGNATURE_LEN] = take(bytes, &mut offset, SIGNATURE_LEN)?
            .try_into()
            .map_err(|_| TeeStatus::ErrVerifyFailed)?;
        if offset != bytes.len()
            || target.is_empty()
            || signer.is_empty()
            || slot_a.is_empty()
            || slot_b.is_empty()
            || slot_a_hash != blake3_256(&slot_a)
            || slot_b_hash != blake3_256(&slot_b)
            || slot_a_load_size != slot_a.len() as u64
            || slot_b_load_size != slot_b.len() as u64
            || slot_a_load_base == slot_b_load_base
        {
            return Err(TeeStatus::ErrVerifyFailed);
        }
        verify_ed25519(
            &QEMU_TRUSTY_UPDATE_PUBLIC_KEY,
            &bytes[..signature_offset],
            &signature,
        )
        .map_err(|_| TeeStatus::ErrVerifyFailed)?;
        Ok(Self {
            generation,
            target,
            abi_version,
            metadata_hash,
            signer,
            slot_a_hash,
            slot_a_load_base,
            slot_a_load_size,
            slot_a,
            slot_b_hash,
            slot_b_load_base,
            slot_b_load_size,
            slot_b,
        })
    }
}

fn take<'a>(bytes: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], TeeStatus> {
    let end = offset.checked_add(len).ok_or(TeeStatus::ErrVerifyFailed)?;
    let out = bytes.get(*offset..end).ok_or(TeeStatus::ErrVerifyFailed)?;
    *offset = end;
    Ok(out)
}

fn take_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, TeeStatus> {
    Ok(u16::from_le_bytes(
        take(bytes, offset, 2)?
            .try_into()
            .map_err(|_| TeeStatus::ErrVerifyFailed)?,
    ))
}

fn take_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, TeeStatus> {
    Ok(u32::from_le_bytes(
        take(bytes, offset, 4)?
            .try_into()
            .map_err(|_| TeeStatus::ErrVerifyFailed)?,
    ))
}

fn take_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, TeeStatus> {
    Ok(u64::from_le_bytes(
        take(bytes, offset, 8)?
            .try_into()
            .map_err(|_| TeeStatus::ErrVerifyFailed)?,
    ))
}
