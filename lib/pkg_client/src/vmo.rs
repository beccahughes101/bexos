use bexos_userspace::Memory;
use pkg_fidl::{BlobDigest, HashType, PackageStatus};
use sha2::{Digest, Sha256};

pub const CLIENT_RIGHTS: u32 =
    kernel_fidl::Rights::TRANSFER.0 | kernel_fidl::Rights::READ.0 | kernel_fidl::Rights::MAP.0;
pub struct ReadOnlyVmo {
    handle: u64,
    address: u64,
    length: u64,
    digest: BlobDigest,
}
impl ReadOnlyVmo {
    pub fn adopt(
        handle: u64,
        length: u64,
        digest: BlobDigest,
        expected: Option<&BlobDigest>,
    ) -> Result<Self, PackageStatus> {
        let mut vmo = Self {
            handle,
            address: 0,
            length,
            digest,
        };
        if length == 0 || length > 256 * 1024 * 1024 {
            return Err(PackageStatus::VerifyFailed);
        }
        let (kind, rights) =
            Memory::object_info(handle).map_err(|_| PackageStatus::VerifyFailed)?;
        if kind != kernel_fidl::ObjectType::Vmo || rights != CLIENT_RIGHTS {
            return Err(PackageStatus::VerifyFailed);
        }
        vmo.address = Memory::map(handle, length, kernel_fidl::Rights::READ.0)
            .map_err(|_| PackageStatus::VerifyFailed)?;
        if !matches_digest(vmo.bytes(), &digest)
            || expected.is_some_and(|digest| !matches_digest(vmo.bytes(), digest))
        {
            return Err(PackageStatus::VerifyFailed);
        }
        Ok(vmo)
    }
    pub fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.address as *const u8, self.length as usize) }
    }
    pub fn length(&self) -> u64 {
        self.length
    }
    pub fn digest(&self) -> BlobDigest {
        self.digest
    }
    pub fn handle(&self) -> u64 {
        self.handle
    }
    pub fn into_handle(mut self) -> u64 {
        if self.address != 0 {
            let _ = Memory::unmap(self.address, self.length);
            self.address = 0;
        }
        core::mem::replace(&mut self.handle, 0)
    }
}
impl Drop for ReadOnlyVmo {
    fn drop(&mut self) {
        if self.address != 0 {
            let _ = Memory::unmap(self.address, self.length);
        }
        if self.handle != 0 {
            let _ = Memory::close(self.handle);
        }
    }
}
pub fn matches_digest(bytes: &[u8], digest: &BlobDigest) -> bool {
    let actual: [u8; 32] = match digest.hash_type {
        HashType::Sha256 => Sha256::digest(bytes).into(),
        HashType::Blake3 => *blake3::hash(bytes).as_bytes(),
    };
    actual == digest.digest
}
