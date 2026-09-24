//! Stable shared-page ABI for restricted userspace execution.
#![no_std]

pub const STATE_VMO_SIZE: u64 = 4096;
pub const MAGIC: u64 = u64::from_le_bytes(*b"BEXRST01");
pub const VERSION: u32 = 1;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Architecture {
    Aarch64 = 1,
    X86_64 = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    Syscall = 1,
    Exception = 2,
    Kick = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub magic: u64,
    pub version: u32,
    pub size: u32,
    pub architecture: u32,
    pub reason: u32,
    pub exception_code: u64,
    pub fault_address: u64,
    pub reserved: [u64; 3],
}

impl Header {
    pub const fn new(architecture: Architecture, size: usize) -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            size: size as u32,
            architecture: architecture as u32,
            reason: 0,
            exception_code: 0,
            fault_address: 0,
            reserved: [0; 3],
        }
    }

    pub fn valid_for(&self, architecture: Architecture, size: usize) -> bool {
        self.magic == MAGIC
            && self.version == VERSION
            && self.size as usize == size
            && self.architecture == architecture as u32
            && self.reserved == [0; 3]
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct X86_64StateV1 {
    pub header: Header,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub r8: u64,
    pub r9: u64,
    pub rax: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub fs_base: u64,
    pub gs_base: u64,
}

impl X86_64StateV1 {
    pub const fn zeroed() -> Self {
        Self {
            header: Header::new(Architecture::X86_64, core::mem::size_of::<Self>()),
            rdi: 0,
            rsi: 0,
            rdx: 0,
            rcx: 0,
            r8: 0,
            r9: 0,
            rax: 0,
            rbx: 0,
            rbp: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rsp: 0,
            rflags: 0x202,
            fs_base: 0,
            gs_base: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Aarch64StateV1 {
    pub header: Header,
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
    pub tpidr_el0: u64,
    pub tpidrro_el0: u64,
}

impl Aarch64StateV1 {
    pub const fn zeroed() -> Self {
        Self {
            header: Header::new(Architecture::Aarch64, core::mem::size_of::<Self>()),
            x: [0; 31],
            sp: 0,
            pc: 0,
            pstate: 0,
            tpidr_el0: 0,
            tpidrro_el0: 0,
        }
    }
}

pub type VectorEntry = unsafe extern "C" fn(context: u64, reason: Reason) -> !;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_bounded_and_self_describing() {
        let x86 = X86_64StateV1::zeroed();
        assert!(
            x86.header
                .valid_for(Architecture::X86_64, core::mem::size_of::<X86_64StateV1>())
        );
        let arm = Aarch64StateV1::zeroed();
        assert!(arm.header.valid_for(
            Architecture::Aarch64,
            core::mem::size_of::<Aarch64StateV1>()
        ));
        assert!(core::mem::size_of::<X86_64StateV1>() <= STATE_VMO_SIZE as usize);
        assert!(core::mem::size_of::<Aarch64StateV1>() <= STATE_VMO_SIZE as usize);
        assert_eq!(core::mem::offset_of!(X86_64StateV1, rdi), 64);
        assert_eq!(core::mem::size_of::<Header>(), 64);
        assert_eq!(core::mem::size_of::<X86_64StateV1>(), 224);
        assert_eq!(core::mem::offset_of!(X86_64StateV1, rip), 184);
        assert_eq!(core::mem::offset_of!(X86_64StateV1, gs_base), 216);
        assert_eq!(core::mem::offset_of!(Aarch64StateV1, x), 64);
        assert_eq!(core::mem::size_of::<Aarch64StateV1>(), 352);
        assert_eq!(core::mem::offset_of!(Aarch64StateV1, sp), 312);
        assert_eq!(core::mem::offset_of!(Aarch64StateV1, tpidrro_el0), 344);
    }

    #[test]
    fn corrupted_headers_are_rejected() {
        let mut state = X86_64StateV1::zeroed();
        state.header.reserved[1] = 1;
        assert!(
            !state
                .header
                .valid_for(Architecture::X86_64, core::mem::size_of::<X86_64StateV1>())
        );
    }
}
