#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
use alloc::collections::BTreeSet;
use alloc::string::String;
#[cfg(feature = "std")]
use alloc::vec;
use alloc::vec::Vec;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

pub const MAGIC: &[u8; 8] = b"BEXARCV2";
pub const LEGACY_V1_MAGIC: &[u8; 8] = b"BEXARCV1";
pub const VERSION: u32 = 2;
pub const CHUNK_SIZE: usize = 4096;
const FIXED_HEADER_BYTES: usize = 32;
const ENTRY_RECORD_BYTES: usize = 72;
const SIGNATURE_MAGIC: &[u8; 8] = b"BEXSIGV2";
const SIGNATURE_FOOTER_LEN_BYTES: usize = 8;
const SIGNATURE_HEADER_BYTES: usize = 8 + 4 + 4 + 8 + 32 + 32 + 4 + 4;
const MAX_CHAIN_CERTS: usize = 4;
const MAX_CHAIN_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Compression {
    None,
    Zstd,
}

impl Compression {
    #[cfg(feature = "std")]
    fn to_u32(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Zstd => 1,
        }
    }

    fn from_u32(value: u32) -> Result<Self, ArchiveError> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Zstd),
            _ => Err(ArchiveError::UnsupportedCompression),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureAlgorithm {
    Ed25519 = 1,
    EcdsaP256Sha256 = 2,
}

impl SignatureAlgorithm {
    fn from_u32(value: u32) -> Result<Self, ArchiveError> {
        match value {
            1 => Ok(Self::Ed25519),
            2 => Ok(Self::EcdsaP256Sha256),
            _ => Err(ArchiveError::UnknownSignatureAlgorithm),
        }
    }

