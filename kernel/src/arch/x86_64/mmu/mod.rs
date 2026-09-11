use core::sync::atomic::{AtomicU64, Ordering};
static SAFE_ROOT: AtomicU64 = AtomicU64::new(0);
pub fn kernel_root() -> u64 {
    SAFE_ROOT.load(Ordering::Acquire)
}
pub fn restore_kernel_root(root: u64) {
    SAFE_ROOT.store(root, Ordering::Release);
}
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
        if va >= 0x0000_8000_0000_0000 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut table = root;
        for shift in [39, 30, 21] {
            let entry = unsafe { Self::entry(table, ((va >> shift) & 511) as usize) };
            unsafe {
                if *entry == 0 {
                    *entry = table_descriptor(self.frames.allocate(1).ok_or(Status::ErrNoMemory)?);
                }
                if *entry & 128 != 0 {
                    return Err(Status::ErrInvalidArgs);
                }
                table = *entry & TABLE_ADDR_MASK;
            }
        }
        Ok(unsafe { Self::entry(table, ((va >> 12) & 511) as usize) })
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
        // Firmware/AP trampoline and fixed Q35 interrupt/timer registers.
        for pa in (0..0x0100_0000).step_by(4096) {
            let leaf = self.leaf(root, pa)?;
            unsafe {
                *leaf = kernel_page_descriptor(
                    pa,
                    MemoryAttr::Normal,
                    Access::KernelReadWrite,
                    pa == 0x8000,
                );
            }
        }
        for pa in [0xfec0_0000, 0xfed0_0000, 0xfee0_0000] {
            let leaf = self.leaf(root, pa)?;
            unsafe {
                *leaf =
                    kernel_page_descriptor(pa, MemoryAttr::Device, Access::KernelReadWrite, false);
            }
        }
        let text_start = align_down(core::ptr::addr_of!(__kernel_text_start) as u64);
        let text_end = align_up(core::ptr::addr_of!(__kernel_text_end) as u64);
        let rodata_start = align_down(core::ptr::addr_of!(__kernel_rodata_start) as u64);
        let rodata_end = align_up(core::ptr::addr_of!(__kernel_rodata_end) as u64);
        let data_start = align_down(core::ptr::addr_of!(__kernel_data_start) as u64);
        let kernel_end = align_up(core::ptr::addr_of!(__kernel_end) as u64);
        let stack_bottom = align_down(core::ptr::addr_of!(__boot_stacks_bottom) as u64);
        let stack_top = align_up(core::ptr::addr_of!(__boot_stacks_top) as u64);
        let mut pa = bexos_boot::RAM_START;
        while pa < RAM_END {
            if is_kernel_stack_guard_page(pa, stack_bottom, stack_top) {
                pa += 4096;
                continue;
            }
            let (access, executable) = if pa == super::transplant::PARK_CODE {
                (Access::KernelReadOnly, true)
            } else if pa >= text_start && pa < text_end {
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
    fn revoke_shared_pin(&mut self, token: u64) {
        use crate::arch::ArchAPI;
        use bexos_secure_monitor_abi::{PINNED_HANDLE_TAG, Request};
        // The architecture gate refuses VMMCALL when no verified monitor is
        // present. Unregister is idempotent for an already-revoked pin.
        let _ = crate::arch::CurrentArch::invoke_smc(
            Request::Unregister {
                handle: token | PINNED_HANDLE_TAG,
            }
            .encode(),
        );
    }
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
        Some(super::time::monotonic_ns())
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
        if kernel_root() == 0 {
            let safe = self.allocate(1)?;
            self.install_kernel_identity(safe)?;
            SAFE_ROOT.store(safe, Ordering::Release);
        }
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
        let mut desc = page_descriptor(pa, attr, access);
        if rights & 8 != 0 {
            desc &= !(1 << 63);
        }
        unsafe {
            *leaf = desc;
        }
        Ok(())
    }
    fn destroy_space(&mut self, root: u64) {
        let active: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) active, options(nostack));
        }
        if active == root {
            unsafe {
                core::arch::asm!("mov cr3, {}", in(reg) kernel_root(), options(nostack));
            }
        }
        fn release(backend: &mut PhysicalBackend, table: u64, level: u8) {
            if level > 1 {
                for index in 0..512 {
                    let entry = unsafe { *PhysicalBackend::entry(table, index) };
                    if entry & 1 != 0 && entry & 128 == 0 {
                        release(backend, entry & TABLE_ADDR_MASK, level - 1);
                    }
                }
            }
            backend.frames.release(table, 1);
        }
        release(self, root, 4);
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
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) root, options(nostack));
    }
    crate::log_line("kernel: x86 MMU enabled isolated process address spaces");
}
fn invalidate() {
    unsafe {
        core::arch::asm!("mov {root}, cr3", "mov cr3, {root}", root = out(reg) _, options(nostack));
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
