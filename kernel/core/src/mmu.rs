//! Architecture-selected page-table encoding; runtime policy uses access rights.
#[cfg(not(bexos_arch_x86_64))]
mod aarch64;
#[cfg(bexos_arch_x86_64)]
mod x86_64;
#[cfg(not(bexos_arch_x86_64))]
pub use aarch64::*;
#[cfg(bexos_arch_x86_64)]
pub use x86_64::*;
