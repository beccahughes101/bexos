//! Architecture-specific bootstrap; Trusty protocol traffic stays in its provider.
#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::{finish_bootstrap, prepare_bootstrap, register_interrupts};

#[cfg(target_arch = "x86_64")]
pub fn prepare_bootstrap(required: bool, _max_cpus: u32) -> bool {
    // The resident monitor completed Trusty boot and AVB approval before the
    // first normal-world instruction. BexOS SMP does not start secure vCPUs.
    assert!(
        !required || crate::arch::x86_64::secure_monitor_ready(),
        "resident Trusty bootstrap unavailable"
    );
    if required {
        crate::log_line("kernel: Trusty initialized by resident monitor");
    }
    false
}
#[cfg(target_arch = "x86_64")]
pub fn finish_bootstrap(_available: bool) {}
