//! Architecture-tagged saved state; shared policy uses normalized operations.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub(crate) regs: [u64; 31],
    pub stack_pointer: u64,
    pub instruction_pointer: u64,
    pub(crate) processor_state: u64,
    pub(crate) simd: [u64; 64],
    pub(crate) fp_control: u64,
    pub(crate) fp_status: u64,
    pub thread_pointer: u64,
    pub(crate) architecture: u64,
}
impl Context {
    pub fn user(entry: u64, stack_top: u64, argument: u64, thread_pointer: u64) -> Option<Self> {
        // Match the TLS setter's userspace limit. In particular, a
        // noncanonical x86 FS base would fault while returning from the kernel.
        if thread_pointer >= bexos_boot::USER_END {
            return None;
        }
        let mut context = Self::zero();
        context.instruction_pointer = entry;
        context.stack_pointer = if cfg!(bexos_arch_x86_64) {
            stack_top.checked_sub(8)?
        } else {
            stack_top
        };
        context.set_initial_argument(argument);
        context.set_thread_pointer(thread_pointer);
        Some(context)
    }
    pub fn valid_user_stack(&self) -> bool {
        self.stack_pointer % if cfg!(bexos_arch_x86_64) { 8 } else { 16 } == 0
    }
    /// Logical syscall words, normalized by the architecture entry assembly.
    pub fn syscall_words(&self) -> &[u64; 10] {
        self.regs[..10].try_into().unwrap()
    }
    pub fn syscall_words_mut(&mut self) -> &mut [u64; 10] {
        (&mut self.regs[..10]).try_into().unwrap()
    }
    pub fn thread_pointer(&self) -> u64 {
        self.thread_pointer
    }
    pub fn set_thread_pointer(&mut self, pointer: u64) {
        self.thread_pointer = pointer;
    }
    pub fn set_initial_argument(&mut self, argument: u64) {
        self.regs[0] = argument;
    }

    pub const ARCHITECTURE: u64 = if cfg!(bexos_arch_x86_64) { 2 } else { 1 };
    pub const fn matches_current_architecture(&self) -> bool {
        self.architecture == Self::ARCHITECTURE
    }

    pub const fn zero() -> Self {
        Self {
            regs: {
                let mut regs = [0; 31];
                if cfg!(bexos_arch_x86_64) {
                    regs[30] = 0x23;
                }
                regs
            },
            stack_pointer: 0,
            instruction_pointer: 0,
            processor_state: if cfg!(bexos_arch_x86_64) { 0x202 } else { 0 },
            simd: {
                let mut state = [0; 64];
                if cfg!(bexos_arch_x86_64) {
                    state[0] = 0x37f;
                    state[3] = 0x1f80;
                }
                state
            },
            fp_control: 0,
            fp_status: 0,
            thread_pointer: 0,
            architecture: Self::ARCHITECTURE,
        }
    }
}
