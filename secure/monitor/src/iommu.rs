//! Q35 VT-d register backend. DMA and interrupt remapping must both report
//! enabled before the caller may enable bus mastering on an assigned device.
//! Register layout: Intel VT-d architecture, legacy root/context mode;
//! QEMU hw/i386/intel_iommu_internal.h provides the emulated capability layout.
use crate::{clock, dma_tables::DmaTables};
const BASE: u64 = 0xfed90000;
unsafe fn read32(offset: u64) -> u32 {
    unsafe { core::ptr::read_volatile((BASE + offset) as *const u32) }
}
unsafe fn read64(offset: u64) -> u64 {
    unsafe { core::ptr::read_volatile((BASE + offset) as *const u64) }
}
unsafe fn write32(offset: u64, value: u32) {
    unsafe { core::ptr::write_volatile((BASE + offset) as *mut u32, value) }
}
unsafe fn write64(offset: u64, value: u64) {
    unsafe { core::ptr::write_volatile((BASE + offset) as *mut u64, value) }
}
unsafe fn wait(offset: u64, mask: u64, expected: u64, wide: bool) -> bool {
    let start = unsafe { clock::now_ns() };
    loop {
        let value = if wide {
            unsafe { read64(offset) }
        } else {
            u64::from(unsafe { read32(offset) })
        };
        if value & mask == expected {
            return true;
        }
        if unsafe { clock::now_ns() }.saturating_sub(start) >= 100_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }
}
/// # Safety
/// Own the Q35 VT-d register page and pinned, initialized tables. All PCI bus
/// masters must be disabled and remain so until this function succeeds. Keep
/// the tables immutable and inaccessible to both guest domains thereafter.
pub unsafe fn initialize(tables: &DmaTables) -> bool {
    unsafe {
        let cap = read64(8);
        let ext = read64(16);
        if read32(0) & 0xff == 0xff
            || cap & (1 << 9) == 0
            || cap & (1 << 34) == 0
            || ext & (1 << 3) == 0
        {
            return false;
        }
        if read32(0x1c) & ((1 << 31) | (1 << 25)) != 0 {
            return false;
        }
        let physical = tables as *const _ as u64;
        if physical & 4095 != 0 {
            return false;
        }
        core::arch::asm!("mfence", options(nostack, preserves_flags));
        write32(0x38, 1 << 31); // Faults are polled, never delivered to guest MSI addresses.
        write64(0x20, physical);
        write32(0x18, 1 << 30);
        if !wait(0x1c, 1 << 30, 1 << 30, false) {
            return false;
        }
        write64(0x28, (1 << 63) | (1 << 61));
        if !wait(0x28, 1 << 63, 0, true) {
            return false;
        }
        let iotlb = ((ext >> 8) & 0x3ff) * 16 + 8;
        if iotlb < 0x80 || iotlb > 0xff8 {
            return false;
        }
        write64(iotlb, (1 << 63) | (1 << 60));
        if !wait(iotlb, 1 << 63, 0, true) {
            return false;
        }
        write64(0xb8, tables.interrupt_table(physical) | 7); // 256 non-present IRTEs.
        write32(0x18, 1 << 24);
        if !wait(0x1c, 1 << 24, 1 << 24, false) {
            return false;
        }
        write32(0x18, (1 << 31) | (1 << 25));
        wait(0x1c, (1 << 31) | (1 << 25), (1 << 31) | (1 << 25), false)
    }
}
/// # Safety
/// Requires an initialized Q35 remapping unit owned by this monitor.
pub unsafe fn fault() -> Option<(u64, u16, u8)> {
    unsafe {
        if read32(0x34) & 2 == 0 {
            return None;
        }
        let offset = ((read64(8) >> 24) & 0x3ff) * 16;
        let high = read64(offset + 8);
        if high & (1 << 63) == 0 {
            return None;
        }
        Some((read64(offset) & !4095, high as u16, (high >> 32) as u8))
    }
}
/// # Safety
/// The monitor has consumed the fault record and owns the remapping unit.
pub unsafe fn clear_fault() {
    unsafe {
        let offset = ((read64(8) >> 24) & 0x3ff) * 16;
        write64(offset + 8, 1 << 63);
        write32(0x34, 1);
    }
}
