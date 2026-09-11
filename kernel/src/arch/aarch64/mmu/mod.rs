use crate::memory::Frames;
use bexos_boot::RAM_END;
use bexos_kernel_core::mmu::*;
use bexos_kernel_core::runtime::{Backend, Result};
use kernel_fidl::Status;
pub mod transplant;
mod zero;
pub struct PhysicalBackend {
    pub frames: Frames,
}

unsafe extern "C" {
    static __kernel_text_start: u8;
    static __kernel_text_end: u8;
    static __kernel_rodata_start: u8;
    static __kernel_rodata_end: u8;
    static __kernel_data_start: u8;
    static __kernel_end: u8;
    static __boot_stacks_bottom: u8;
    static __boot_stacks_top: u8;
}

impl PhysicalBackend {
    unsafe fn entry(root: u64, index: usize) -> *mut u64 {
        (root as *mut u64).wrapping_add(index)
    }
    fn leaf(&mut self, root: u64, va: u64) -> Result<*mut u64> {
        unsafe {
            let l1 = Self::entry(root, ((va >> 30) & 511) as usize);
            if *l1 == 0 {
                *l1 = table_descriptor(self.frames.allocate(1).ok_or(Status::ErrNoMemory)?);
            }
            let l2 = Self::entry(*l1 & TABLE_ADDR_MASK, ((va >> 21) & 511) as usize);
            if *l2 == 0 {
                *l2 = table_descriptor(self.frames.allocate(1).ok_or(Status::ErrNoMemory)?);
            }
            Ok(Self::entry(
                *l2 & TABLE_ADDR_MASK,
                ((va >> 12) & 511) as usize,
            ))
        }
    }

    fn map_kernel_identity_page(
        &mut self,
        root: u64,
        pa: u64,
        access: Access,
        executable: bool,
    ) -> Result<()> {
        let leaf = self.leaf(root, pa)?;
        unsafe {
            *leaf = kernel_page_descriptor(pa, MemoryAttr::Normal, access, executable);
        }
        Ok(())
    }

