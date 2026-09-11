use super::*;

pub fn active_cpu_mask() -> u64 {
    CPU_BOOT_MASK.load(Ordering::Acquire)
}

pub fn current_cpu_id() -> u64 {
    let mpidr: u64;
    unsafe {
        asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
    }
    mpidr & 0xff
}

pub fn set_kernel_thread_pointer(cpu_id: u64) {
    unsafe {
        asm!("msr tpidr_el1, {cpu_id:x}", cpu_id = in(reg) cpu_id, options(nomem, nostack));
    }
}

pub fn wait_for_cpu_boot_mask(expected_mask: u64) -> u64 {
    let mut spins = 0u32;
    loop {
        let mask = CPU_BOOT_MASK.load(Ordering::Acquire);
        if mask & expected_mask == expected_mask || spins >= 1_000_000 {
            return mask;
        }
        spins = spins.saturating_add(1);
        core::hint::spin_loop();
    }
}

pub fn expected_cpu_mask(max_cpus: u32) -> u64 {
    if max_cpus >= 64 {
        u64::MAX
    } else {
        (1u64 << max_cpus) - 1
    }
}

pub fn set_configured_max_cpus(max_cpus: u32) {
    CONFIGURED_MAX_CPUS.store(u64::from(max_cpus), Ordering::Release);
}

pub fn configured_cpu_mask() -> u64 {
    expected_cpu_mask(CONFIGURED_MAX_CPUS.load(Ordering::Acquire) as u32)
}

pub fn release_secondary_schedulers() {
    SECONDARY_SCHED_READY.store(true, Ordering::Release);
    unsafe {
        asm!("sev", options(nomem, nostack));
    }
}

pub fn wait_for_scheduler_release() {
    while !SECONDARY_SCHED_READY.load(Ordering::Acquire) {
        unsafe {
            asm!("wfe", options(nomem, nostack));
        }
    }
}

pub fn detect_cpu_features() -> KernelFeatureState {
    let mmfr0_el1: u64;
    let pfr0_el1: u64;
    let pfr1_el1: u64;
    let isar0_el1: u64;
    let isar1_el1: u64;
    let isar2_el1: u64;
    unsafe {
        asm!("mrs {value:x}, id_aa64mmfr0_el1", value = out(reg) mmfr0_el1, options(nomem, nostack));
        asm!("mrs {value:x}, id_aa64pfr0_el1", value = out(reg) pfr0_el1, options(nomem, nostack));
        asm!("mrs {value:x}, id_aa64pfr1_el1", value = out(reg) pfr1_el1, options(nomem, nostack));
        asm!("mrs {value:x}, id_aa64isar0_el1", value = out(reg) isar0_el1, options(nomem, nostack));
        asm!("mrs {value:x}, id_aa64isar1_el1", value = out(reg) isar1_el1, options(nomem, nostack));
        asm!("mrs {value:x}, id_aa64isar2_el1", value = out(reg) isar2_el1, options(nomem, nostack));
    }
    KernelFeatureState::from_detected(Aarch64CpuFeatures::from_registers(
        mmfr0_el1,
        pfr0_el1,
        pfr1_el1,
        isar0_el1,
        isar1_el1,
        isar2_el1,
        timer_frequency(),
    ))
}

pub fn enable_user_access_protection(features: KernelFeatureState) {
    if !features.pan_enabled {
        return;
    }
    unsafe {
        asm!("msr S3_0_C4_C2_3, {enabled:x}", enabled = in(reg) 1_u64, options(nomem, nostack));
    }
}

pub fn invoke_smc(regs: [u64; 8]) -> [u64; 8] {
    let _interrupts = interrupts::secure::CallGuard::enter();
    let mut x0 = regs[0];
    let mut x1 = regs[1];
    let mut x2 = regs[2];
    let mut x3 = regs[3];
    let mut x4 = regs[4];
    let mut x5 = regs[5];
    let mut x6 = regs[6];
    let mut x7 = regs[7];
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") x0,
            inout("x1") x1,
            inout("x2") x2,
            inout("x3") x3,
            inout("x4") x4,
            inout("x5") x5,
            inout("x6") x6,
            inout("x7") x7,
            lateout("x8") _,
            lateout("x9") _,
            lateout("x10") _,
            lateout("x11") _,
            lateout("x12") _,
            lateout("x13") _,
            lateout("x14") _,
            lateout("x15") _,
            lateout("x16") _,
            lateout("x17") _,
            options(nostack),
        );
    }
    [x0, x1, x2, x3, x4, x5, x6, x7]
}

pub fn configure_mmu(ttbr0: u64, mair: u64, tcr: u64) {
    unsafe {
        asm!("msr mair_el1, {mair:x}", mair = in(reg) mair, options(nostack));
        asm!("msr tcr_el1, {tcr:x}", tcr = in(reg) tcr, options(nostack));
        asm!("msr ttbr0_el1, {ttbr0:x}", ttbr0 = in(reg) ttbr0, options(nostack));
        asm!("dsb ishst; tlbi vmalle1; dsb ish; isb", options(nostack));

        let mut sctlr: u64;
        asm!("mrs {sctlr:x}, sctlr_el1", sctlr = out(reg) sctlr, options(nostack));
        sctlr |= 1 << 19;
        sctlr |= 1 << 0;
        sctlr |= 1 << 2;
        sctlr |= 1 << 12;
        asm!("msr sctlr_el1, {sctlr:x}", sctlr = in(reg) sctlr, options(nostack));
        asm!("isb", options(nostack));
    }
}