    #[cfg(feature = "std")]
    fn signature_len(self) -> usize {
        match self {
            Self::Ed25519 => 64,
            Self::EcdsaP256Sha256 => 64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedKey<'a> {
    pub key_id: [u8; 32],
    pub public_key: &'a [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveEntry {
    pub path: String,
    pub compression: Compression,
    pub mode: u32,
    pub uncompressed_size: u64,
    pub stored_offset: u64,
    pub stored_len: u64,
    pub chunk_hash_offset: u64,
    pub chunk_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenArchive<'a> {
    bytes: &'a [u8],
    entries: Vec<ArchiveEntry>,
    key_id: [u8; 32],
    content_root: [u8; 32],
    signer: SignatureEnvelope<'a>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignatureEnvelope<'a> {
    pub algorithm: SignatureAlgorithm,
    pub key_id: [u8; 32],
    pub content_digest: [u8; 32],
    pub certificate_chain: Vec<&'a [u8]>,
    pub signature: &'a [u8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedArchive<'a> {
    pub archive: OpenArchive<'a>,
    pub anchor_id: String,
    pub signer_fingerprint: [u8; 32],
    pub granted_tier: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    BadMagic,
    UnsupportedVersion,
    UnexpectedEof,
    LengthOverflow,
    InvalidUtf8,
    InvalidPath,
    DuplicatePath,
    EntriesOutOfOrder,
    BadSignatureBlock,
    UnknownKey,
    UnknownSignatureAlgorithm,
    BadSignature,
    BadChunkHash,
    UnsupportedCompression,
    ChainTooLarge,
    DigestMismatch,
    Zstd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildEntry<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub mode: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltArchive {
    pub bytes: Vec<u8>,
    pub content_root: [u8; 32],
}

impl<'a> OpenArchive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ArchiveError> {
        Self::parse_inner(bytes, true)
    }

    fn parse_inner(bytes: &'a [u8], verify_entry_chunks: bool) -> Result<Self, ArchiveError> {
        if bytes.len() < FIXED_HEADER_BYTES + SIGNATURE_FOOTER_LEN_BYTES {
            return Err(ArchiveError::UnexpectedEof);
        }
        if &bytes[..8] == LEGACY_V1_MAGIC {
            return Err(ArchiveError::UnsupportedVersion);
        }
        if &bytes[..8] != MAGIC {
            return Err(ArchiveError::BadMagic);
        }
        let version = read_u32(bytes, 8)?;
        if version != VERSION {
            return Err(ArchiveError::UnsupportedVersion);
        }
        let entry_count = read_u32(bytes, 12)?;
        let directory_offset = read_u64(bytes, 16)?;
        let directory_len = read_u64(bytes, 24)?;
        let footer_len_offset = bytes.len() - SIGNATURE_FOOTER_LEN_BYTES;
        let footer_len = read_u64(bytes, footer_len_offset)? as usize;
        if footer_len < SIGNATURE_HEADER_BYTES {
            return Err(ArchiveError::BadSignatureBlock);
        }
        let sig_offset = footer_len_offset
            .checked_sub(footer_len)
            .ok_or(ArchiveError::BadSignatureBlock)?;
        if directory_offset as usize + directory_len as usize > sig_offset {
            return Err(ArchiveError::UnexpectedEof);
        }

        let sig = SignatureBlock::parse(slice(bytes, sig_offset, footer_len)?)?;
        if sig.signed_len as usize != sig_offset {
            return Err(ArchiveError::BadSignatureBlock);
        }
        let actual_digest: [u8; 32] = blake3::hash(&bytes[..sig_offset]).into();
        if actual_digest != sig.content_digest {
            return Err(ArchiveError::DigestMismatch);
        }

        let entries = parse_directory(
            bytes,
            directory_offset as usize,
            directory_len as usize,
            entry_count as usize,
            sig_offset,
        )?;
        let archive = Self {
            bytes,
            entries,
            key_id: sig.key_id,
            content_root: sig.content_digest,
            signer: SignatureEnvelope {
                algorithm: sig.algorithm,
                key_id: sig.key_id,
                content_digest: sig.content_digest,
                certificate_chain: sig.certificate_chain,
                signature: sig.signature,
            },
        };
        if verify_entry_chunks {
            for entry in &archive.entries {
                archive.verify_entry_chunks(entry)?;
            }
        }
        Ok(archive)
    }

    pub fn parse_and_verify(
        bytes: &'a [u8],
        trusted: &[TrustedKey<'_>],
    ) -> Result<Self, ArchiveError> {
        let archive = Self::parse_inner(bytes, false)?;
        let sig = archive.signer();
        let trusted_key = trusted
            .iter()
            .find(|key| key.key_id == sig.key_id)
            .ok_or(ArchiveError::UnknownKey)?;
        if sig.algorithm != SignatureAlgorithm::Ed25519
            || sig.certificate_chain.len() != 1
            || sig.certificate_chain[0] != trusted_key.public_key
        {
            return Err(ArchiveError::UnknownKey);
        }
        let verifying = VerifyingKey::from_bytes(trusted_key.public_key)
            .map_err(|_| ArchiveError::UnknownKey)?;
        let signature_bytes: [u8; 64] = sig
            .signature
            .try_into()
            .map_err(|_| ArchiveError::BadSignatureBlock)?;
        let signature = Signature::from_bytes(&signature_bytes);
        verifying
            .verify(&sig.content_digest, &signature)
            .map_err(|_| ArchiveError::BadSignature)?;
        Ok(archive)
    }

    pub fn assume_verified(
        self,
        anchor_id: String,
        signer_fingerprint: [u8; 32],
        granted_tier: u8,
    ) -> VerifiedArchive<'a> {
        VerifiedArchive {
            archive: self,
            anchor_id,
            signer_fingerprint,
            granted_tier,
        }
    }

    pub fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }

    pub fn key_id(&self) -> [u8; 32] {
        self.key_id
    }

    pub fn content_root(&self) -> [u8; 32] {
        self.content_root
    }

    pub fn signer(&self) -> &SignatureEnvelope<'a> {
        &self.signer
    }

    pub fn find(&self, path: &str) -> Option<&ArchiveEntry> {
        let normalized = normalize_path(path).ok()?;
        self.entries.iter().find(|entry| entry.path == normalized)
    }

    pub fn read_file(&self, entry: &ArchiveEntry) -> Result<Vec<u8>, ArchiveError> {
        self.verify_entry_chunks(entry)?;
        let stored = self.stored_bytes(entry)?;
        match entry.compression {
            Compression::None => {
                if stored.len() != entry.uncompressed_size as usize {
                    return Err(ArchiveError::UnexpectedEof);
                }
                Ok(stored.to_vec())
            }
            Compression::Zstd => decompress_zstd(stored, entry.uncompressed_size as usize),
        }
    }

    pub fn stored_bytes(&self, entry: &ArchiveEntry) -> Result<&'a [u8], ArchiveError> {
        slice(
            self.bytes,
            entry.stored_offset as usize,
            entry.stored_len as usize,
        )
    }

