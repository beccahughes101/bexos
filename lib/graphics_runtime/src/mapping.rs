use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Memory, live_migration::Resource};
/// A retained VMO mapping. Adopted candidates do not own source resources before commit.
#[derive(Debug)]
pub struct Mapping {
    pub handle: u64,
    pub address: u64,
    pub size: u64,
    pub rights: u32,
    pub owned: bool,
}
impl Mapping {
    pub fn new(size: u64) -> Result<Self, kernel_fidl::Status> {
        let h = Memory::create(size, 0)?;
        Self::map(h, size, 6)
    }
    pub fn map(handle: u64, size: u64, rights: u32) -> Result<Self, kernel_fidl::Status> {
        match Memory::map(handle, size, rights) {
            Ok(address) => Ok(Self {
                handle,
                address,
                size,
                rights,
                owned: true,
            }),
            Err(e) => {
                let _ = Memory::close(handle);
                Err(e)
            }
        }
    }
    pub fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.address as *const u8, self.size as usize) }
    }
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        assert!(self.rights & 4 != 0);
        unsafe { core::slice::from_raw_parts_mut(self.address as *mut u8, self.size as usize) }
    }
    pub fn encode(&self, w: &mut Encoder) {
        for n in [self.handle, self.address, self.size, self.rights as u64] {
            w.word(n);
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let m = Self {
            handle: r.word()?,
            address: r.word()?,
            size: r.word()?,
            rights: r.word()? as u32,
            owned: false,
        };
        if m.handle == 0
            || m.address == 0
            || m.address % 4096 != 0
            || m.size == 0
            || m.size > 64 * 1024 * 1024
            || !matches!(m.rights, 2 | 6)
        {
            return Err(Error::InvalidData);
        }
        Ok(m)
    }
    pub fn resources(&self) -> Vec<Resource> {
        vec![
            Resource::Handle(self.handle),
            Resource::Mapping {
                handle: self.handle,
                offset: 0,
                va: self.address,
                size: (self.size + 4095) & !4095,
                rights: self.rights,
            },
        ]
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        if self.owned {
            let _ = Memory::unmap(self.address, self.size);
            let _ = Memory::close(self.handle);
        }
    }
}
