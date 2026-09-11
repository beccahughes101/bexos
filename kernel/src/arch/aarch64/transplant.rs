use crate::arch::ArchAPI;
use bexos_kernel_core::transplant::{Aarch64CpuContextRecord, Aarch64SystemRegisters};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static PARK_REQUESTED: AtomicBool = AtomicBool::new(false);
static PARKED: AtomicU64 = AtomicU64::new(1);

pub fn parking_requested() -> bool {
    PARK_REQUESTED.load(Ordering::Acquire)
}

/// The current hardware runtime pins EL0 threads to CPU0. Its idle secondary
/// stacks and vectors must not remain live inside the image being reclaimed.
/// Migrating running secondary EL0 ownership is a separate, unsupported path.
pub fn park_secondaries(rt: &crate::syscall::Rt) -> Result<(), &'static str> {
    let expected = crate::arch::CurrentArch::configured_cpu_mask();
    if expected & !0xff != 0 {
        return Err("replacement secondary parking exceeds GIC target mask");
    }
    for cpu in 1..rt.scheduler.cpu_count() {
        if rt.scheduler.current_on_cpu(cpu).is_some() {
            return Err("replacement cannot migrate active secondary CPU ownership");
        }
    }
    PARK_REQUESTED.store(true, Ordering::Release);
    crate::interrupts::request_reschedule(expected & !1);
    let deadline = crate::migration::now_ms().saturating_add(100);
    while PARKED.load(Ordering::Acquire) & expected != expected {
        if crate::migration::now_ms() >= deadline {
            return Err("secondary CPUs did not acknowledge replacement parking");
        }
        core::hint::spin_loop();
    }
    crate::log_line("heart-transplant: idle secondary CPUs parked outside kernel image");
    Ok(())
}

pub fn park_current_cpu() -> ! {
    let cpu = crate::arch::CurrentArch::current_cpu_id();
    let acknowledgement = core::ptr::addr_of!(PARKED) as u64;
    // Cold boot installs this stackless trampoline in reserved handoff RAM.
    // It records the acknowledgement once, then never touches the old image.
    unsafe {
        core::arch::asm!(
            "msr daifset, #0xf",
            "dsb sy",
            "mrs x3, sctlr_el1",
            "bic x3, x3, #1",
            "msr sctlr_el1, x3",
            "isb",
            "br x2",
            in("x0") cpu,
            in("x1") acknowledgement,
            in("x2") 0x4010_2000u64,
            options(noreturn),
        );
    }
}

pub fn capture_system() -> Aarch64SystemRegisters {
    let mut r = Aarch64SystemRegisters::default();
    unsafe {
        core::arch::asm!("mrs {}, mair_el1", out(reg) r.mair, options(nostack));
        core::arch::asm!("mrs {}, tcr_el1", out(reg) r.tcr, options(nostack));
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) r.sctlr, options(nostack));
        core::arch::asm!("mrs {}, cpacr_el1", out(reg) r.cpacr, options(nostack));
        core::arch::asm!("mrs {}, cntkctl_el1", out(reg) r.cntkctl, options(nostack));
    }
    r
}
pub fn restore_system(r: &Aarch64SystemRegisters) {
    unsafe {
        core::arch::asm!("msr mair_el1, {}", in(reg) r.mair, options(nostack));
        core::arch::asm!("msr tcr_el1, {}", in(reg) r.tcr, options(nostack));
        core::arch::asm!("msr cpacr_el1, {}", in(reg) r.cpacr, options(nostack));
        core::arch::asm!("msr cntkctl_el1, {}", in(reg) r.cntkctl, options(nostack));
        core::arch::asm!("msr sctlr_el1, {}; isb", in(reg) r.sctlr, options(nostack));
    }
}

pub fn capture_cpu_context() -> Aarch64CpuContextRecord {
    let (sp, ttbr0, ttbr1, vbar, pstate, cpu): (u64, u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack));
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
        core::arch::asm!("mrs {}, ttbr1_el1", out(reg) ttbr1, options(nomem, nostack));
        core::arch::asm!("mrs {}, vbar_el1", out(reg) vbar, options(nomem, nostack));
        core::arch::asm!("mrs {}, daif", out(reg) pstate, options(nomem, nostack));
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) cpu, options(nomem, nostack));
    }
    Aarch64CpuContextRecord {
        cpu_id: cpu & 255,
        program_counter: crate::transplant::commit_pending as *const () as u64,
        stack_pointer: sp,
        pstate,
        ttbr0_el1: ttbr0,
        ttbr1_el1: ttbr1,
        vbar_el1: vbar,
    }
}

pub fn capture_architecture_state() -> [u64; 16] {
    let mut state = [0; 16];
    state[0] = 1;
    state[1] = u64::from(super::interrupts::secure::mask());
    state
}
pub fn restore_architecture_state(state: &[u64; 16]) {
    assert!(state[0] <= 1 && state[2..].iter().all(|value| *value == 0));
    assert!(state[1] <= u64::from(u32::MAX));
    assert!(state[0] != 0 || state[1] == 0);
    super::interrupts::secure::install(state[1] as u32);
}
