//! Runtime updates require a separate platform-signed archive with a fixed role.
//! A guest archive's signature never authorizes its arbitrary native files.
use crate::{Manifest, PackageImageError};
use alloc::vec::Vec;
use sha2::{Digest, Sha256};
pub const ARCHIVE_PATH: &str = ".bexos/wasm_runner.bex";
pub struct VerifiedRuntime {
    pub bytes: Vec<u8>,
    pub digest: [u8; 32],
}
pub const MAX_RUNTIME_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RUNTIME_ELF_BYTES: u64 = 64 * 1024 * 1024;

pub fn verify(bytes: &[u8]) -> Result<VerifiedRuntime, PackageImageError> {
    if bytes.len() > MAX_RUNTIME_ARCHIVE_BYTES {
        return Err(PackageImageError::AccessDenied);
    }
    let archive = bexos_app_archive::OpenArchive::parse_and_verify(bytes, &trusted_keys())
        .map_err(|_| PackageImageError::AccessDenied)?;
    let manifest = archive
        .find("package.bexmanifest")
        .filter(|e| e.uncompressed_size <= 65536)
        .ok_or(PackageImageError::AccessDenied)?;
    let manifest = archive
        .read_file(manifest)
        .map_err(|_| PackageImageError::AccessDenied)?;
    let manifest = Manifest::decode(&manifest).map_err(|_| PackageImageError::AccessDenied)?;
    if manifest.package_name != bexos_wasm_abi::RUNNER_PACKAGE
        || !manifest.processes.is_empty()
        || manifest.driver_info.is_some()
        || !manifest.library_dependencies.is_empty()
    {
        return Err(PackageImageError::AccessDenied);
    }
    let entry = archive
        .find("bin/wasm_runner")
        .filter(|e| e.uncompressed_size <= MAX_RUNTIME_ELF_BYTES)
        .ok_or(PackageImageError::AccessDenied)?;
    let bytes = archive
        .read_file(entry)
        .map_err(|_| PackageImageError::AccessDenied)?;
    if !bytes.starts_with(b"\x7fELF") {
        return Err(PackageImageError::AccessDenied);
    }
    let digest = Sha256::digest(&bytes).into();
    Ok(VerifiedRuntime { bytes, digest })
}
pub(crate) fn trusted_keys() -> [bexos_app_archive::TrustedKey<'static>; 1] {
    [bexos_app_archive::TrustedKey {
        key_id: *b"bexos-qemu-test-ed25519-key-v001",
        public_key: &[
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ],
    }]
}
