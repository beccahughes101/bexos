//! Versioned x86 legacy-state record. Capture only after the VMEXIT handler
//! completes and before re-entry; no live CPU may mutate these registers.
use super::*;
use crate::svm::{InvalidVmcb, RestorePolicy, Vmcb};
use sha2::{Digest, Sha256};
const GUEST: usize = 32 + 3072;
const GENERAL: usize = 32 + GUEST;
const EXTENDED: usize = GENERAL + 128;
const END: usize = EXTENDED + 512;
pub const STATE_BYTES: usize = END + 32;

#[derive(Clone, Copy)]
pub struct CpuIdentity {
    pub domain: u32,
    pub cpu: u32,
}
impl Registers {
    pub fn snapshot(
        &self,
        vmcb: &Vmcb,
        identity: CpuIdentity,
        output: &mut [u8],
    ) -> Result<(), InvalidVmcb> {
        if output.len() != STATE_BYTES || identity.domain == 0 || identity.cpu >= 32 {
            return Err(InvalidVmcb);
        }
        output.fill(0);
        output[..8].copy_from_slice(b"BEXCPU01");
        output[8..12].copy_from_slice(&identity.domain.to_le_bytes());
        output[12..16].copy_from_slice(&identity.cpu.to_le_bytes());
        output[16..24].copy_from_slice(&1u64.to_le_bytes()); // Legacy x87/SSE ABI.
        output[24..28]
            .copy_from_slice(&u32::from(bexos_secure_monitor_abi::ARCH_X86_64).to_le_bytes());
        vmcb.save_guest(&mut output[32..GENERAL])?;
        for (slot, value) in output[GENERAL..EXTENDED]
            .chunks_exact_mut(8)
            .zip(self.values(vmcb))
        {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        output[EXTENDED..END].copy_from_slice(&self.extended.0);
        let digest = Sha256::digest(&output[..END]);
        output[END..].copy_from_slice(&digest);
        Ok(())
    }

    /// Requires protected resident provenance and matching memory/device
    /// records. Restored control pointers come exclusively from `policy`.
    /// Neither live destination is changed if validation fails.
    pub fn restore_protected(
        &mut self,
        vmcb: &mut Vmcb,
        identity: CpuIdentity,
        policy: RestorePolicy,
        input: &[u8],
    ) -> Result<(), InvalidVmcb> {
        if input.len() != STATE_BYTES
            || &input[..8] != b"BEXCPU01"
            || identity.domain == 0
            || identity.cpu >= 32
            || input[8..12] != identity.domain.to_le_bytes()
            || input[12..16] != identity.cpu.to_le_bytes()
            || input[16..24] != 1u64.to_le_bytes()
            || input[24..28] != u32::from(bexos_secure_monitor_abi::ARCH_X86_64).to_le_bytes()
            || input[28..32] != [0; 4]
            || input[END..] != Sha256::digest(&input[..END])[..]
        {
            return Err(InvalidVmcb);
        }
        let mxcsr = u32::from_le_bytes(input[EXTENDED + 24..EXTENDED + 28].try_into().unwrap());
        // Baseline SSE state; reject reserved bits before host FXRSTOR.
        if mxcsr & !policy.mxcsr_mask != 0 {
            return Err(InvalidVmcb);
        }
        let mut restored = Vmcb::load_guest(&input[32..GENERAL], policy)?;
        let values = core::array::from_fn(|index| {
            u64::from_le_bytes(
                input[GENERAL + index * 8..GENERAL + index * 8 + 8]
                    .try_into()
                    .unwrap(),
            )
        });
        if values[0] != restored.rax() || values[4] != restored.rsp() {
            return Err(InvalidVmcb);
        }
        let mut registers = Self::default();
        registers.set_values(&mut restored, &values);
        registers.extended.0.copy_from_slice(&input[EXTENDED..END]);
        *vmcb = restored;
        *self = registers;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_cpu_preserves_general_sse_msr_and_pending_interrupt_state() {
        let identity = CpuIdentity { domain: 1, cpu: 3 };
        let policy = RestorePolicy {
            asid: 5,
            npt: 0x3000,
            iopm: 0x10000,
            msrpm: 0x20000,
            mxcsr_mask: 0xffff,
        };
        let mut vmcb = Vmcb::new();
        vmcb.initialize(1, 0x1000, 0x2000, 0x4000, 0x8000).unwrap();
        vmcb.virtual_interrupt(Some(33));
        assert!(vmcb.set_saved_msr(0xc0000102, 0xffff800000001000));
        let mut old = Registers::default();
        let values = core::array::from_fn(|index| index as u64 * 17);
        old.set_values(&mut vmcb, &values);
        for index in 0..16 {
            assert!(old.extended.set_xmm(index, (index as u128) << 100 | 99));
        }
        let mut record = [0; STATE_BYTES];
        old.snapshot(&vmcb, identity, &mut record).unwrap();
        let mut new = Registers::default();
        let mut restored = Vmcb::new();
        new.restore_protected(&mut restored, identity, policy, &record)
            .unwrap();
        assert_eq!(new.values(&restored), values);
        assert_eq!(restored.saved_msr(0xc0000102), Some(0xffff800000001000));
        assert!(restored.virtual_interrupt_pending());
        for index in 0..16 {
            assert_eq!(new.extended.xmm(index), old.extended.xmm(index));
        }
        for offset in [8, 12, 16, 24, GENERAL, EXTENDED + 27] {
            let mut bad = record;
            bad[offset] ^= 0x80;
            let digest = Sha256::digest(&bad[..END]);
            bad[END..].copy_from_slice(&digest);
            assert!(
                new.restore_protected(&mut restored, identity, policy, &bad)
                    .is_err()
            );
            assert_eq!(new.values(&restored), values);
        }
    }
}