    fn verify_entry_chunks(&self, entry: &ArchiveEntry) -> Result<(), ArchiveError> {
        let stored = self.stored_bytes(entry)?;
        let expected_count = if stored.is_empty() {
            0
        } else {
            stored.len().div_ceil(CHUNK_SIZE)
        };
        if expected_count != entry.chunk_count as usize {
            return Err(ArchiveError::BadChunkHash);
        }
        for index in 0..entry.chunk_count as usize {
            let start = index * CHUNK_SIZE;
            let end = stored.len().min(start + CHUNK_SIZE);
            let expected = slice(
                self.bytes,
                entry.chunk_hash_offset as usize + index * 32,
                32,
            )?;
            let actual = blake3::hash(&stored[start..end]);
            if actual.as_bytes() != expected {
                return Err(ArchiveError::BadChunkHash);
            }
        }
        Ok(())
    }
}

#[cfg(feature = "std")]
pub fn build_archive(
    entries: &[BuildEntry<'_>],
    compression: Compression,
    key_id: [u8; 32],
    signing_seed: [u8; 32],
) -> Result<BuiltArchive, ArchiveError> {
    use ed25519_dalek::{Signer, SigningKey};

    let signing_key = SigningKey::from_bytes(&signing_seed);
    let public_key = signing_key.verifying_key().to_bytes();
    build_archive_with_signer(
        entries,
        compression,
        key_id,
        SignatureAlgorithm::Ed25519,
        &[public_key.to_vec()],
        |digest| Ok(signing_key.sign(digest).to_bytes().to_vec()),
    )
}

#[cfg(feature = "std")]
pub fn build_archive_with_signer(
    entries: &[BuildEntry<'_>],
    compression: Compression,
    key_id: [u8; 32],
    algorithm: SignatureAlgorithm,
    certificate_chain: &[Vec<u8>],
    sign_digest: impl FnOnce(&[u8; 32]) -> Result<Vec<u8>, ArchiveError>,
) -> Result<BuiltArchive, ArchiveError> {
    if certificate_chain.is_empty()
        || certificate_chain.len() > MAX_CHAIN_CERTS
        || certificate_chain.iter().map(Vec::len).sum::<usize>() > MAX_CHAIN_BYTES
    {
        return Err(ArchiveError::ChainTooLarge);
    }

    let unsigned = build_unsigned(entries, compression)?;
    let content_root: [u8; 32] = blake3::hash(&unsigned).into();
    let signature = sign_digest(&content_root)?;
    if signature.len() > algorithm.signature_len() {
        return Err(ArchiveError::BadSignatureBlock);
    }
    let mut out = unsigned;
    let signed_len = out.len() as u64;
    let footer_start = out.len();
    out.extend_from_slice(SIGNATURE_MAGIC);
    put_u32_extend(&mut out, VERSION);
    put_u32_extend(&mut out, algorithm as u32);
    put_u64_extend(&mut out, signed_len);
    out.extend_from_slice(&key_id);
    out.extend_from_slice(&content_root);
    put_u32_extend(&mut out, certificate_chain.len() as u32);
    put_u32_extend(&mut out, signature.len() as u32);
    for cert in certificate_chain {
        put_u32_extend(&mut out, cert.len() as u32);
        out.extend_from_slice(cert);
    }
    out.extend_from_slice(&signature);
    let footer_len = (out.len() - footer_start) as u64;
    put_u64_extend(&mut out, footer_len);
    Ok(BuiltArchive {
        bytes: out,
        content_root,
    })
}

#[cfg(feature = "std")]
fn build_unsigned(
    entries: &[BuildEntry<'_>],
    compression: Compression,
) -> Result<Vec<u8>, ArchiveError> {
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in entries {
        let path = normalize_path(entry.path)?;
        if !seen.insert(path.clone()) {
            return Err(ArchiveError::DuplicatePath);
        }
        normalized.push((path, entry.bytes, entry.mode));
    }
    normalized.sort_by(|a, b| a.0.cmp(&b.0));

    let directory_offset = FIXED_HEADER_BYTES;
    let directory_len = normalized
        .iter()
        .map(|(path, _, _)| ENTRY_RECORD_BYTES + path.len())
        .sum::<usize>();
    let mut out = vec![0; FIXED_HEADER_BYTES + directory_len];
    out[..8].copy_from_slice(MAGIC);
    put_u32(&mut out, 8, VERSION);
    put_u32(&mut out, 12, normalized.len() as u32);
    put_u64(&mut out, 16, directory_offset as u64);
    put_u64(&mut out, 24, directory_len as u64);

    let mut records = Vec::new();
    for (path, uncompressed, mode) in &normalized {
        let stored = match compression {
            Compression::None => uncompressed.to_vec(),
            Compression::Zstd => compress_zstd(uncompressed)?,
        };
        let stored_offset = out.len() as u64;
        out.extend_from_slice(&stored);
        let chunk_hash_offset = out.len() as u64;
        let chunk_count = if stored.is_empty() {
            0
        } else {
            stored.len().div_ceil(CHUNK_SIZE)
        };
        for chunk in stored.chunks(CHUNK_SIZE) {
            out.extend_from_slice(blake3::hash(chunk).as_bytes());
        }
        records.push(ArchiveEntry {
            path: path.clone(),
            compression,
            mode: *mode,
            uncompressed_size: uncompressed.len() as u64,
            stored_offset,
            stored_len: stored.len() as u64,
            chunk_hash_offset,
            chunk_count: chunk_count as u32,
        });
    }

    let mut cursor = directory_offset;
    for record in &records {
        put_u32(&mut out, cursor, record.path.len() as u32);
        put_u32(&mut out, cursor + 4, record.compression.to_u32());
        put_u32(&mut out, cursor + 8, record.mode);
        put_u32(&mut out, cursor + 12, record.chunk_count);
        put_u64(&mut out, cursor + 16, record.uncompressed_size);
        put_u64(&mut out, cursor + 24, record.stored_offset);
        put_u64(&mut out, cursor + 32, record.stored_len);
        put_u64(&mut out, cursor + 40, record.chunk_hash_offset);
        out[cursor + 72..cursor + 72 + record.path.len()].copy_from_slice(record.path.as_bytes());
        cursor += ENTRY_RECORD_BYTES + record.path.len();
    }
    Ok(out)
}

fn parse_directory(
    bytes: &[u8],
    offset: usize,
    len: usize,
    count: usize,
    signed_len: usize,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let end = offset
        .checked_add(len)
        .ok_or(ArchiveError::LengthOverflow)?;
    if end > bytes.len() {
        return Err(ArchiveError::UnexpectedEof);
    }
    let mut cursor = offset;
    let mut entries = Vec::new();
    let mut last = String::new();
    for index in 0..count {
        if cursor + ENTRY_RECORD_BYTES > end {
            return Err(ArchiveError::UnexpectedEof);
        }
        let path_len = read_u32(bytes, cursor)? as usize;
        let compression = Compression::from_u32(read_u32(bytes, cursor + 4)?)?;
        let mode = read_u32(bytes, cursor + 8)?;
        let chunk_count = read_u32(bytes, cursor + 12)?;
        let uncompressed_size = read_u64(bytes, cursor + 16)?;
        let stored_offset = read_u64(bytes, cursor + 24)?;
        let stored_len = read_u64(bytes, cursor + 32)?;
        let chunk_hash_offset = read_u64(bytes, cursor + 40)?;
        let path_bytes = slice(bytes, cursor + ENTRY_RECORD_BYTES, path_len)?;
        let path = core::str::from_utf8(path_bytes)
            .map_err(|_| ArchiveError::InvalidUtf8)
            .and_then(normalize_path)?;
        if index > 0 && path <= last {
            return if path == last {
                Err(ArchiveError::DuplicatePath)
            } else {
                Err(ArchiveError::EntriesOutOfOrder)
            };
        }
        last = path.clone();
        bounds_within_signature(stored_offset, stored_len, signed_len)?;
        bounds_within_signature(chunk_hash_offset, u64::from(chunk_count) * 32, signed_len)?;
        entries.push(ArchiveEntry {
            path,
            compression,
            mode,
            uncompressed_size,
            stored_offset,
            stored_len,
            chunk_hash_offset,
            chunk_count,
        });
        cursor += ENTRY_RECORD_BYTES + path_len;
    }
    if cursor != end {
        return Err(ArchiveError::UnexpectedEof);
    }
    Ok(entries)
}

fn bounds_within_signature(offset: u64, len: u64, signed_len: usize) -> Result<(), ArchiveError> {
    let end = offset
        .checked_add(len)
        .ok_or(ArchiveError::LengthOverflow)?;
    if end as usize > signed_len {
        return Err(ArchiveError::UnexpectedEof);
    }
    Ok(())
}

fn normalize_path(path: &str) -> Result<String, ArchiveError> {
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Err(ArchiveError::InvalidPath);
    }
    let mut out = String::new();
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(ArchiveError::InvalidPath);
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(component);
    }
    Ok(out)
}

struct SignatureBlock;

impl SignatureBlock {
    fn parse<'a>(bytes: &'a [u8]) -> Result<SignatureBlockRef<'a>, ArchiveError> {
        if bytes.len() < SIGNATURE_HEADER_BYTES || &bytes[..8] != SIGNATURE_MAGIC {
            return Err(ArchiveError::BadSignatureBlock);
        }
        if read_u32(bytes, 8)? != VERSION {
            return Err(ArchiveError::BadSignatureBlock);
        }
        let algorithm = SignatureAlgorithm::from_u32(read_u32(bytes, 12)?)?;
        let signed_len = read_u64(bytes, 16)?;
        let mut key_id = [0; 32];
        key_id.copy_from_slice(slice(bytes, 24, 32)?);
        let mut content_digest = [0; 32];
        content_digest.copy_from_slice(slice(bytes, 56, 32)?);
        let chain_count = read_u32(bytes, 88)? as usize;
        let signature_len = read_u32(bytes, 92)? as usize;
        if chain_count == 0 || chain_count > MAX_CHAIN_CERTS {
            return Err(ArchiveError::ChainTooLarge);
        }
        if signature_len == 0 || signature_len > 512 {
            return Err(ArchiveError::BadSignatureBlock);
        }
        let mut offset = SIGNATURE_HEADER_BYTES;
        let mut chain = Vec::new();
        let mut total_chain_bytes = 0usize;
        for _ in 0..chain_count {
            let len = read_u32(bytes, offset)? as usize;
            offset = offset.checked_add(4).ok_or(ArchiveError::LengthOverflow)?;
            total_chain_bytes = total_chain_bytes
                .checked_add(len)
                .ok_or(ArchiveError::LengthOverflow)?;
            if len == 0 || total_chain_bytes > MAX_CHAIN_BYTES {
                return Err(ArchiveError::ChainTooLarge);
            }
            chain.push(slice(bytes, offset, len)?);
            offset = offset
                .checked_add(len)
                .ok_or(ArchiveError::LengthOverflow)?;
        }
        let signature = slice(bytes, offset, signature_len)?;
        offset = offset
            .checked_add(signature_len)
            .ok_or(ArchiveError::LengthOverflow)?;
        if offset != bytes.len() {
            return Err(ArchiveError::BadSignatureBlock);
        }
        Ok(SignatureBlockRef {
            algorithm,
            signed_len,
            key_id,
            content_digest,
            certificate_chain: chain,
            signature,
        })
    }
}

