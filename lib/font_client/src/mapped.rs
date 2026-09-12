use bexos_userspace::Memory;

pub struct MappedFont {
    handle: u64,
    address: u64,
    len: usize,
    mapped_len: u64,
    pub font_id: u64,
    pub collection_index: u32,
}

impl MappedFont {
    pub(crate) fn new(
        handle: u64,
        len: u64,
        font_id: u64,
        collection_index: u32,
    ) -> Result<Self, crate::Error> {
        if len == 0 || len > crate::MAX_FONT_BYTES as u64 {
            let _ = Memory::close(handle);
            return Err(crate::Error::InvalidResponse);
        }
        let (kind, rights) = match Memory::object_info(handle) {
            Ok(info) => info,
            Err(_) => {
                let _ = Memory::close(handle);
                return Err(crate::Error::InvalidResponse);
            }
        };
        if kind != kernel_fidl::ObjectType::Vmo || !crate::acceptable_vmo_rights(rights) {
            let _ = Memory::close(handle);
            return Err(crate::Error::InvalidRights);
        }
        let address = Memory::map(handle, len, crate::RIGHT_READ).map_err(|_| {
            let _ = Memory::close(handle);
            crate::Error::Storage
        })?;
        Ok(Self {
            handle,
            address,
            len: len as usize,
            mapped_len: len,
            font_id,
            collection_index,
        })
    }
}

impl AsRef<[u8]> for MappedFont {
    fn as_ref(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.address as *const u8, self.len) }
    }
}

unsafe impl Send for MappedFont {}
unsafe impl Sync for MappedFont {}

impl Drop for MappedFont {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.address, self.mapped_len);
        let _ = Memory::close(self.handle);
    }
}
