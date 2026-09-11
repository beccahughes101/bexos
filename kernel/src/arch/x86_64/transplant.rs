//! CPU0 takeover and idle-secondary parking outside the reclaimed image.
use super::{cpu, io};
use bexos_kernel_core::transplant::{Aarch64CpuContextRecord, Aarch64SystemRegisters};
use core::{
    arch::asm,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
static PARK_REQUESTED: AtomicBool = AtomicBool::new(false);
pub const PARK_CODE: u64 = bexos_boot::HANDOFF_ADDR + 8192;
const PARK_ACK: u64 = bexos_boot::HANDOFF_ADDR + 12288;
// This arena is below the initial kernel image and never enters the frame pool.
pub const PARK_TABLES: u64 = 0x0110_0000;
pub const PARK_CPU_BYTES: u64 = 32768;
fn parked() -> &'static AtomicU64 {
    unsafe { &*(PARK_ACK as *const AtomicU64) }
}
pub fn initialize_parking_trampoline() {
    // cli; mov cr3,rcx; lock bts [rsi],rdi; hlt; jmp hlt.
    // CR3, GDT, IDT, TSS and emergency stacks survive old-image reclamation.
    let code = [
        0xfa, 0x0f, 0x22, 0xd9, 0xf0, 0x48, 0x0f, 0xab, 0x3e, 0xf4, 0xeb, 0xfd,
    ];
    unsafe {
        core::ptr::copy_nonoverlapping(code.as_ptr(), PARK_CODE as *mut u8, code.len());
    }
    parked().store(1, Ordering::Release);
}
pub fn parking_requested() -> bool {
    PARK_REQUESTED.load(Ordering::Acquire)
}
pub fn park_secondaries(rt: &crate::syscall::Rt) -> Result<(), &'static str> {
    for cpu in 1..rt.scheduler.cpu_count() {
        if rt.scheduler.current_on_cpu(cpu).is_some() {
            return Err("replacement cannot migrate active secondary CPU ownership");
        }
    }
    PARK_REQUESTED.store(true, Ordering::Release);
    super::interrupts::request_reschedule(cpu::configured_cpu_mask() & !1);
    let end = super::time::monotonic_ns() + 100_000_000;
    while parked().load(Ordering::Acquire) & cpu::configured_cpu_mask()
        != cpu::configured_cpu_mask()
    {
        if super::time::monotonic_ns() >= end {
            return Err("secondary parking timeout");
        }
        core::hint::spin_loop();
    }
    Ok(())
}
pub fn park_current_cpu() -> ! {
    super::context::install_parking_vectors();
    unsafe {
        asm!("cli", "jmp rax", in("rax") PARK_CODE, in("rdi") cpu::current_cpu_id(), in("rsi") PARK_ACK, in("rcx") super::mmu::kernel_root(), options(noreturn));
    }
}
pub fn capture_cpu_context() -> Aarch64CpuContextRecord {
    let (sp, root, flags): (u64, u64, u64);
    unsafe {
        asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack));
        asm!("mov {}, cr3", out(reg) root, options(nomem, nostack));
        asm!("pushfq", "pop {}", out(reg) flags);
    }
    #[repr(C, packed)]
    struct Idtr {
        limit: u16,
        base: u64,
    }
    let mut idtr = Idtr { limit: 0, base: 0 };
    unsafe {
        asm!("sidt [{}]", in(reg) &mut idtr, options(nostack));
    }
    Aarch64CpuContextRecord {
        cpu_id: cpu::current_cpu_id(),
        program_counter: crate::transplant::commit_pending as *const () as u64,
        stack_pointer: sp,
        pstate: flags,
        ttbr0_el1: root,
        ttbr1_el1: 0,
        vbar_el1: idtr.base,
    }
}
pub fn capture_system() -> Aarch64SystemRegisters {
    let (cr0, cr4): (u64, u64);
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
    }
    Aarch64SystemRegisters {
        mair: unsafe { io::rdmsr(0x277) },
        tcr: cr4,
        sctlr: cr0,
        cpacr: unsafe { io::rdmsr(0xc0000080) },
        cntkctl: 0,
    }
}
pub fn restore_system(saved: &Aarch64SystemRegisters) {
    unsafe {
        io::wrmsr(0x277, saved.mair);
        io::wrmsr(0xc0000080, saved.cpacr);
        asm!("mov cr4, {}", in(reg) saved.tcr, options(nostack));
        asm!("mov cr0, {}", in(reg) saved.sctlr, options(nostack));
    }
}

core::arch::global_asm!(
    r#"
.section .text,"ax"
.global kernel_prepare_call
kernel_prepare_call:
push rbp
push rbx
push r12
push r13
push r14
push r15
pushfq
mov [rip + PREPARE_RETURN_SP], rsp
mov rax, rdi
mov rsp, rsi
and rsp, -16
mov rdi, rdx
mov rsi, rcx
mov rdx, r8
call rax
jmp 2f
.global kernel_prepare_failure
kernel_prepare_failure:
mov rax, -1
2:
cli
mov rsp, [rip + PREPARE_RETURN_SP]
mov qword ptr [rip + PREPARE_RETURN_SP], 0
popfq
pop r15
pop r14
pop r13
pop r12
pop rbx
pop rbp
ret
"#
);

pub fn capture_architecture_state() -> [u64; 16] {
    let mut state = [0; 16];
    state[..3].copy_from_slice(&super::time::capture_state());
    state[3] = super::interrupts::capture_base();
    cpu::capture_state(&mut state[4..14]);
    state[14] = cpu::pci_ecam_base();
    state[15] = super::mmu::kernel_root();
    state
}
pub fn restore_architecture_state(state: &[u64; 16]) {
    cpu::restore_ecam(state[14]);
    super::mmu::restore_kernel_root(state[15]);
    cpu::restore_state(&state[4..14]);
    super::time::restore_state(&state[..3]);
    super::interrupts::restore_base(state[3]);
}