struct SignatureBlockRef<'a> {
    algorithm: SignatureAlgorithm,
    signed_len: u64,
    key_id: [u8; 32],
    content_digest: [u8; 32],
    certificate_chain: Vec<&'a [u8]>,
    signature: &'a [u8],
}

fn slice(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], ArchiveError> {
    let end = offset
        .checked_add(len)
        .ok_or(ArchiveError::LengthOverflow)?;
    bytes.get(offset..end).ok_or(ArchiveError::UnexpectedEof)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ArchiveError> {
    let mut raw = [0; 4];
    raw.copy_from_slice(slice(bytes, offset, 4)?);
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ArchiveError> {
    let mut raw = [0; 8];
    raw.copy_from_slice(slice(bytes, offset, 8)?);
    Ok(u64::from_le_bytes(raw))
}

#[cfg(feature = "std")]
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "std")]
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "std")]
fn put_u32_extend(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "std")]
fn put_u64_extend(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(all(feature = "std", feature = "zstd"))]
fn compress_zstd(bytes: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    use ruzstd::encoding::CompressionLevel;
    Ok(ruzstd::encoding::compress_to_vec(
        bytes,
        CompressionLevel::Fastest,
    ))
}

#[cfg(all(feature = "std", not(feature = "zstd")))]
fn compress_zstd(_bytes: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    Err(ArchiveError::UnsupportedCompression)
}

