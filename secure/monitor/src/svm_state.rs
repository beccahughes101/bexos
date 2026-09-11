//! Guest state only. Replacement installs fresh monitor-owned control maps;
//! saved NPT, IOPM, MSRPM, ASID and host addresses are never imported.
use super::*;

pub(crate) const GUEST_BYTES: usize = 32 + 3072;
#[derive(Clone, Copy)]
pub struct RestorePolicy {
    pub asid: u32,
    pub npt: u64,
    pub iopm: u64,
    pub msrpm: u64,
    /// Supported MXCSR bits from a resident host FXSAVE (use architectural
    /// 0xffbf when hardware reports a zero mask), never from the record.
    pub mxcsr_mask: u32,
}
impl Vmcb {
    pub(crate) fn save_guest(&self, out: &mut [u8]) -> Result<(), InvalidVmcb> {
        if out.len() != GUEST_BYTES {
            return Err(InvalidVmcb);
        }
        out[..8].copy_from_slice(&self.0[0x50..0x58]); // TSC offset.
        out[8..24].copy_from_slice(&self.0[0x60..0x70]); // Virtual IRQ/shadow.
        out[24..32].copy_from_slice(&self.0[0xa8..0xb0]); // Event injection.
        out[32..].copy_from_slice(&self.0[0x400..]);
        Ok(())
    }

    pub(crate) fn load_guest(input: &[u8], policy: RestorePolicy) -> Result<Self, InvalidVmcb> {
        if input.len() != GUEST_BYTES || policy.mxcsr_mask == 0 || policy.mxcsr_mask & !0xffff != 0
        {
            return Err(InvalidVmcb);
        }
        let word = |offset| u64::from_le_bytes(input[offset..offset + 8].try_into().unwrap());
        // This runtime supports the legacy virtual IRQ fields and interrupt
        // shadow. AVIC, virtual GIF and exception injection are not enabled.
        if word(8) & !0x000000ff_010f01ff != 0 || word(16) & !1 != 0 || word(24) != 0 {
            return Err(InvalidVmcb);
        }
        let mut restored = Self::new();
        restored.initialize(policy.asid, policy.npt, 0, 0, 0)?;
        restored.set_permission_maps(policy.iopm, policy.msrpm)?;
        restored.intercept_cpuid();
        restored.intercept_nmi();
        restored.0[0x50..0x58].copy_from_slice(&input[..8]);
        restored.0[0x60..0x70].copy_from_slice(&input[8..24]);
        // Keep physical interrupt masking under monitor control, including a
        // vCPU that had not yet entered when the snapshot was taken.
        restored.write32(0x60, word(8) as u32 | (1 << 24));
        restored.0[0x400..].copy_from_slice(&input[32..]);
        if restored.cr4() & !0x6f0 != 0
            || restored.efer() & !(SVME | 0xd01) != 0
            || restored.efer() & SVME == 0
            || restored.cpl() > 3
            || !canonical(restored.rip())
            || !canonical(restored.rsp())
        {
            return Err(InvalidVmcb);
        }
        // VMLOAD occurs before VMRUN's consistency checks. Validate every
        // imported base/MSR that could otherwise fault in the monitor.
        for offset in [0x448, 0x458, 0x478, 0x498] {
            if !canonical(restored.read64(offset)) {
                return Err(InvalidVmcb);
            }
        }
        for index in [
            0xc0000081, 0xc0000082, 0xc0000083, 0xc0000084, 0xc0000100, 0xc0000101, 0xc0000102,
            0x174, 0x175, 0x176, PAT,
        ] {
            let value = restored.saved_msr(index).ok_or(InvalidVmcb)?;
            if !restored.set_saved_msr(index, value) {
                return Err(InvalidVmcb);
            }
        }
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_resume_installs_fresh_isolation_roots_and_flushes_tlbs() {
        let mut old = Vmcb::new();
        old.initialize(1, 0x1000, 0x2000, 0x4000, 0x8000).unwrap();
        old.set_permission_maps(0x10000, 0x20000).unwrap();
        old.virtual_interrupt(Some(45));
        old.write64(0x68, 1);
        assert!(old.set_saved_msr(PAT, 0x0601_0405_0607_0400));
        let mut bytes = [0; GUEST_BYTES];
        old.save_guest(&mut bytes).unwrap();
        let new = Vmcb::load_guest(
            &bytes,
            RestorePolicy {
                asid: 3,
                npt: 0x3000,
                iopm: 0x30000,
                msrpm: 0x40000,
                mxcsr_mask: 0xffff,
            },
        )
        .unwrap();
        assert_eq!(new.read64(0xb0), 0x3000);
        assert_eq!(new.read64(0x40), 0x30000);
        assert_eq!(new.read64(0x48), 0x40000);
        assert_eq!(new.0[0x5c], 1);
        assert_eq!(new.cr3(), old.cr3());
        assert_eq!(new.rip(), old.rip());
        assert_eq!(new.read64(0x60), old.read64(0x60));
        assert_eq!(new.read64(0x68), 1);
        assert_eq!(new.saved_msr(PAT), old.saved_msr(PAT));
        assert!(new.virtual_interrupt_pending());
        for (offset, value) in [
            (8, u64::MAX),
            (16, 2),
            (24, 1),
            (32 + 0x208, 0x800000000000),
            (32 + 0x268, 0x0606_0606_0606_0602),
        ] {
            let mut invalid = bytes;
            invalid[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(
                Vmcb::load_guest(
                    &invalid,
                    RestorePolicy {
                        asid: 3,
                        npt: 0x3000,
                        iopm: 0x30000,
                        msrpm: 0x40000,
                        mxcsr_mask: 0xffff
                    }
                )
                .is_err()
            );
        }
    }
}
