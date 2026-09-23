use bexos_userspace::Memory;
use locale_fidl::Status;
/// Owns the address-space mapping; the Client separately owns the VMO handle.
pub struct MappedData {
    address: u64,
    len: u64,
}
impl MappedData {
    pub fn map(handle: u64, len: u64) -> Result<Self, Status> {
        if handle == 0 || len == 0 || len > 64 * 1024 * 1024 {
            return Err(Status::InvalidArgs);
        }
        let (kind, rights) = Memory::object_info(handle).map_err(|_| Status::InvalidArgs)?;
        if kind != kernel_fidl::ObjectType::Vmo || rights & 4 != 0 || rights & (2 | 16) != (2 | 16)
        {
            return Err(Status::AccessDenied);
        }
        let address = Memory::map(handle, len, 2).map_err(|_| Status::Io)?;
        Ok(Self { address, len })
    }
    pub fn bytes(&self) -> &[u8] {
        // This immutable slice cannot outlive the mapping. The VMO has no write
        // capability in this process and is mapped with read permission only.
        unsafe { core::slice::from_raw_parts(self.address as *const u8, self.len as usize) }
    }
}
impl Drop for MappedData {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.address, self.len);
    }
}
