//! DMA buffers whose mappings and ownership survive a driver transplant.
use crate::BexHal;
use core::ptr::NonNull;
use virtio_drivers::{BufferDirection, Hal};

pub struct DmaBuffer {
    pointer: NonNull<u8>,
    physical: u64,
    pages: usize,
    owned: bool,
}

impl DmaBuffer {
    pub fn new(pages: usize) -> Option<Self> {
        if pages == 0 || pages > 16384 {
            return None;
        }
        let (physical, pointer) = BexHal::dma_alloc(pages, BufferDirection::Both);
        if physical == 0 {
            return None;
        }
        Some(Self {
            pointer,
            physical,
            pages,
            owned: true,
        })
    }

    pub fn snapshot(&self) -> [u64; 3] {
        [
            self.physical,
            self.pointer.as_ptr() as u64,
            self.pages as u64,
        ]
    }

    /// Call after the migration receiver has installed the retained DMA map.
    pub fn adopt(words: [u64; 3]) -> Option<Self> {
        let [physical, address, pages] = words;
        if pages == 0
            || pages > 16384
            || !crate::dma_snapshot().iter().any(|allocation| {
                allocation.paddr == physical
                    && allocation.vaddr == address
                    && allocation.size == pages * 4096
            })
        {
            return None;
        }
        Some(Self {
            pointer: NonNull::new(address as *mut u8)?,
            physical,
            pages: pages as usize,
            owned: false,
        })
    }

    pub fn activate(&mut self) {
        self.owned = true;
    }

    /// The caller must serialize CPU access with DMA queue ownership.
    pub unsafe fn bytes(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.pages * 4096) }
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        if self.owned {
            unsafe {
                BexHal::dma_dealloc(self.physical, self.pointer, self.pages);
            }
        }
    }
}
