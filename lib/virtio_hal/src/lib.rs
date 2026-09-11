#![no_std]
extern crate alloc;
pub mod buffer;

use core::cell::UnsafeCell;
use core::ptr::NonNull;

use alloc::vec::Vec;
use bexos_userspace::Memory;
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

const PAGE_SIZE: u64 = 4096;
const VMO_FLAG_CONTIGUOUS_PHYS: u32 = 0x0000_0002;
const VMO_FLAG_CACHE_POLICY_UC: u32 = 0x0000_0008;
const DMA_RIGHTS_READ_WRITE: u32 = 2 | 4;
const MAX_DMA_ALLOCS: usize = 64;
const MAX_SHARED_RANGES: usize = 64;
const MAX_MMIO_MAPPINGS: usize = 32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DmaAllocation {
    pub paddr: u64,
    pub vaddr: u64,
    pub size: u64,
    pub handle: u64,
    pub token: u64,
    pub active: bool,
    pub owned: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SharedRange {
    pub vaddr: u64,
    pub paddr: u64,
    pub size: u64,
    pub vmo: u64,
    pub token: u64,
    pub active: bool,
    pub owned: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MmioMapping {
    pub paddr: u64,
    pub vaddr: u64,
    pub size: u64,
    pub handle: u64,
    pub active: bool,
    pub owned: bool,
}

struct Registry<T, const N: usize>(UnsafeCell<[T; N]>);

unsafe impl<T, const N: usize> Sync for Registry<T, N> {}

static DMA_ALLOCS: Registry<DmaAllocation, MAX_DMA_ALLOCS> = Registry(UnsafeCell::new(
    [DmaAllocation {
        paddr: 0,
        vaddr: 0,
        size: 0,
        handle: 0,
        token: 0,
        active: false,
        owned: false,
    }; MAX_DMA_ALLOCS],
));

static SHARED_RANGES: Registry<SharedRange, MAX_SHARED_RANGES> = Registry(UnsafeCell::new(
    [SharedRange {
        vaddr: 0,
        paddr: 0,
        size: 0,
        vmo: 0,
        token: 0,
        active: false,
        owned: false,
    }; MAX_SHARED_RANGES],
));

static MMIO_MAPPINGS: Registry<MmioMapping, MAX_MMIO_MAPPINGS> = Registry(UnsafeCell::new(
    [MmioMapping {
        paddr: 0,
        vaddr: 0,
        size: 0,
        handle: 0,
        active: false,
        owned: false,
    }; MAX_MMIO_MAPPINGS],
));

static IOMMU_DOMAIN: Registry<u64, 1> = Registry(UnsafeCell::new([0; 1]));

pub struct BexHal;

pub fn set_iommu_domain(domain: u64) {
    unsafe {
        (*IOMMU_DOMAIN.0.get())[0] = domain;
    }
}

fn iommu_domain() -> Option<u64> {
    let domain = unsafe { (*IOMMU_DOMAIN.0.get())[0] };
    (domain != 0).then_some(domain)
}

pub fn iommu_domain_handle() -> Option<u64> {
    iommu_domain()
}

pub fn register_shared_range(vmo: u64, vaddr: u64, paddr: u64, token: u64, size: u64) -> bool {
    with_shared_ranges(|ranges| {
        if let Some(slot) = ranges.iter_mut().find(|range| !range.active) {
            *slot = SharedRange {
                vaddr,
                paddr,
                size,
                vmo,
                token,
                active: true,
                owned: true,
            };
            true
        } else {
            false
        }
    })
}

pub fn unregister_shared_range(vmo: u64) -> Option<(u64, u64, u64)> {
    with_shared_ranges(|ranges| {
        let range = ranges
            .iter_mut()
            .find(|range| range.active && range.vmo == vmo)?;
        range.active = false;
        Some((range.vaddr, range.token, range.size))
    })
}

pub fn dma_snapshot() -> Vec<DmaAllocation> {
    with_dma_allocs(|allocs| {
        allocs
            .iter()
            .copied()
            .filter(|entry| entry.active)
            .collect()
    })
}

pub fn shared_range_snapshot() -> Vec<SharedRange> {
    with_shared_ranges(|ranges| {
        ranges
            .iter()
            .copied()
            .filter(|entry| entry.active)
            .collect()
    })
}

pub fn mmio_snapshot() -> Vec<MmioMapping> {
    with_mmio_mappings(|mappings| {
        mappings
            .iter()
            .copied()
            .filter(|entry| entry.active)
            .collect()
    })
}

pub fn adopt_dma_snapshot(snapshot: &[DmaAllocation]) -> bool {
    if snapshot.len() > MAX_DMA_ALLOCS
        || snapshot.iter().any(|entry| {
            !entry.active
                || entry.paddr == 0
                || entry.vaddr == 0
                || entry.size == 0
                || entry.handle == 0
                || entry.token == 0
        })
    {
        return false;
    }
    with_dma_allocs(|allocs| {
        allocs.fill(DmaAllocation::default());
        for (slot, mut entry) in allocs.iter_mut().zip(snapshot.iter().copied()) {
            entry.owned = false;
            *slot = entry;
        }
        true
    })
}

pub fn adopt_shared_range_snapshot(snapshot: &[SharedRange]) -> bool {
    if snapshot.len() > MAX_SHARED_RANGES
        || snapshot.iter().any(|entry| {
            !entry.active
                || entry.vaddr == 0
                || entry.paddr == 0
                || entry.size == 0
                || entry.vmo == 0
                || entry.token == 0
        })
    {
        return false;
    }
    with_shared_ranges(|ranges| {
        ranges.fill(SharedRange::default());
        for (slot, mut entry) in ranges.iter_mut().zip(snapshot.iter().copied()) {
            entry.owned = false;
            *slot = entry;
        }
        true
    })
}

pub fn adopt_mmio_snapshot(snapshot: &[MmioMapping]) -> bool {
    if snapshot.len() > MAX_MMIO_MAPPINGS
        || snapshot.iter().any(|entry| {
            !entry.active
                || entry.paddr == 0
                || entry.vaddr == 0
                || entry.size == 0
                || entry.handle == 0
        })
    {
        return false;
    }
    with_mmio_mappings(|mappings| {
        mappings.fill(MmioMapping::default());
        for (slot, mut entry) in mappings.iter_mut().zip(snapshot.iter().copied()) {
            entry.owned = false;
            *slot = entry;
        }
        true
    })
}

pub fn activate_adopted() {
    with_dma_allocs(|allocs| {
        for allocation in allocs.iter_mut().filter(|entry| entry.active) {
            allocation.owned = true;
        }
    });
    with_shared_ranges(|ranges| {
        for range in ranges.iter_mut().filter(|entry| entry.active) {
            range.owned = true;
        }
    });
    with_mmio_mappings(|mappings| {
        for mapping in mappings.iter_mut().filter(|entry| entry.active) {
            mapping.owned = true;
        }
    });
}

unsafe impl Hal for BexHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let Some(size) = (pages as u64).checked_mul(PAGE_SIZE) else {
            return null_dma();
        };
        let Ok(handle) = Memory::create(size, VMO_FLAG_CONTIGUOUS_PHYS | VMO_FLAG_CACHE_POLICY_UC)
        else {
            return null_dma();
        };
        let Ok(vaddr) = Memory::map(handle, size, DMA_RIGHTS_READ_WRITE) else {
            let _ = Memory::close(handle);
            return null_dma();
        };
        let Some(domain) = iommu_domain() else {
            let _ = Memory::unmap(vaddr, size);
            let _ = Memory::close(handle);
            return null_dma();
        };
        let Ok((paddr, token)) = Memory::map_dma(domain, handle, 0, size, 2 | 4) else {
            let _ = Memory::unmap(vaddr, size);
            let _ = Memory::close(handle);
            return null_dma();
        };
        unsafe {
            core::ptr::write_bytes(vaddr as *mut u8, 0, size as usize);
        }
        if !with_dma_allocs(|allocs| {
            if let Some(slot) = allocs.iter_mut().find(|allocation| !allocation.active) {
                *slot = DmaAllocation {
                    paddr,
                    vaddr,
                    size,
                    handle,
                    token,
                    active: true,
                    owned: true,
                };
                true
            } else {
                false
            }
        }) {
            let _ = Memory::unmap_dma(token);
            let _ = Memory::unmap(vaddr, size);
            let _ = Memory::close(handle);
            return null_dma();
        }
        (paddr, NonNull::new(vaddr as *mut u8).unwrap())
    }

    unsafe fn dma_dealloc(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        let Some(size) = (pages as u64).checked_mul(PAGE_SIZE) else {
            return -1;
        };
        let removed = with_dma_allocs(|allocs| {
            let allocation = allocs.iter_mut().find(|allocation| {
                allocation.active
                    && allocation.paddr == paddr
                    && allocation.vaddr == vaddr.as_ptr() as u64
                    && allocation.size == size
            })?;
            allocation.active = false;
            if !allocation.owned {
                return Some(None);
            }
            Some(Some(*allocation))
        });
        let Some(removed) = removed else {
            return -1;
        };
        let Some(allocation) = removed else {
            return 0;
        };
        let status = Memory::unmap_dma(allocation.token)
            .and_then(|_| Memory::unmap(allocation.vaddr, allocation.size))
            .and_then(|_| Memory::close(allocation.handle));
        if status.is_ok() { 0 } else { -1 }
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        if let Some(vaddr) = with_mmio_mappings(|mappings| {
            mappings.iter().find_map(|mapping| {
                physical_for_range(
                    mapping.paddr,
                    mapping.vaddr,
                    mapping.size,
                    paddr,
                    size as u64,
                )
            })
        }) {
            return NonNull::new(vaddr as *mut u8).unwrap();
        }

        let rounded = page_round(size as u64).unwrap_or(PAGE_SIZE);
        let handle = Memory::physical(paddr, rounded).expect("virtio-net MMIO physical VMO");
        let vaddr =
            Memory::map(handle, rounded, DMA_RIGHTS_READ_WRITE).expect("virtio-net MMIO map");
        with_mmio_mappings(|mappings| {
            if let Some(slot) = mappings.iter_mut().find(|mapping| !mapping.active) {
                *slot = MmioMapping {
                    paddr,
                    vaddr,
                    size: rounded,
                    handle,
                    active: true,
                    owned: true,
                };
            }
        });
        NonNull::new(vaddr as *mut u8).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        let ptr = buffer.as_ptr() as *mut u8 as u64;
        let len = unsafe { buffer.as_ref().len() as u64 };
        with_dma_allocs(|allocs| {
            allocs.iter().find_map(|allocation| {
                physical_for_range(
                    allocation.vaddr,
                    allocation.paddr,
                    allocation.size,
                    ptr,
                    len,
                )
            })
        })
        .or_else(|| {
            with_shared_ranges(|ranges| {
                ranges.iter().find_map(|range| {
                    physical_for_range(range.vaddr, range.paddr, range.size, ptr, len)
                })
            })
        })
        .unwrap_or(0)
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}

fn null_dma() -> (PhysAddr, NonNull<u8>) {
    (0, NonNull::dangling())
}

fn physical_for_range(
    base_vaddr: u64,
    base_paddr: u64,
    size: u64,
    ptr: u64,
    len: u64,
) -> Option<u64> {
    if ptr >= base_vaddr && ptr.checked_add(len)? <= base_vaddr.checked_add(size)? {
        Some(base_paddr + (ptr - base_vaddr))
    } else {
        None
    }
}

fn page_round(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|n| n & !(PAGE_SIZE - 1))
}

fn with_dma_allocs<R>(f: impl FnOnce(&mut [DmaAllocation; MAX_DMA_ALLOCS]) -> R) -> R {
    let slots = unsafe { &mut *DMA_ALLOCS.0.get() };
    f(slots)
}

fn with_shared_ranges<R>(f: impl FnOnce(&mut [SharedRange; MAX_SHARED_RANGES]) -> R) -> R {
    let slots = unsafe { &mut *SHARED_RANGES.0.get() };
    f(slots)
}

fn with_mmio_mappings<R>(f: impl FnOnce(&mut [MmioMapping; MAX_MMIO_MAPPINGS]) -> R) -> R {
    let slots = unsafe { &mut *MMIO_MAPPINGS.0.get() };
    f(slots)
}

pub mod migration;
