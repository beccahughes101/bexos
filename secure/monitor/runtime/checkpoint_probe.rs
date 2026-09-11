//! Diagnostic only: resume the real saved Trusty image after discarding and
//! reconstructing its CPU and peripheral objects from protected records.
use crate::{platform::Platform, transport::Boot};
use bexos_secure_monitor::{svm::Vmcb, vcpu::Registers};

pub unsafe fn verify(vmcb: &mut Vmcb, registers: &mut Registers, platform: &mut Platform<1>) {
    unsafe {
        let transport = Boot::new(|| crate::secure_step(vmcb, registers, platform));
        let mut avb = bexos_trusty_boot::avb::Avb::connect(transport).unwrap();
        let floor = avb.read_rollback(0).unwrap();
        avb.close().unwrap();
        drop(avb);

        reconstruct(vmcb, registers, platform);
        let mut restored_pending = false;
        let transport = Boot::new(|| {
            crate::secure_step(vmcb, registers, platform);
            if !restored_pending && crate::transport::has_running_request() {
                reconstruct(vmcb, registers, platform);
                restored_pending = true;
            }
        });
        let mut avb = bexos_trusty_boot::avb::Avb::connect(transport).unwrap();
        assert_eq!(avb.read_rollback(0).unwrap(), floor);
        avb.close().unwrap();
        drop(avb);
        assert!(restored_pending, "probe must interrupt a fetched request");
        crate::log("monitor-runtime: pending Trusty request retained across transport restore\n");
        crate::log("monitor-runtime: real Trusty IPC continued after CPU and platform restore\n");
    }
}

// The complete record lives outside either guest and contains no host pointers.
static mut OWNER_STATE: [u8; crate::secure_state::STATE_BYTES] =
    [0; crate::secure_state::STATE_BYTES];
unsafe fn reconstruct(vmcb: &mut Vmcb, registers: &mut Registers, platform: &mut Platform<1>) {
    unsafe {
        let state = &mut *core::ptr::addr_of_mut!(OWNER_STATE);
        crate::secure_state::snapshot(1, vmcb, registers, platform, state).unwrap();
        let rip = vmcb.rip();
        crate::secure_state::corrupt_transport_for_probe(state);
        assert!(
            crate::secure_state::restore_protected(1, state, vmcb, registers, platform).is_err()
        );
        assert_eq!(vmcb.rip(), rip);
        crate::secure_state::corrupt_transport_for_probe(state);
        crate::secure_state::assert_unchanged_for_probe(state, vmcb, registers, platform);
        crate::transport::discard_for_probe();
        *platform = Platform::new(
            crate::memory::DomainMemory {
                base: crate::BANK,
                length: crate::BANK_SIZE,
            },
            true,
        );
        *registers = Registers::default();
        *vmcb = Vmcb::new();
        crate::secure_state::restore_protected(1, state, vmcb, registers, platform).unwrap();
        assert_eq!(vmcb.rip(), rip);
        crate::log("monitor-runtime: secure owner restored after atomic transport rejection\n");
    }
}
