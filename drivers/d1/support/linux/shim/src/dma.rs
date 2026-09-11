use crate::error::{LinuxError, Result};
use crate::page::{PAGE_SIZE, PhysAddr, align_up};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaDirection {
    ToDevice,
    FromDevice,
    Bidirectional,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DmaBuffer {
    phys: PhysAddr,
    bytes: Vec<u8>,
    coherent: bool,
}

impl DmaBuffer {
    pub fn new(phys: PhysAddr, size_bytes: usize, coherent: bool) -> Self {
        Self {
            phys,
            bytes: vec![0; size_bytes],
            coherent,
        }
    }

    pub const fn phys(&self) -> PhysAddr {
        self.phys
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    pub const fn is_coherent(&self) -> bool {
        self.coherent
    }
}

pub trait DmaAllocator {
    fn allocate_coherent(&mut self, size_bytes: usize, align: usize) -> Result<DmaBuffer>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MockDmaAllocator {
    next_phys: u64,
}

impl MockDmaAllocator {
    pub const fn new(base_phys: u64) -> Self {
        Self {
            next_phys: base_phys,
        }
    }
}

impl DmaAllocator for MockDmaAllocator {
    fn allocate_coherent(&mut self, size_bytes: usize, align: usize) -> Result<DmaBuffer> {
        if size_bytes == 0 {
            return Err(LinuxError::Invalid);
        }
        let align = align.max(PAGE_SIZE);
        let phys = align_up(self.next_phys as usize, align) as u64;
        let size = align_up(size_bytes, PAGE_SIZE);
        self.next_phys = phys.checked_add(size as u64).ok_or(LinuxError::NoMemory)?;
        Ok(DmaBuffer::new(PhysAddr(phys), size, true))
    }
}
