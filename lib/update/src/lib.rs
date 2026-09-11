#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

pub const MANIFEST_MAGIC: &[u8; 8] = b"BEXUPD1\0";
pub const MANIFEST_VERSION: u32 = 1;
pub const MAX_APP_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_PLATFORM_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const FIXED_MANIFEST_BYTES: usize = 8 + 4 + 4 + 8 + 8 + 8 + 32 + 32 + 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactKind {
    AppPackage = 1,
    Microkernel = 2,
    TeeImage = 3,
    Hypervisor = 4,
}

impl ArtifactKind {
    pub fn from_u32(value: u32) -> Result<Self, UpdateError> {
        match value {
            1 => Ok(Self::AppPackage),
            2 => Ok(Self::Microkernel),
            3 => Ok(Self::TeeImage),
            4 => Ok(Self::Hypervisor),
            _ => Err(UpdateError::UnsupportedArtifactKind),
        }
    }

    pub const fn max_size(self) -> u64 {
        match self {
            Self::AppPackage => MAX_APP_ARTIFACT_BYTES,
            Self::Microkernel | Self::TeeImage | Self::Hypervisor => MAX_PLATFORM_ARTIFACT_BYTES,
        }
    }

    pub const fn as_tuf_kind(self) -> &'static str {
        match self {
            Self::AppPackage => "APP_PACKAGE",
            Self::Microkernel => "MICROKERNEL",
            Self::TeeImage => "TEE_IMAGE",
            Self::Hypervisor => "HYPERVISOR",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrustedKey<'a> {
    pub key_id: [u8; 32],
    pub public_key: &'a [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateManifest {
    pub generation: u64,
    pub target_id: String,
    pub artifact_kind: ArtifactKind,
    pub artifact_len: u64,
    pub artifact_hash: [u8; 32],
    pub key_id: [u8; 32],
    pub signature: [u8; 64],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateError {
    BadMagic,
    UnsupportedVersion,
    UnexpectedEof,
    InvalidUtf8,
    UnsupportedArtifactKind,
    EmptyArtifact,
    ArtifactTooLarge,
    LengthMismatch,
    HashMismatch,
    UnknownKey,
    BadSignature,
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedUpdate {
    pub manifest: UpdateManifest,
}

pub fn parse_manifest(bytes: &[u8]) -> Result<UpdateManifest, UpdateError> {
    if bytes.len() < FIXED_MANIFEST_BYTES {
        return Err(UpdateError::UnexpectedEof);
    }
    if &bytes[..8] != MANIFEST_MAGIC {
        return Err(UpdateError::BadMagic);
    }
    let version = read_u32(bytes, 8)?;
    if version != MANIFEST_VERSION {
        return Err(UpdateError::UnsupportedVersion);
    }
    let kind = ArtifactKind::from_u32(read_u32(bytes, 12)?)?;
    let generation = read_u64(bytes, 16)?;
    let artifact_len = read_u64(bytes, 24)?;
    let target_len =
        usize::try_from(read_u64(bytes, 32)?).map_err(|_| UpdateError::LengthMismatch)?;
    if FIXED_MANIFEST_BYTES.checked_add(target_len) != Some(bytes.len()) {
        return Err(UpdateError::LengthMismatch);
    }
    let mut artifact_hash = [0; 32];
    artifact_hash.copy_from_slice(slice(bytes, 40, 32)?);
    let mut key_id = [0; 32];
    key_id.copy_from_slice(slice(bytes, 72, 32)?);
    let mut signature = [0; 64];
    signature.copy_from_slice(slice(bytes, 104, 64)?);
    let target = slice(bytes, FIXED_MANIFEST_BYTES, target_len)?;
    let target_id = core::str::from_utf8(target)
        .map_err(|_| UpdateError::InvalidUtf8)?
        .into();
    Ok(UpdateManifest {
        generation,
        target_id,
        artifact_kind: kind,
        artifact_len,
        artifact_hash,
        key_id,
        signature,
    })
}

pub fn verify_update(
    manifest_bytes: &[u8],
    artifact: &[u8],
    trusted: &[TrustedKey<'_>],
    minimum_generation: u64,
) -> Result<VerifiedUpdate, UpdateError> {
    let manifest = parse_manifest(manifest_bytes)?;
    if manifest.generation < minimum_generation {
        return Err(UpdateError::Rollback);
    }
    if manifest.artifact_len == 0 || artifact.is_empty() {
        return Err(UpdateError::EmptyArtifact);
    }
    if manifest.artifact_len > manifest.artifact_kind.max_size() {
        return Err(UpdateError::ArtifactTooLarge);
    }
    if manifest.artifact_len as usize != artifact.len() {
        return Err(UpdateError::LengthMismatch);
    }
    let actual_hash: [u8; 32] = blake3::hash(artifact).into();
    if actual_hash != manifest.artifact_hash {
        return Err(UpdateError::HashMismatch);
    }
    let trusted_key = trusted
        .iter()
        .find(|key| key.key_id == manifest.key_id)
        .ok_or(UpdateError::UnknownKey)?;
    let verifying_key =
        VerifyingKey::from_bytes(trusted_key.public_key).map_err(|_| UpdateError::UnknownKey)?;
    let signature = Signature::from_bytes(&manifest.signature);
    verifying_key
        .verify(&signed_payload(&manifest), &signature)
        .map_err(|_| UpdateError::BadSignature)?;
    Ok(VerifiedUpdate { manifest })
}

pub fn encode_unsigned_manifest(
    generation: u64,
    target_id: &str,
    artifact_kind: ArtifactKind,
    artifact: &[u8],
    key_id: [u8; 32],
) -> Vec<u8> {
    let artifact_hash: [u8; 32] = blake3::hash(artifact).into();
    let manifest = UpdateManifest {
        generation,
        target_id: target_id.into(),
        artifact_kind,
        artifact_len: artifact.len() as u64,
        artifact_hash,
        key_id,
        signature: [0; 64],
    };
    signed_payload(&manifest)
}

#[cfg(feature = "std")]
pub fn build_signed_manifest(
    generation: u64,
    target_id: &str,
    artifact_kind: ArtifactKind,
    artifact: &[u8],
    key_id: [u8; 32],
    signing_seed: [u8; 32],
) -> Vec<u8> {
    use ed25519_dalek::{Signer, SigningKey};

    let unsigned = encode_unsigned_manifest(generation, target_id, artifact_kind, artifact, key_id);
    let signing = SigningKey::from_bytes(&signing_seed);
    let signature = signing.sign(&unsigned).to_bytes();
    let mut signed = unsigned;
    signed[104..168].copy_from_slice(&signature);
    signed
}

fn signed_payload(manifest: &UpdateManifest) -> Vec<u8> {
    let mut out = Vec::with_capacity(FIXED_MANIFEST_BYTES + manifest.target_id.len());
    out.extend_from_slice(MANIFEST_MAGIC);
    put_u32(&mut out, MANIFEST_VERSION);
    put_u32(&mut out, manifest.artifact_kind as u32);
    put_u64(&mut out, manifest.generation);
    put_u64(&mut out, manifest.artifact_len);
    put_u64(&mut out, manifest.target_id.len() as u64);
    out.extend_from_slice(&manifest.artifact_hash);
    out.extend_from_slice(&manifest.key_id);
    out.extend_from_slice(&[0; 64]);
    out.extend_from_slice(manifest.target_id.as_bytes());
    out
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, UpdateError> {
    let mut raw = [0; 4];
    raw.copy_from_slice(slice(bytes, offset, 4)?);
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, UpdateError> {
    let mut raw = [0; 8];
    raw.copy_from_slice(slice(bytes, offset, 8)?);
    Ok(u64::from_le_bytes(raw))
}

fn slice(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], UpdateError> {
    let end = offset.checked_add(len).ok_or(UpdateError::UnexpectedEof)?;
    bytes.get(offset..end).ok_or(UpdateError::UnexpectedEof)
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
