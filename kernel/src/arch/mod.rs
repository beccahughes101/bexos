mod api;
pub use api::ArchAPI;
#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::{
    Aarch64 as CurrentArch, early_uart, interrupts, mmu, power, transplant as cpu_transplant,
};

#[cfg(target_arch = "x86_64")]
pub mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::{
    X86_64 as CurrentArch, early_uart, interrupts, mmu, power, transplant as cpu_transplant,
};