    fn install_kernel_identity(&mut self, root: u64) -> Result<()> {
        unsafe {
            *Self::entry(root, 0) =
                block_descriptor_1g(0, MemoryAttr::Device, Access::KernelReadWrite);
        }
        let text_start = align_down(core::ptr::addr_of!(__kernel_text_start) as u64);
        let text_end = align_up(core::ptr::addr_of!(__kernel_text_end) as u64);
        let rodata_start = align_down(core::ptr::addr_of!(__kernel_rodata_start) as u64);
        let rodata_end = align_up(core::ptr::addr_of!(__kernel_rodata_end) as u64);
        let data_start = align_down(core::ptr::addr_of!(__kernel_data_start) as u64);
        let kernel_end = align_up(core::ptr::addr_of!(__kernel_end) as u64);
        let stack_bottom = align_down(core::ptr::addr_of!(__boot_stacks_bottom) as u64);
        let stack_top = align_up(core::ptr::addr_of!(__boot_stacks_top) as u64);
        let mut pa = 0x4000_0000;
        while pa < RAM_END {
            if is_kernel_stack_guard_page(pa, stack_bottom, stack_top) {
                pa += 4096;
                continue;
            }
            let (access, executable) = if pa >= text_start && pa < text_end {
                (Access::KernelReadOnly, true)
            } else if pa >= rodata_start && pa < rodata_end {
                (Access::KernelReadOnly, false)
            } else if pa >= data_start && pa < kernel_end {
                (Access::KernelReadWrite, false)
            } else {
                (Access::KernelReadWrite, false)
            };
            self.map_kernel_identity_page(root, pa, access, executable)?;
            pa += 4096;
        }
        Ok(())
    }
}
impl Backend for PhysicalBackend {
    fn trace_retirement(&self, process: usize, elapsed_ms: [u64; 4]) {
        crate::log_line(&alloc::format!(
            "service-transplant: retirement detached process={process} handles_ms={} mappings_ms={} resources_ms={} tables_ms={}",
            elapsed_ms[0],
            elapsed_ms[1],
            elapsed_ms[2],
            elapsed_ms[3]
        ));
    }
    fn monotonic_ms(&self) -> Option<u64> {
        Some(crate::migration::now_ms())
    }
    fn monotonic_ns(&self) -> Option<u64> {
        Some(crate::arch::aarch64::monotonic_ns())
    }
    fn allocate(&mut self, pages: u64) -> Result<u64> {
        self.frames.allocate(pages).ok_or(Status::ErrNoMemory)
    }
    fn release(&mut self, base: u64, pages: u64) {
        self.frames.release(base, pages);
    }
    fn new_space(&mut self) -> Result<u64> {
        let root = self.allocate(1)?;
        self.install_kernel_identity(root)?;
        transplant::install_staged_code(self, root)?;
        Ok(root)
    }
    fn map_page(&mut self, root: u64, va: u64, pa: u64, rights: u32, device: bool) -> Result<()> {
        let leaf = self.leaf(root, va)?;
        let access = if rights & 4 != 0 {
            Access::KernelUserReadWrite
        } else {
            Access::KernelUserReadOnly
        };
        let attr = if device {
            MemoryAttr::Device
        } else {
            MemoryAttr::Normal
        };
        let mut desc = page_descriptor(pa, attr, access) | (1 << 53);
        if rights & 8 == 0 {
            desc |= 1 << 54;
        }
        unsafe {
            *leaf = desc;
        }
        Ok(())
    }
    fn destroy_space(&mut self, root: u64) {
        // Entries 0/1 are the kernel's 1-GiB identity block mappings, not
        // allocated child tables. Only table descriptors own table pages.
        unsafe {
            for i in 0..512 {
                let l1 = *Self::entry(root, i);
                if l1 & 3 != 3 {
                    continue;
                }
                let l2 = l1 & TABLE_ADDR_MASK;
                for j in 0..512 {
                    let entry = *Self::entry(l2, j);
                    if entry & 3 == 3 {
                        self.frames.release(entry & TABLE_ADDR_MASK, 1);
                    }
                }
                self.frames.release(l2, 1);
            }
        }
        self.frames.release(root, 1);
    }
    fn unmap_page(&mut self, root: u64, va: u64) {
        if let Ok(leaf) = self.leaf(root, va) {
            unsafe {
                *leaf = 0;
            }
        }
    }
    fn flush_mappings(&mut self) {
        invalidate();
    }
    fn flush_page(&mut self, _asid: u16, _va: u64) {
        // A write fault replaces the shared read-only zero page.  Keep this
        // conservative until targeted invalidation has been validated on all
        // supported cores; a stale entry otherwise traps on the same store
        // forever and prevents the scheduler timer from making progress.
        invalidate();
    }
    fn read(&self, pa: u64, out: &mut [u8]) {
        unsafe {
            core::ptr::copy_nonoverlapping(pa as *const u8, out.as_mut_ptr(), out.len());
        }
    }
    fn write(&mut self, pa: u64, bytes: &[u8]) {
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), pa as *mut u8, bytes.len());
        }
    }
    fn zero(&mut self, pa: u64, bytes: u64) {
        // Runtime reclamation calls this only for owned normal RAM.
        unsafe {
            zero::normal_ram(pa, bytes);
        }
    }
    fn free_pages(&self) -> u64 {
        self.frames.free_pages()
    }
    fn reused_pages(&self) -> u64 {
        self.frames.reused()
    }
}
pub fn init(root: u64) {
    crate::arch::aarch64::configure_mmu(
        root,
        0xff00,
        (25 << 0) | (1 << 8) | (1 << 10) | (3 << 12) | (1 << 36),
    );
    crate::log_line("kernel: mmu enabled isolated process address spaces");
}
fn invalidate() {
    unsafe {
        core::arch::asm!("dsb ishst; tlbi vmalle1; dsb ish; isb", options(nostack));
    }
}

const fn align_down(value: u64) -> u64 {
    value & !(4096 - 1)
}

const fn align_up(value: u64) -> u64 {
    (value + 4095) & !(4096 - 1)
}

fn is_kernel_stack_guard_page(pa: u64, stack_bottom: u64, stack_top: u64) -> bool {
    const PER_CPU_STACK_STRIDE: u64 = 260 * 1024;
    if pa < stack_bottom || pa >= stack_top {
        return false;
    }
    (pa - stack_bottom) % PER_CPU_STACK_STRIDE == 0
}
