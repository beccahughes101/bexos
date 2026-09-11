#![no_std]

//! Authenticated replacement images. This format is independent of normal
//! world update authorization: the execution owner verifies the saved root.
pub mod selection;
pub mod staging;
pub mod store;
use bexos_avb::{verify_partition, verify_vbmeta};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"BEXFW001";
pub const HEADER_BYTES: usize = 32;
pub const MAX_METADATA_BYTES: usize = 64 * 1024;
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_BUNDLE_BYTES: usize = HEADER_BYTES + MAX_METADATA_BYTES + MAX_IMAGE_BYTES;
pub const STATE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Architecture {
    Aarch64,
    X86_64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Component {
    Trusty,
    Hypervisor,
}

impl Component {
    pub const fn rollback_location(self) -> u32 {
        match self {
            Self::Trusty => 30,
            Self::Hypervisor => 31,
        }
    }
    pub const fn descriptor(self, arch: Architecture) -> Option<&'static [u8]> {
        match (self, arch) {
            (Self::Trusty, Architecture::Aarch64) => Some(b"trusty/aarch64/state/1"),
            (Self::Trusty, Architecture::X86_64) => Some(b"trusty/x86_64/state/1"),
            (Self::Hypervisor, Architecture::X86_64) => Some(b"hypervisor/x86_64/state/1"),
            (Self::Hypervisor, Architecture::Aarch64) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Bounds,
    Format,
    Authentication,
    Policy,
    Image,
    Owner,
    Order,
    Busy,
    Deadline,
}

#[derive(Debug)]
pub struct Verified<'a> {
    pub image: &'a [u8],
    pub generation: u64,
    pub digest: [u8; 32],
}

/// The returned image borrows the owner's immutable snapshot. A successful
/// check neither executes it nor advances any persistent generation floor.
pub fn verify<'a>(
    bundle: &'a [u8],
    root: &[u8],
    arch: Architecture,
    component: Component,
    active_generation: u64,
    durable_floor: u64,
) -> Result<Verified<'a>, Error> {
    let name = component.descriptor(arch).ok_or(Error::Policy)?;
    if bundle.len() < HEADER_BYTES || bundle.len() > MAX_BUNDLE_BYTES || root.len() != 520 {
        return Err(Error::Bounds);
    }
    if &bundle[..8] != MAGIC || bundle[24..32] != [0; 8] {
        return Err(Error::Format);
    }
    let metadata_len = usize::try_from(u64::from_le_bytes(bundle[8..16].try_into().unwrap()))
        .map_err(|_| Error::Bounds)?;
    let image_len = usize::try_from(u64::from_le_bytes(bundle[16..24].try_into().unwrap()))
        .map_err(|_| Error::Bounds)?;
    if !(256..=MAX_METADATA_BYTES).contains(&metadata_len)
        || !(64..=MAX_IMAGE_BYTES).contains(&image_len)
        || HEADER_BYTES
            .checked_add(metadata_len)
            .and_then(|n| n.checked_add(image_len))
            != Some(bundle.len())
    {
        return Err(Error::Bounds);
    }
    let metadata = &bundle[HEADER_BYTES..HEADER_BYTES + metadata_len];
    let auth = u64::from_be_bytes(metadata[12..20].try_into().unwrap());
    let aux = u64::from_be_bytes(metadata[20..28].try_into().unwrap());
    if auth.checked_add(aux).and_then(|n| n.checked_add(256)) != Some(metadata_len as u64) {
        return Err(Error::Format);
    }
    let verified = verify_vbmeta(metadata, &root[8..264]).map_err(|_| Error::Authentication)?;
    if verified.flags != 0
        || verified.rollback_index <= active_generation
        || verified.rollback_index < durable_floor
        || verified.rollback_index_location != component.rollback_location()
    {
        return Err(Error::Policy);
    }
    let descriptor = verified.hash_descriptor(name).map_err(|_| Error::Policy)?;
    if descriptor.flags != 0 || descriptor.image_size != image_len as u64 {
        return Err(Error::Image);
    }
    let image = &bundle[HEADER_BYTES + metadata_len..];
    verify_partition(descriptor, image).map_err(|_| Error::Authentication)?;
    let machine = match arch {
        Architecture::Aarch64 => 183u16,
        Architecture::X86_64 => 62,
    };
    if &image[..7] != b"\x7fELF\x02\x01\x01"
        || image[16..18] != 2u16.to_le_bytes()
        || image[18..20] != machine.to_le_bytes()
    {
        return Err(Error::Image);
    }
    Ok(Verified {
        image,
        generation: verified.rollback_index,
        digest: Sha256::digest(image).into(),
    })
}
