//! Retire the loader's address space before clearing either guest RAM bank.
//! In particular the development Multiboot loader can reside in BexOS RAM.
use core::arch::asm;
#[repr(C, align(4096))]
struct Table([u64; 512]);
#[unsafe(link_section = ".resident.root.pml4")]
static mut PML4: Table = Table([0; 512]);
#[unsafe(link_section = ".resident.root.pdpt")]
static mut PDPT: Table = Table([0; 512]);
#[unsafe(link_section = ".resident.root.pd")]
#[unsafe(no_mangle)]
static mut PD: [Table; 4] = [const { Table([0; 512]) }; 4];
#[unsafe(link_section = ".resident.root.gdt")]
static GDT: [u64; 3] = [0, 0x00af9a000000ffff, 0x00cf92000000ffff];
#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    base: u64,
}

/// All monitor code, stacks, guest banks and assigned Q35 MMIO are below 4GiB.
/// These tables are host-only; each guest retains its restrictive NPT root.
pub unsafe fn initialize() {
    unsafe {
        let pml4 = &mut *core::ptr::addr_of_mut!(PML4);
        let pdpt = &mut *core::ptr::addr_of_mut!(PDPT);
        let pd = &mut *core::ptr::addr_of_mut!(PD);
        pml4.0[0] = core::ptr::addr_of!(PDPT) as u64 | 3;
        for (index, table) in pd.iter_mut().enumerate() {
            pdpt.0[index] = table as *mut _ as u64 | 3;
            for (page, entry) in table.0.iter_mut().enumerate() {
                *entry = ((index * 512 + page) as u64 * 0x200000) | 0x83;
            }
        }
        let gdt = Descriptor {
            limit: (core::mem::size_of_val(&GDT) - 1) as u16,
            base: GDT.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &gdt, options(readonly, nostack));
        asm!("mov cr3, {}", in(reg) core::ptr::addr_of!(PML4) as u64, options(nostack));
    }
}

/// Switch the one replaceable policy page and retain a private alias to the
/// previous bank. Execution, stacks and hardware owners remain in the nucleus.
#[cfg(feature = "resident_nucleus")]
pub unsafe fn map_policy(physical: usize, previous: usize) {
    assert!(matches!(physical, 0x04000000 | 0x60000000));
    assert!(matches!(previous, 0x04000000 | 0x60000000));
    unsafe {
        (*core::ptr::addr_of_mut!(PD))[0].0[32] = physical as u64 | 0x83;
        (*core::ptr::addr_of_mut!(PD))[2].0[0] = previous as u64 | 0x83;
        asm!("mov rax, cr3", "mov cr3, rax", out("rax") _, options(nostack));
    }
}
