pub mod aarch64;
pub mod x86_64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Machine {
    Aarch64,
    X86_64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Relocation {
    Relative,
    Absolute,
    Symbol,
    TlsModule,
    TlsOffset,
    ThreadOffset,
}
impl Machine {
    pub const fn current_guest() -> Self {
        if cfg!(bexos_arch_x86_64) {
            Self::X86_64
        } else {
            Self::Aarch64
        }
    }
    pub const fn elf_machine(self) -> u16 {
        match self {
            Self::Aarch64 => 183,
            Self::X86_64 => 62,
        }
    }
    pub const fn from_elf(value: u16) -> Option<Self> {
        match value {
            183 => Some(Self::Aarch64),
            62 => Some(Self::X86_64),
            _ => None,
        }
    }
    pub fn relocation(self, kind: u32) -> Option<Relocation> {
        match self {
            Self::Aarch64 => aarch64::relocation(kind),
            Self::X86_64 => x86_64::relocation(kind),
        }
    }
}
