use crate::{AsHandleRef, Handle, HandleRef, Result, Rights, Status};
use bexos_userspace::Memory;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VmarFlags(pub u32);

impl VmarFlags {
    pub const PERM_READ: Self = Self(1);
    pub const PERM_WRITE: Self = Self(2);
    pub const PERM_EXECUTE: Self = Self(4);
    pub const SPECIFIC: Self = Self(8);
}

#[derive(Debug)]
pub struct Vmo(Handle);

impl Vmo {
    pub fn create(size: u64) -> Result<Self> {
        Memory::create(size, 0)
            .map(|raw| Self(unsafe { Handle::from_raw(raw) }))
            .map_err(Status::from)
    }

    pub const unsafe fn from_raw(raw: u64) -> Self {
        Self(unsafe { Handle::from_raw(raw) })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Memory::from_bytes(bytes)
            .map(|raw| Self(unsafe { Handle::from_raw(raw) }))
            .map_err(Status::from)
    }

    pub fn duplicate(&self, rights: Rights) -> Result<Self> {
        self.0.duplicate(rights).map(Self)
    }

    pub fn into_raw(self) -> u64 {
        self.0.into_raw()
    }
}

impl AsHandleRef for Vmo {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        self.0.as_handle_ref()
    }
}

#[derive(Debug)]
pub struct MappedVmo {
    address: u64,
    size: u64,
}

impl MappedVmo {
    pub const fn address(&self) -> u64 {
        self.address
    }

    pub const fn size(&self) -> u64 {
        self.size
    }
}

impl Drop for MappedVmo {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.address, self.size);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vmar;

impl Vmar {
    pub const fn root_self() -> Self {
        Self
    }

    pub fn map(
        &self,
        vmar_offset: u64,
        vmo: &Vmo,
        vmo_offset: u64,
        size: u64,
        flags: VmarFlags,
    ) -> Result<MappedVmo> {
        let target = if flags.0 & VmarFlags::SPECIFIC.0 != 0 {
            vmar_offset
        } else {
            0
        };
        let mut rights = 0;
        if flags.0 & VmarFlags::PERM_READ.0 != 0 {
            rights |= Rights::READ.0;
        }
        if flags.0 & VmarFlags::PERM_WRITE.0 != 0 {
            rights |= Rights::WRITE.0;
        }
        if flags.0 & VmarFlags::PERM_EXECUTE.0 != 0 {
            rights |= Rights::EXECUTE.0;
        }
        Memory::map_at(
            vmo.as_handle_ref().raw_handle(),
            vmo_offset,
            size,
            target,
            rights,
        )
        .map(|address| MappedVmo { address, size })
        .map_err(Status::from)
    }

    pub fn unmap(&self, address: u64, size: u64) -> Result<()> {
        Memory::unmap(address, size).map_err(Status::from)
    }
}
