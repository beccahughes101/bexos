//! Interrupt ownership across the polling secure-monitor transport.
//!
//! Legacy GICv2 Trusty registers its timer and IPIs as Group 1 interrupts.
//! Each CPU must forward wakeups into its secure scheduler. The reference
//! Trusty IRQ protocol masks a delivered source, acknowledges it, reenables it
//! before entry, then issues a concurrent NOP on that same CPU.
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::{GICD_BASE, GICD_ICENABLER, GICD_ISENABLER, SCHEDULER_SGI, TIMER_IRQ};

static OWNED: AtomicU32 = AtomicU32::new(0);
static PENDING: AtomicU64 = AtomicU64::new(0);

fn cpu_bit() -> u64 {
    1 << super::aarch64::current_cpu_id()
}

pub fn enqueue() {
    if mask() != 0 {
        PENDING.fetch_or(cpu_bit(), Ordering::Release);
    }
}

pub fn pending() -> bool {
    PENDING.load(Ordering::Acquire) & cpu_bit() != 0
}

pub fn install(mask: u32) {
    assert_eq!(mask & ((1 << SCHEDULER_SGI) | (1 << TIMER_IRQ)), 0);
    super::write32(GICD_BASE + GICD_ICENABLER, mask);
    OWNED.store(mask, Ordering::Release);
}

pub fn mask() -> u32 {
    OWNED.load(Ordering::Acquire)
}

pub fn enable_local() {
    super::write32(GICD_BASE + GICD_ISENABLER, mask());
}

pub fn owns(irq: u32) -> bool {
    irq < 32 && mask() & (1 << irq) != 0
}

pub fn mask_delivered(irq: u32) {
    super::write32(GICD_BASE + GICD_ICENABLER, 1 << irq);
}

/// Run from the idle loop after EOI, outside interrupt/scheduler locks. NOP is the SMP progress operation;
/// never queue or restart another caller's ordinary secure standard call.
pub fn progress() {
    if mask() == 0 || !pending() {
        return;
    }
    let mut call = 0x3c00_0003;
    for _ in 0..16 {
        let result = super::aarch64::invoke_smc([call, 0, 0, 0, 0, 0, 0, 0])[0] as i32;
        match result {
            -12 => call = 0x3c00_0002,
            -14 => call = 0x3c00_0003,
            _ => return,
        }
    }
    enqueue();
}

pub struct CallGuard {
    daif: u64,
}

impl CallGuard {
    pub fn enter() -> Self {
        let daif: u64;
        unsafe {
            core::arch::asm!(
                "mrs {saved}, daif",
                "msr daifset, #3",
                saved = out(reg) daif,
                options(nostack),
            );
        }
        let interrupts = mask();
        PENDING.fetch_and(!cpu_bit(), Ordering::AcqRel);
        super::write32(GICD_BASE + GICD_ISENABLER, interrupts);
        unsafe { core::arch::asm!("dsb sy", "isb", options(nostack)) };
        Self { daif }
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        // Leave sources enabled so a secure timer/IPI can wake the owning CPU
        // even when normal-world provider polling runs on a different CPU.
        unsafe { core::arch::asm!("msr daifset, #3", options(nostack)) };
        unsafe {
            core::arch::asm!(
                "dsb sy",
                "isb",
                "msr daif, {saved}",
                saved = in(reg) self.daif,
                options(nostack),
            );
        }
    }
}
