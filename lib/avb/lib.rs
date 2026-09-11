#![cfg_attr(not(test), no_std)]

use sha2::{Digest, Sha256};

pub const HEADER_BYTES: usize = 256;
pub const ALGORITHM_SHA256_RSA2048: u32 = 1;
pub const HASH_DESCRIPTOR_TAG: u64 = 2;
const HASH_DESCRIPTOR_FIXED_BYTES: usize = 132;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Bounds,
    Header,
    UnsupportedAlgorithm,
    UntrustedKey,
    Digest,
    Signature,
    Descriptor,
    MissingPartition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedVbmeta<'a> {
    pub rollback_index: u64,
    pub rollback_index_location: u32,
    pub flags: u32,
    descriptors: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HashDescriptor<'a> {
    pub image_size: u64,
    pub partition_name: &'a [u8],
    pub salt: &'a [u8],
    pub digest: &'a [u8],
    pub flags: u32,
}

impl<'a> VerifiedVbmeta<'a> {
    pub fn hash_descriptor(&self, partition: &[u8]) -> Result<HashDescriptor<'a>, Error> {
        let mut remaining = self.descriptors;
        while !remaining.is_empty() {
            if remaining.len() < 16 {
                return Err(Error::Descriptor);
            }
            let tag = be_u64(remaining, 0)?;
            let following = usize::try_from(be_u64(remaining, 8)?).map_err(|_| Error::Bounds)?;
            let total = 16usize.checked_add(following).ok_or(Error::Bounds)?;
            if total > remaining.len() {
                return Err(Error::Bounds);
            }
            if tag == HASH_DESCRIPTOR_TAG {
                let descriptor = parse_hash_descriptor(&remaining[..total])?;
                if descriptor.partition_name == partition {
                    return Ok(descriptor);
                }
            }
            remaining = &remaining[total..];
        }
        Err(Error::MissingPartition)
    }
}

pub fn verify_vbmeta<'a>(
    bytes: &'a [u8],
    trusted_modulus: &[u8],
) -> Result<VerifiedVbmeta<'a>, Error> {
    if bytes.len() < HEADER_BYTES || &bytes[..4] != b"AVB0" || be_u32(bytes, 4)? > 1 {
        return Err(Error::Header);
    }
    if be_u32(bytes, 28)? != ALGORITHM_SHA256_RSA2048 {
        return Err(Error::UnsupportedAlgorithm);
    }
    let auth_len = to_usize(be_u64(bytes, 12)?)?;
    let aux_len = to_usize(be_u64(bytes, 20)?)?;
    let auth_end = HEADER_BYTES.checked_add(auth_len).ok_or(Error::Bounds)?;
    let image_end = auth_end.checked_add(aux_len).ok_or(Error::Bounds)?;
    let auth = bytes.get(HEADER_BYTES..auth_end).ok_or(Error::Bounds)?;
    let aux = bytes.get(auth_end..image_end).ok_or(Error::Bounds)?;

    let hash = field(auth, be_u64(bytes, 32)?, be_u64(bytes, 40)?)?;
    let signature = field(auth, be_u64(bytes, 48)?, be_u64(bytes, 56)?)?;
    if hash.len() != 32 || signature.len() != 256 {
        return Err(Error::Header);
    }
    let public_key = field(aux, be_u64(bytes, 64)?, be_u64(bytes, 72)?)?;
    if public_key.len() != 520 || public_key.get(8..264) != Some(trusted_modulus) {
        return Err(Error::UntrustedKey);
    }
    let descriptors = field(aux, be_u64(bytes, 96)?, be_u64(bytes, 104)?)?;

    let mut signed = Sha256::new();
    signed.update(&bytes[..HEADER_BYTES]);
    signed.update(aux);
    let calculated = signed.finalize();
    if calculated.as_slice() != hash {
        return Err(Error::Digest);
    }
    verify_rsa2048_sha256(&public_key[8..264], signature, &calculated)?;

    Ok(VerifiedVbmeta {
        rollback_index: be_u64(bytes, 112)?,
        flags: be_u32(bytes, 120)?,
        rollback_index_location: be_u32(bytes, 124)?,
        descriptors,
    })
}

pub fn verify_partition(descriptor: HashDescriptor<'_>, image: &[u8]) -> Result<(), Error> {
    let image_size = usize::try_from(descriptor.image_size).map_err(|_| Error::Bounds)?;
    let image = image.get(..image_size).ok_or(Error::Bounds)?;
    let mut hasher = Sha256::new();
    hasher.update(descriptor.salt);
    hasher.update(image);
    if hasher.finalize().as_slice() == descriptor.digest {
        Ok(())
    } else {
        Err(Error::Digest)
    }
}

fn verify_rsa2048_sha256(modulus: &[u8], signature: &[u8], digest: &[u8]) -> Result<(), Error> {
    if modulus.len() != 256 || signature.len() != 256 || digest.len() != 32 {
        return Err(Error::Signature);
    }
    let modulus = bigint(modulus)?;
    let signature = bigint(signature)?;
    if compare(&signature, &modulus) != core::cmp::Ordering::Less {
        return Err(Error::Signature);
    }
    let mut value = [0u32; 64];
    value[0] = 1;
    let mut base = signature;
    for bit in 0..17 {
        if bit == 0 || bit == 16 {
            value = multiply_mod(&value, &base, &modulus);
        }
        if bit != 16 {
            base = multiply_mod(&base, &base, &modulus);
        }
    }
    let mut encoded = [0u8; 256];
    for (index, word) in value.iter().enumerate() {
        encoded[252 - index * 4..256 - index * 4].copy_from_slice(&word.to_be_bytes());
    }
    const PREFIX: &[u8] = &[
        0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
        0x05, 0x00, 0x04, 0x20,
    ];
    let separator = 256 - PREFIX.len() - digest.len() - 1;
    if encoded[..2] != [0, 1]
        || encoded[2..separator].iter().any(|byte| *byte != 0xff)
        || encoded[separator] != 0
        || &encoded[separator + 1..separator + 1 + PREFIX.len()] != PREFIX
        || &encoded[256 - digest.len()..] != digest
    {
        return Err(Error::Signature);
    }
    Ok(())
}

