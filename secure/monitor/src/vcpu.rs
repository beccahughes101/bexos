//! Software-owned vCPU registers, separate from the architectural VMCB.
//!
//! VMRUN does not switch general registers, extended state, or the extra state
//! handled by VMLOAD/VMSAVE (AMD APM volume 2, chapter 15). Keep that state per
//! vCPU, rather than letting a guest inherit monitor or another guest registers.
#[path = "vcpu_state.rs"]
mod state;
pub use state::{CpuIdentity, STATE_BYTES};

#[repr(C, align(16))]
pub struct LegacyExtendedState([u8; 512]);

impl Default for LegacyExtendedState {
    fn default() -> Self {
        let mut bytes = [0; 512];
        bytes[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
        bytes[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
        Self(bytes)
    }
}

impl LegacyExtendedState {
    /// Read the processor's mask, never a mask imported from a guest record.
    #[cfg(target_arch = "x86_64")]
    pub fn supported_mxcsr_mask() -> u32 {
        let mut state = Self::default();
        unsafe {
            core::arch::asm!("fxsave64 [{state}]", state = in(reg) &mut state, options(nostack));
        }
        let mask = u32::from_le_bytes(state.0[28..32].try_into().unwrap());
        if mask == 0 { 0xffbf } else { mask }
    }
    pub fn xmm(&self, index: usize) -> Option<u128> {
        if index >= 16 {
            return None;
        }
        let offset = 160 + index * 16;
        Some(u128::from_le_bytes(
            self.0[offset..offset + 16].try_into().unwrap(),
        ))
    }

    pub fn set_xmm(&mut self, index: usize, value: u128) -> bool {
        if index >= 16 {
            return false;
        }
        let offset = 160 + index * 16;
        self.0[offset..offset + 16].copy_from_slice(&value.to_le_bytes());
        true
    }
}

/// RAX and RSP live in the VMCB. These offsets are consumed by entry.S.
#[derive(Default)]
#[repr(C, align(16))]
pub struct Registers {
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub extended: LegacyExtendedState,
}

impl Registers {
    pub fn values(&self, vmcb: &crate::svm::Vmcb) -> [u64; 16] {
        [
            vmcb.rax(),
            self.rcx,
            self.rdx,
            self.rbx,
            vmcb.rsp(),
            self.rbp,
            self.rsi,
            self.rdi,
            self.r8,
            self.r9,
            self.r10,
            self.r11,
            self.r12,
            self.r13,
            self.r14,
            self.r15,
        ]
    }
    pub fn set_values(&mut self, vmcb: &mut crate::svm::Vmcb, values: &[u64; 16]) {
        vmcb.set_rax(values[0]);
        self.rcx = values[1];
        self.rdx = values[2];
        self.rbx = values[3];
        vmcb.set_rsp(values[4]);
        self.rbp = values[5];
        self.rsi = values[6];
        self.rdi = values[7];
        self.r8 = values[8];
        self.r9 = values[9];
        self.r10 = values[10];
        self.r11 = values[11];
        self.r12 = values[12];
        self.r13 = values[13];
        self.r14 = values[14];
        self.r15 = values[15];
    }
}

const _: () = {
    assert!(core::mem::offset_of!(Registers, r15) == 104);
    assert!(core::mem::offset_of!(Registers, extended) == 112);
    assert!(core::mem::size_of::<Registers>() == 624);
};

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
core::arch::global_asm!(include_str!("entry.S"));

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
unsafe extern "C" {
    /// Enter a legacy x87/SSE guest and save its state on VMEXIT.
    ///
    /// # Safety
    /// SVM and VM_HSAVE_PA must be initialized on this CPU. Both VMCB addresses
    /// must be aligned, private, resident physical addresses also identity
    /// mapped by the host. Registers must reside in private monitor memory.
    /// The caller must configure intercepts and CPUID policy to deny extended
    /// features beyond x87/SSE, guest DR0-3 access, and guest CR4 changes; the
    /// caller must not concurrently enter this vCPU on another host CPU.
    /// Maskable interrupts are disabled until host state is fully restored.
    /// NMI/MCE handling remains the caller's responsibility.
    pub fn bexos_svm_enter(guest_vmcb: u64, registers: &mut Registers, host_vmcb: u64);
}
