pub const ELF_MACHINE: bexos_elf::arch::Machine = bexos_elf::arch::Machine::X86_64;
pub const TLS_LOAD_BASE: u64 = 0xbe00_0000;
pub const LIBRARY_LOAD_BASE: u64 = 0xc000_0000;
pub const LIBRARY_LOAD_LIMIT: u64 = 0xf000_0000;
