#[cfg(all(bexos_guest, target_arch = "aarch64"))]
mod aarch64;
#[cfg(all(bexos_guest, target_arch = "aarch64"))]
pub use aarch64::syscall;
#[cfg(all(bexos_guest, target_arch = "x86_64"))]
mod x86_64;
#[cfg(all(bexos_guest, target_arch = "x86_64"))]
pub use x86_64::syscall;
#[cfg(not(bexos_guest))]
pub mod host;
#[cfg(not(bexos_guest))]
pub use host as syscall;
