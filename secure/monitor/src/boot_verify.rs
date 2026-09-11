//! Authenticate the exact payloads which the monitor will load. A signed
//! descriptor of a prefix never authorizes an appended executable or BootFS.
use bexos_avb::{verify_partition, verify_vbmeta};
use bexos_bootfs_parser::Bootfs;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Metadata,
    Policy,
    Kernel,
    Bootfs,
    Bounds,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Verified {
    pub generation: u64,
    pub rollback_location: u32,
    pub kernel_sha256: [u8; 32],
    pub bootfs_sha256: [u8; 32],
    pub policy_sha256: [u8; 32],
    pub root_sha256: [u8; 32],
}
pub fn verify(
    metadata: &[u8],
    root: &[u8],
    kernel: &[u8],
    bootfs: &[u8],
) -> Result<Verified, Error> {
    if metadata.len() < bexos_avb::HEADER_BYTES
        || metadata.len() > 64 * 1024
        || root.len() != 520
        || kernel.is_empty()
        || kernel.len() > 64 * 1024 * 1024
        || bootfs.is_empty()
        || bootfs.len() > 128 * 1024 * 1024
    {
        return Err(Error::Bounds);
    }
    let auth = u64::from_be_bytes(metadata[12..20].try_into().unwrap());
    let auxiliary = u64::from_be_bytes(metadata[20..28].try_into().unwrap());
    if auth
        .checked_add(auxiliary)
        .and_then(|length| length.checked_add(bexos_avb::HEADER_BYTES as u64))
        != Some(metadata.len() as u64)
    {
        return Err(Error::Metadata);
    }
    let verified = verify_vbmeta(metadata, &root[8..264]).map_err(|_| Error::Metadata)?;
    if verified.flags != 0 || verified.rollback_index == 0 || verified.rollback_index_location > 31
    {
        return Err(Error::Policy);
    }
    for (name, image, error) in [
        (b"kernel".as_slice(), kernel, Error::Kernel),
        (b"bootfs".as_slice(), bootfs, Error::Bootfs),
    ] {
        let descriptor = verified.hash_descriptor(name).map_err(|_| error)?;
        if descriptor.flags != 0 || descriptor.image_size != image.len() as u64 {
            return Err(error);
        }
        verify_partition(descriptor, image).map_err(|_| error)?;
    }
    let policy = Bootfs::parse(bootfs)
        .map_err(|_| Error::Bootfs)?
        .find("/boot/platform.pcfg")
        .map_err(|_| Error::Policy)?
        .ok_or(Error::Policy)?
        .bytes;
    let descriptor = verified
        .hash_descriptor(b"platform-policy")
        .map_err(|_| Error::Policy)?;
    if descriptor.flags != 0 || descriptor.image_size != policy.len() as u64 {
        return Err(Error::Policy);
    }
    verify_partition(descriptor, policy).map_err(|_| Error::Policy)?;
    Ok(Verified {
        generation: verified.rollback_index,
        rollback_location: verified.rollback_index_location,
        kernel_sha256: Sha256::digest(kernel).into(),
        bootfs_sha256: Sha256::digest(bootfs).into(),
        policy_sha256: Sha256::digest(policy).into(),
        root_sha256: Sha256::digest(&root[8..264]).into(),
    })
}