#[cfg(feature = "zstd")]
fn decompress_zstd(bytes: &[u8], expected_len: usize) -> Result<Vec<u8>, ArchiveError> {
    use ruzstd::io::Read;
    let mut decoder =
        ruzstd::decoding::StreamingDecoder::new(bytes).map_err(|_| ArchiveError::Zstd)?;
    let mut out = Vec::new();
    // The signed entry already supplies an exact output bound. Geometric
    // read_to_end growth can request 64 MiB for a 35 MiB service, exceeding a
    // guest VMO's limit once allocator metadata is included. Reserve once and
    // reject both short output and expansion beyond the authenticated length.
    out.try_reserve_exact(expected_len)
        .map_err(|_| ArchiveError::LengthOverflow)?;
    out.resize(expected_len, 0);
    // StreamingDecoder retains enough decoded bytes to fill each requested
    // slice before returning it. Bound each read so its history ring does not
    // grow to the entire executable in addition to our output allocation.
    for chunk in out.chunks_mut(128 * 1024) {
        Read::read_exact(&mut decoder, chunk).map_err(|_| ArchiveError::Zstd)?;
    }
    if Read::read(&mut decoder, &mut [0; 1]).map_err(|_| ArchiveError::Zstd)? != 0 {
        return Err(ArchiveError::Zstd);
    }
    Ok(out)
}

#[cfg(not(feature = "zstd"))]
fn decompress_zstd(_bytes: &[u8], _expected_len: usize) -> Result<Vec<u8>, ArchiveError> {
    Err(ArchiveError::UnsupportedCompression)
}
