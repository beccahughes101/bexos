use crate::dma::DmaBuffer;
use crate::error::{LinuxError, Result};
use crate::mmio::Mmio;
use crate::page::PhysAddr;

pub const RIGHTS_READ: u32 = 2;
pub const RIGHTS_READ_WRITE: u32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappedMmio {
    base: u64,
}

impl MappedMmio {
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    pub const fn base(self) -> u64 {
        self.base
    }

    fn addr(self, offset: usize) -> Result<u64> {
        self.base
            .checked_add(offset as u64)
            .ok_or(LinuxError::Invalid)
    }
}

impl Mmio for MappedMmio {
    fn read32(&self, offset: usize) -> Result<u32> {
        let addr = self.addr(offset)?;
        Ok(unsafe { core::ptr::read_volatile(addr as *const u32) })
    }

    fn write32(&mut self, offset: usize, value: u32) -> Result<()> {
        let addr = self.addr(offset)?;
        unsafe {
            core::ptr::write_volatile(addr as *mut u32, value);
        }
        Ok(())
    }

    fn read64(&self, offset: usize) -> Result<u64> {
        let addr = self.addr(offset)?;
        Ok(unsafe { core::ptr::read_volatile(addr as *const u64) })
    }

    fn write64(&mut self, offset: usize, value: u64) -> Result<()> {
        let addr = self.addr(offset)?;
        unsafe {
            core::ptr::write_volatile(addr as *mut u64, value);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct PinnedDmaPage {
    handle: u64,
    va: u64,
    phys: PhysAddr,
    token: u64,
    owned: bool,
}

impl PinnedDmaPage {
    pub fn new() -> Result<Self> {
        let handle = bexos_userspace::Memory::create(crate::page::PAGE_SIZE as u64, 2)
            .map_err(map_status)?;
        let va =
            bexos_userspace::Memory::map(handle, crate::page::PAGE_SIZE as u64, RIGHTS_READ_WRITE)
                .map_err(map_status)?;
        let (phys, token) = bexos_userspace::Memory::pin(handle).map_err(map_status)?;
        Ok(Self {
            handle,
            va,
            phys: PhysAddr(phys),
            token,
            owned: true,
        })
    }

    pub fn adopt(handle: u64, va: u64, phys: u64, token: u64) -> Result<Self> {
        if handle == 0
            || token == 0
            || va % crate::page::PAGE_SIZE as u64 != 0
            || phys % crate::page::PAGE_SIZE as u64 != 0
        {
            return Err(LinuxError::Invalid);
        }
        Ok(Self {
            handle,
            va,
            phys: PhysAddr(phys),
            token,
            owned: false,
        })
    }

    pub const fn handle(&self) -> u64 {
        self.handle
    }

    pub const fn va(&self) -> u64 {
        self.va
    }

    pub const fn phys(&self) -> PhysAddr {
        self.phys
    }

    pub const fn token(&self) -> u64 {
        self.token
    }

    pub fn activate(&mut self) {
        self.owned = true;
    }

    pub fn as_dma_buffer(&self, size_bytes: usize) -> DmaBuffer {
        DmaBuffer::new(self.phys, size_bytes, true)
    }
}

impl Drop for PinnedDmaPage {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let _ = bexos_userspace::Memory::unpin(self.token);
        let _ = bexos_userspace::Memory::unmap(self.va, crate::page::PAGE_SIZE as u64);
        let _ = bexos_userspace::Memory::close(self.handle);
    }
}

fn map_status(status: kernel_fidl::Status) -> LinuxError {
    match status {
        kernel_fidl::Status::ErrNoMemory | kernel_fidl::Status::ErrResourceExhausted => {
            LinuxError::NoMemory
        }
        kernel_fidl::Status::ErrTimedOut => LinuxError::Timeout,
        kernel_fidl::Status::ErrInvalidHandle | kernel_fidl::Status::ErrInvalidArgs => {
            LinuxError::Invalid
        }
        _ => LinuxError::Io,
    }
}
