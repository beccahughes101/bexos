use crate::{Result, Status};
use bexos_userspace::Memory;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rights(pub u32);

impl Rights {
    pub const READ: Self = Self(2);
    pub const WRITE: Self = Self(4);
    pub const EXECUTE: Self = Self(8);
    pub const MAP: Self = Self(16);
    pub const SAME_RIGHTS: Self = Self(u32::MAX);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandleRef<'a> {
    raw: u64,
    marker: core::marker::PhantomData<&'a Handle>,
}

impl HandleRef<'_> {
    pub const fn raw_handle(self) -> u64 {
        self.raw
    }
}

pub trait AsHandleRef {
    fn as_handle_ref(&self) -> HandleRef<'_>;
}

#[derive(Debug, Eq, PartialEq)]
pub struct Handle(u64);

impl Handle {
    /// Takes ownership of a raw BexOS capability.
    pub const unsafe fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw_handle(&self) -> u64 {
        self.0
    }

    pub fn duplicate(&self, rights: Rights) -> Result<Self> {
        Memory::duplicate(self.0, rights.0)
            .map(|raw| unsafe { Self::from_raw(raw) })
            .map_err(Status::from)
    }

    pub fn into_raw(mut self) -> u64 {
        let raw = self.0;
        self.0 = 0;
        core::mem::forget(self);
        raw
    }
}

impl AsHandleRef for Handle {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        HandleRef {
            raw: self.0,
            marker: core::marker::PhantomData,
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.0 != 0 {
            let _ = Memory::close(self.0);
        }
    }
}