fn bigint(bytes: &[u8]) -> Result<[u32; 64], Error> {
    if bytes.len() != 256 {
        return Err(Error::Signature);
    }
    let mut value = [0u32; 64];
    for (index, chunk) in bytes.rchunks_exact(4).enumerate() {
        value[index] = u32::from_be_bytes(chunk.try_into().map_err(|_| Error::Signature)?);
    }
    Ok(value)
}

fn multiply_mod(a: &[u32; 64], b: &[u32; 64], modulus: &[u32; 64]) -> [u32; 64] {
    let mut result = [0u32; 64];
    let mut addend = *a;
    for bit in 0..2048 {
        if b[bit / 32] & (1u32 << (bit % 32)) != 0 {
            result = add_mod(&result, &addend, modulus);
        }
        addend = add_mod(&addend, &addend, modulus);
    }
    result
}

fn add_mod(a: &[u32; 64], b: &[u32; 64], modulus: &[u32; 64]) -> [u32; 64] {
    let mut sum = [0u32; 64];
    let mut carry = 0u64;
    for index in 0..64 {
        let next = u64::from(a[index]) + u64::from(b[index]) + carry;
        sum[index] = next as u32;
        carry = next >> 32;
    }
    if carry != 0 || compare(&sum, modulus) != core::cmp::Ordering::Less {
        subtract(&sum, modulus)
    } else {
        sum
    }
}

fn subtract(a: &[u32; 64], b: &[u32; 64]) -> [u32; 64] {
    let mut result = [0u32; 64];
    let mut borrow = 0u64;
    for index in 0..64 {
        let subtrahend = u64::from(b[index]) + borrow;
        result[index] = u64::from(a[index]).wrapping_sub(subtrahend) as u32;
        borrow = if u64::from(a[index]) < subtrahend {
            1
        } else {
            0
        };
    }
    result
}

fn compare(a: &[u32; 64], b: &[u32; 64]) -> core::cmp::Ordering {
    for index in (0..64).rev() {
        match a[index].cmp(&b[index]) {
            core::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    core::cmp::Ordering::Equal
}

fn parse_hash_descriptor(bytes: &[u8]) -> Result<HashDescriptor<'_>, Error> {
    if bytes.len() < HASH_DESCRIPTOR_FIXED_BYTES || &bytes[24..56] != padded_sha256_name() {
        return Err(Error::Descriptor);
    }
    let name_len = to_usize(u64::from(be_u32(bytes, 56)?))?;
    let salt_len = to_usize(u64::from(be_u32(bytes, 60)?))?;
    let digest_len = to_usize(u64::from(be_u32(bytes, 64)?))?;
    if digest_len != 32 {
        return Err(Error::Descriptor);
    }
    let payload = bytes
        .get(HASH_DESCRIPTOR_FIXED_BYTES..)
        .ok_or(Error::Bounds)?;
    let name_end = name_len;
    let salt_end = name_end.checked_add(salt_len).ok_or(Error::Bounds)?;
    let digest_end = salt_end.checked_add(digest_len).ok_or(Error::Bounds)?;
    Ok(HashDescriptor {
        image_size: be_u64(bytes, 16)?,
        partition_name: payload.get(..name_end).ok_or(Error::Bounds)?,
        salt: payload.get(name_end..salt_end).ok_or(Error::Bounds)?,
        digest: payload.get(salt_end..digest_end).ok_or(Error::Bounds)?,
        flags: be_u32(bytes, 68)?,
    })
}

fn padded_sha256_name() -> &'static [u8] {
    b"sha256\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"
}

fn field(bytes: &[u8], offset: u64, size: u64) -> Result<&[u8], Error> {
    let start = to_usize(offset)?;
    let end = start.checked_add(to_usize(size)?).ok_or(Error::Bounds)?;
    bytes.get(start..end).ok_or(Error::Bounds)
}

fn to_usize(value: u64) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::Bounds)
}

fn be_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::Bounds)?
            .try_into()
            .map_err(|_| Error::Bounds)?,
    ))
}

fn be_u64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    Ok(u64::from_be_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(Error::Bounds)?
            .try_into()
            .map_err(|_| Error::Bounds)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsigned_and_truncated_metadata() {
        assert_eq!(verify_vbmeta(&[], &[]), Err(Error::Header));
        let mut header = [0u8; HEADER_BYTES];
        header[..4].copy_from_slice(b"AVB0");
        header[28..32].copy_from_slice(&ALGORITHM_SHA256_RSA2048.to_be_bytes());
        assert_eq!(verify_vbmeta(&header, &[]), Err(Error::Header));
    }

    #[test]
    fn partition_digest_is_bounded_and_authenticated() {
        let digest = Sha256::digest(b"payload");
        let descriptor = HashDescriptor {
            image_size: 7,
            partition_name: b"kernel",
            salt: &[],
            digest: &digest,
            flags: 0,
        };
        assert_eq!(verify_partition(descriptor, b"payload trailing"), Ok(()));
        assert_eq!(verify_partition(descriptor, b"changed"), Err(Error::Digest));
        assert_eq!(verify_partition(descriptor, b"tiny"), Err(Error::Bounds));
    }
}
