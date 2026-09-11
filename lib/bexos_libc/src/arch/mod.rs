#[cfg(all(bexos_guest, target_arch = "aarch64"))]
mod aarch64;
#[cfg(all(bexos_guest, target_arch = "aarch64"))]
pub(crate) use aarch64::*;
#[cfg(all(bexos_guest, target_arch = "x86_64"))]
mod x86_64;
#[cfg(all(bexos_guest, target_arch = "x86_64"))]
pub(crate) use x86_64::*;

#[cfg(all(bexos_guest, target_arch = "aarch64"))]
#[path = "aarch64/memory.rs"]
mod memory;
#[cfg(all(bexos_guest, target_arch = "x86_64"))]
#[path = "x86_64/memory.rs"]
mod memory;
#[cfg(not(bexos_guest))]
#[path = "host_memory.rs"]
mod memory;
pub(crate) use memory::*;

// Exercise the actual ARM routines on an ARM host, without guest TLS/syscalls.
#[cfg(all(test, target_arch = "aarch64", not(bexos_guest)))]
#[path = "aarch64/memory.rs"]
mod native_memory_tests;
