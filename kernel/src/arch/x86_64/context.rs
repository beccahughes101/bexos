use super::{cpu, frame::TrapFrame, io};
use crate::arch::ArchAPI;
use bexos_kernel_core::runtime::Context;
use core::{arch::asm, cell::UnsafeCell};
#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    base: u64,
}
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Gate {
    low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    mid: u16,
    high: u32,
    reserved: u32,
}
impl Gate {
    const EMPTY: Self = Self {
        low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        mid: 0,
        high: 0,
        reserved: 0,
    };
}
#[repr(C, align(16))]
struct Tables {
    gdt: [u64; 7],
    tss: [u8; 104],
    idt: [Gate; 256],
}
struct Slot(UnsafeCell<Tables>);
unsafe impl Sync for Slot {}
static TABLES: [Slot; 64] = [const {
    Slot(UnsafeCell::new(Tables {
        gdt: [0; 7],
        tss: [0; 104],
        idt: [Gate::EMPTY; 256],
    }))
}; 64];
struct SyscallSlot(UnsafeCell<[u64; 2]>);
unsafe impl Sync for SyscallSlot {}
static SYSCALL_SLOTS: [SyscallSlot; 64] = [const { SyscallSlot(UnsafeCell::new([0; 2])) }; 64];
unsafe extern "C" {
    static __x86_vectors: [u64; 256];
    static __boot_stacks_bottom: u8;
    fn x86_enter_context(context: *const Context) -> !;
    fn __x86_syscall_entry();
}
pub fn install_exception_vectors() {
    let cpu = cpu::current_cpu_id() as usize;
    let tables = unsafe { &mut *TABLES[cpu].0.get() };
    let base = tables.tss.as_ptr() as u64;
    let stack = core::ptr::addr_of!(__boot_stacks_bottom) as u64 + (cpu as u64 + 1) * 266240;
    let syscall_slot = unsafe { &mut *SYSCALL_SLOTS[cpu].0.get() };
    syscall_slot[0] = stack;
    syscall_slot[1] = 0;
    tables.tss[4..12].copy_from_slice(&stack.to_le_bytes());
    let emergency = stack - 240 * 1024;
    tables.tss[36..44].copy_from_slice(&emergency.to_le_bytes());
    tables.tss[102..104].copy_from_slice(&104u16.to_le_bytes());
    tables.gdt = [
        0,
        0x00af9a000000ffff,
        0x00cf92000000ffff,
        0x00cff2000000ffff,
        0x00affa000000ffff,
        103 | ((base & 0xffffff) << 16) | (0x89 << 40) | (((base >> 24) & 255) << 56),
        base >> 32,
    ];
    for (i, gate) in tables.idt.iter_mut().enumerate() {
        let entry = unsafe { __x86_vectors[i] };
        *gate = Gate {
            low: entry as u16,
            selector: 8,
            ist: if i == 8 || i == 2 { 1 } else { 0 },
            attributes: if i == 128 { 0xee } else { 0x8e },
            mid: (entry >> 16) as u16,
            high: (entry >> 32) as u32,
            reserved: 0,
        };
    }
    let gdt = Descriptor {
        limit: 55,
        base: tables.gdt.as_ptr() as u64,
    };
    let idt = Descriptor {
        limit: 4095,
        base: tables.idt.as_ptr() as u64,
    };
    unsafe {
        asm!("lgdt [{gdt}]", "push 8", "lea rax, [rip + 2f]", "push rax", "retfq", "2:", "mov ax, 16", "mov ds, ax", "mov es, ax", "mov ss, ax", "mov ax, 40", "ltr ax", "lidt [{idt}]", gdt = in(reg) &gdt, idt = in(reg) &idt, out("rax") _);
        io::wrmsr(0xc000_0102, syscall_slot.as_ptr() as u64);
        io::wrmsr(0xc000_0081, (8u64 << 32) | (0x13u64 << 48));
        io::wrmsr(0xc000_0082, __x86_syscall_entry as *const () as u64);
        io::wrmsr(0xc000_0084, (1 << 8) | (1 << 9) | (1 << 10));
        io::wrmsr(0xc000_0080, io::rdmsr(0xc000_0080) | 1);
    }
}
pub unsafe fn enter_context(context: *const Context) -> ! {
    assert!(unsafe { (*context).matches_current_architecture() });
    unsafe { x86_enter_context(context) }
}
pub fn fault_address() -> u64 {
    let address;
    unsafe {
        asm!("mov {}, cr2", out(reg) address, options(nomem, nostack));
    }
    address
}
pub fn is_userspace(context: &Context) -> bool {
    TrapFrame::view(context).cs & 3 == 3
}
#[unsafe(no_mangle)]
pub extern "C" fn x86_handle_trap(frame: *mut TrapFrame, vector: u64, error: u64) {
    let frame = frame.cast::<Context>();
    let context = unsafe { &mut *frame };
    assert!(context.matches_current_architecture());
    let restricted = crate::userspace::RUNTIME.with(|s| {
        s.as_ref()
            .is_some_and(|runtime| runtime.restricted_is_active())
    });
    if vector == 129 && is_userspace(context) && restricted {
        if let Some(host) = crate::userspace::RUNTIME.with(|s| {
            s.as_mut().and_then(|runtime| {
                runtime
                    .restricted_exit_current(bexos_restricted_abi::Reason::Syscall, 0, 0, *context)
                    .ok()
            })
        }) {
            *context = host;
        }
    } else if vector == 128 && is_userspace(context) && restricted {
        // INT instructions report the following RIP; exception reflection
        // exposes the instruction that caused the exit.
        context.instruction_pointer = context.instruction_pointer.saturating_sub(2);
        if let Some(host) = crate::userspace::RUNTIME.with(|s| {
            s.as_mut().and_then(|runtime| {
                runtime
                    .restricted_exit_current(
                        bexos_restricted_abi::Reason::Exception,
                        (vector << 32) | error,
                        0,
                        *context,
                    )
                    .ok()
            })
        }) {
            *context = host;
        }
    } else if vector == 128 && is_userspace(context) {
        crate::syscall::dispatch(frame, TrapFrame::view(context).rax as u32);
    } else if vector >= 32 {
        super::interrupts::dispatch(frame, vector);
    } else if vector == 14
        && error & 6 == 6
        && is_userspace(context)
        && crate::userspace::RUNTIME.with(|s| {
            s.as_mut()
                .is_some_and(|rt| rt.commit_user_write_fault(fault_address()).is_ok())
        })
    {
    } else if vector == 129 && is_userspace(context) {
        crate::log_line("guest fault: native syscall instruction outside restricted mode");
        crate::userspace::RUNTIME.with(|s| s.as_mut().unwrap().exit_current());
        crate::sched::timer_tick(frame);
    } else if is_userspace(context) && restricted {
        if let Some(host) = crate::userspace::RUNTIME.with(|s| {
            s.as_mut().and_then(|runtime| {
                runtime
                    .restricted_exit_current(
                        bexos_restricted_abi::Reason::Exception,
                        (vector << 32) | error,
                        (vector == 14).then(fault_address).unwrap_or(0),
                        *context,
                    )
                    .ok()
            })
        }) {
            *context = host;
        }
    } else if is_userspace(context) {
        crate::log_line(&alloc::format!(
            "guest fault vector={vector} error={error:x} pc={:x} address={:x}",
            context.instruction_pointer,
            fault_address()
        ));
        crate::userspace::RUNTIME.with(|s| s.as_mut().unwrap().exit_current());
        crate::sched::timer_tick(frame);
    } else if crate::transplant::prepare::PREPARE_RETURN_SP
        .load(core::sync::atomic::Ordering::Acquire)
        != 0
    {
        use crate::transplant::prepare::*;
        use core::sync::atomic::Ordering;
        PREPARE_FAULT_ESR.store((vector << 32) | error, Ordering::Release);
        PREPARE_FAULT_ELR.store(context.instruction_pointer, Ordering::Release);
        PREPARE_FAULT_FAR.store(fault_address(), Ordering::Release);
        unsafe extern "C" {
            fn kernel_prepare_failure();
        }
        super::X86_64::prepare_failure(context, kernel_prepare_failure as *const () as u64);
    } else {
        panic!(
            "kernel exception vector={vector} error={error:x} pc={:x} address={:x}",
            context.instruction_pointer,
            fault_address()
        );
    }
}
pub fn switch_address_space(space: bexos_kernel_core::runtime::AddressSpaceSwitch) {
    unsafe {
        asm!("mov cr3, {}", in(reg) if space.root_table_phys == 0 { super::mmu::kernel_root() } else { space.root_table_phys }, options(nostack));
    }
}
pub fn jump_to_replacement(entry: u64, handoff: u64) -> ! {
    unsafe {
        asm!("cli", "jmp rax", in("rax") entry, in("rdi") handoff, options(noreturn));
    }
}
pub fn debug_break() {
    unsafe {
        asm!("int3", options(nostack));
    }
}
pub fn wait_for_interrupt() {
    unsafe {
        asm!("hlt", options(nomem, nostack));
    }
}
pub fn enable_interrupts() {
    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}
