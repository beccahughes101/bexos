//! A private writable staging VMO, sealed before it can leave pkgd.
use crate::{Error, Result};
use sha2::{Digest, Sha256};
pub struct Payload {
    #[cfg(feature = "guest")]
    handle: u64,
    #[cfg(feature = "guest")]
    address: u64,
    #[cfg(not(feature = "guest"))]
    bytes: Vec<u8>,
    length: usize,
    written: usize,
    sealed: bool,
}
impl Payload {
    pub fn new(length: usize) -> Result<Self> {
        if length == 0 || length > 256 * 1024 * 1024 {
            return Err(Error::ResourceExhausted);
        }
        #[cfg(feature = "guest")]
        {
            use bexos_userspace::Memory;
            let handle = Memory::create(length as u64, 0).map_err(|_| Error::ResourceExhausted)?;
            let mut value = Self {
                handle,
                address: 0,
                length,
                written: 0,
                sealed: false,
            };
            value.address = Memory::map(
                handle,
                length as u64,
                kernel_fidl::Rights::READ.0 | kernel_fidl::Rights::WRITE.0,
            )
            .map_err(|_| Error::ResourceExhausted)?;
            Ok(value)
        }
        #[cfg(not(feature = "guest"))]
        {
            Ok(Self {
                bytes: vec![0; length],
                length,
                written: 0,
                sealed: false,
            })
        }
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if self.sealed || bytes.len() > self.length.saturating_sub(self.written) {
            return Err(Error::VerifyFailed);
        }
        #[cfg(feature = "guest")]
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.address as *mut u8).add(self.written),
                bytes.len(),
            );
        }
        #[cfg(not(feature = "guest"))]
        self.bytes[self.written..self.written + bytes.len()].copy_from_slice(bytes);
        self.written += bytes.len();
        Ok(())
    }
    pub fn seal(mut self, digest: &[u8; 32]) -> Result<Self> {
        if self.written != self.length || <[u8; 32]>::from(Sha256::digest(&*self)) != *digest {
            return Err(Error::VerifyFailed);
        }
        #[cfg(feature = "guest")]
        {
            use bexos_userspace::Memory;
            Memory::unmap(self.address, self.length as u64).map_err(|_| Error::Io)?;
            self.address = 0;
            let canonical = Memory::duplicate(
                self.handle,
                kernel_fidl::Rights::READ.0
                    | kernel_fidl::Rights::MAP.0
                    | kernel_fidl::Rights::TRANSFER.0
                    | kernel_fidl::Rights::DUPLICATE.0,
            )
            .map_err(|_| Error::Io)?;
            let old = core::mem::replace(&mut self.handle, canonical);
            Memory::close(old).map_err(|_| Error::Io)?;
            self.address =
                Memory::map(self.handle, self.length as u64, kernel_fidl::Rights::READ.0)
                    .map_err(|_| Error::Io)?;
        }
        self.sealed = true;
        Ok(self)
    }
    pub fn from_bytes(bytes: &[u8], digest: &[u8; 32]) -> Result<Self> {
        let mut value = Self::new(bytes.len())?;
        value.write(bytes)?;
        value.seal(digest)
    }
    #[cfg(feature = "guest")]
    pub fn duplicate_handle(&self) -> Result<u64> {
        if !self.sealed {
            return Err(Error::VerifyFailed);
        }
        bexos_userspace::Memory::duplicate(
            self.handle,
            kernel_fidl::Rights::READ.0
                | kernel_fidl::Rights::MAP.0
                | kernel_fidl::Rights::TRANSFER.0
                | kernel_fidl::Rights::DUPLICATE.0,
        )
        .map_err(|_| Error::ResourceExhausted)
    }
    #[cfg(feature = "guest")]
    pub fn into_handle(mut self) -> Result<u64> {
        if !self.sealed {
            return Err(Error::VerifyFailed);
        }
        bexos_userspace::Memory::unmap(self.address, self.length as u64).map_err(|_| Error::Io)?;
        self.address = 0;
        Ok(core::mem::replace(&mut self.handle, 0))
    }
}
impl core::ops::Deref for Payload {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        #[cfg(feature = "guest")]
        unsafe {
            core::slice::from_raw_parts(self.address as *const u8, self.length)
        }
        #[cfg(not(feature = "guest"))]
        {
            &self.bytes
        }
    }
}
#[cfg(feature = "guest")]
impl Drop for Payload {
    fn drop(&mut self) {
        if self.address != 0 {
            let _ = bexos_userspace::Memory::unmap(self.address, self.length as u64);
        }
        if self.handle != 0 {
            let _ = bexos_userspace::Memory::close(self.handle);
        }
    }
}
impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        self
    }
}
impl core::fmt::Debug for Payload {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Payload")
            .field("length", &self.length)
            .field("sealed", &self.sealed)
            .finish()
    }
}
