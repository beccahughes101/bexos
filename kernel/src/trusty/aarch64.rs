use crate::arch::ArchAPI;
const API_VERSION: u64 = 0xbc00_000b;
const API_VERSION_SMP: u64 = 2;
const GET_SMP_MAX_CPUS: u64 = 0xbc00_000d;
const GET_NEXT_IRQ: u64 = 0xbc00_0003;
const SM_ERR_END_OF_INPUT: i32 = -10;
const RESTART_FIQ: u64 = 0x3c00_0002;
const NOP: u64 = 0x3c00_0003;
const SM_ERR_BUSY: i32 = -5;
const SM_ERR_FIQ_INTERRUPTED: i32 = -12;
const SM_ERR_CPU_IDLE: i32 = -13;
const SM_ERR_NOP_INTERRUPTED: i32 = -14;
const SM_ERR_NOP_DONE: i32 = -15;
const MAX_BOOTSTRAP_CALLS: usize = 16_384;

/// Negotiate the SMP protocol and check firmware capacity before PSCI enters
/// Trusty on any secondary CPU. Upstream requires this capacity check first.
pub fn prepare_bootstrap(required: bool, max_cpus: u32) -> bool {
    let version =
        crate::arch::CurrentArch::invoke_smc([API_VERSION, API_VERSION_SMP, 0, 0, 0, 0, 0, 0])[0];
    if version != API_VERSION_SMP {
        assert!(
            !required,
            "Trusty bootstrap API negotiation failed: {version:#x}"
        );
        return false;
    }
    let capacity = crate::arch::CurrentArch::invoke_smc([GET_SMP_MAX_CPUS, 0, 0, 0, 0, 0, 0, 0])[0];
    assert!(
        capacity >= u64::from(max_cpus) && capacity <= 64,
        "Trusty firmware CPU capacity {capacity} cannot boot {max_cpus} CPUs; refresh firmware with bazel run //third_party/trusty:refresh_image"
    );
    true
}

/// Progress asynchronous built-in-TA startup on the primary CPU. Drain once
/// before secondary entry, then again after PSCI has entered each secure CPU.
pub fn finish_bootstrap(available: bool) {
    if !available {
        return;
    }
    let mut call = NOP;
    for _ in 0..MAX_BOOTSTRAP_CALLS {
        let result = crate::arch::CurrentArch::invoke_smc([call, 0, 0, 0, 0, 0, 0, 0]);
        match result[0] as i32 {
            SM_ERR_NOP_DONE | 0 => {
                crate::log_line("kernel: Trusty bootstrap completed");
                return;
            }
            SM_ERR_NOP_INTERRUPTED => call = NOP,
            SM_ERR_FIQ_INTERRUPTED => call = RESTART_FIQ,
            SM_ERR_BUSY | SM_ERR_CPU_IDLE => {
                core::hint::spin_loop();
                call = NOP;
            }
            status => panic!("Trusty bootstrap failed status={status}"),
        }
    }
    panic!("Trusty bootstrap timed out");
}

/// Enumeration freezes Trusty's handler table, so it must follow secure
/// secondary initialization rather than the first primary bootstrap drain.
pub fn register_interrupts(available: bool) {
    if !available {
        return;
    }
    let mut mask = 0u32;
    let mut first = 0u64;
    loop {
        let irq = crate::arch::CurrentArch::invoke_smc([GET_NEXT_IRQ, first, 1, 0, 0, 0, 0, 0])[0];
        if irq as i32 == SM_ERR_END_OF_INPUT {
            break;
        }
        assert!(
            irq >= first && irq < 32,
            "invalid secure per-CPU interrupt {irq}"
        );
        mask |= 1u32 << irq;
        first = irq + 1;
    }
    let spi = crate::arch::CurrentArch::invoke_smc([GET_NEXT_IRQ, 32, 0, 0, 0, 0, 0, 0])[0];
    assert_eq!(
        spi as i32, SM_ERR_END_OF_INPUT,
        "secure SPI routing unavailable"
    );
    crate::arch::aarch64::interrupts::secure::install(mask);
    // The boot allocator has not been initialized at this point.
    crate::log_line("kernel: secure interrupt ownership registered");
}