pub fn set_kernel_thread_pointer(_cpu_id: u64) { /* Logical CPU is resolved from the APIC ID; FS belongs to userspace. */
}
pub fn synchronize_code(_start: u64, _len: u64) {
    unsafe {
        asm!("mfence", options(nostack));
        let _ = core::arch::x86_64::__cpuid(0);
    }
}

/// Park with descriptor tables and emergency stacks outside both kernel images.
/// Unexpected NMIs or machine checks stay in the reserved halt loop.
pub fn install_parking_vectors() {
    let cpu = cpu::current_cpu_id();
    let slot = super::transplant::PARK_TABLES + cpu * super::transplant::PARK_CPU_BYTES;
    assert!(core::mem::size_of::<Tables>() <= 8192);
    let tables = unsafe { &mut *(slot as *mut Tables) };
    unsafe {
        core::ptr::write_bytes(tables as *mut Tables, 0, 1);
    }
    let base = tables.tss.as_ptr() as u64;
    let stack = slot + super::transplant::PARK_CPU_BYTES;
    tables.tss[4..12].copy_from_slice(&stack.to_le_bytes());
    tables.tss[36..44].copy_from_slice(&stack.to_le_bytes());
    tables.tss[102..104].copy_from_slice(&104u16.to_le_bytes());
    tables.gdt = [
        0,
        0x00af9a000000ffff,
        0x00cf92000000ffff,
        0,
        0,
        103 | ((base & 0xffffff) << 16) | (0x89 << 40) | (((base >> 24) & 255) << 56),
        base >> 32,
    ];
    let entry = super::transplant::PARK_CODE + 9;
    for gate in &mut tables.idt {
        *gate = Gate {
            low: entry as u16,
            selector: 8,
            ist: 1,
            attributes: 0x8e,
            mid: (entry >> 16) as u16,
            high: (entry >> 32) as u32,
            reserved: 0,
        };
    }
    let gdt = Descriptor {
        limit: 55,
        base: tables.gdt.as_ptr() as u64,
    };
    let idt = Descriptor {
        limit: 4095,
        base: tables.idt.as_ptr() as u64,
    };
    unsafe {
        asm!("cli", "lgdt [{gdt}]", "mov ax, 40", "ltr ax", "lidt [{idt}]",
        gdt = in(reg) &gdt, idt = in(reg) &idt, out("rax") _, options(nostack));
    }
}
