//! Actual disk-selected Trusty reboot diagnostic, with root-enforced trial I/O
//! fencing and no admitted normal-world clients. Live Trusty is not supported.
use bexos_secure_firmware::{Component, selection::Identity, store};
use bexos_secure_monitor::{
    firmware_disk::{Disk, Native},
    svm::Vmcb,
    vcpu::Registers,
};
use bexos_trusty_boot::{avb::Avb, ql::Transport, recovery, selection};
const COMPONENT: Component = Component::Trusty;
const ROOT: &[u8] = include_bytes!(env!("RECOVERY_ROOT"));
static mut SCRATCH: [u8; bexos_secure_firmware::MAX_BUNDLE_BYTES] =
    [0; bexos_secure_firmware::MAX_BUNDLE_BYTES];

fn generation(transport: &mut impl Transport) -> u64 {
    let mut value = [0; 8];
    assert_eq!(transport.exchange(0x8000_0003, 8, &mut value), Ok(8));
    u64::from_le_bytes(value)
}
fn release(transport: &mut impl Transport) {
    assert_eq!(transport.exchange(0x8000_0000, 1, &mut [1]), Ok(0));
}
pub unsafe fn boot(
    vmcb: &mut Vmcb,
    regs: &mut Registers,
    platform: &mut crate::platform::Platform<1>,
) -> ! {
    let mut disk = Disk::identify(unsafe { Native::acquire() }).unwrap();
    let scratch = unsafe { &mut *core::ptr::addr_of_mut!(SCRATCH) };
    let mut owner =
        unsafe { crate::transport::Boot::new(|| crate::secure_step(vmcb, regs, platform)) };
    let state = selection::query(&mut owner).unwrap();
    if state.revision == 1 {
        drop(owner);
        let mut avb = Avb::connect(unsafe {
            crate::transport::Boot::new(|| crate::secure_step(vmcb, regs, platform))
        })
        .unwrap();
        // Independent persistent service data, outside component floors.
        avb.write_rollback(27, 7).unwrap();
        avb.close().unwrap();
        drop(avb);
        let mut owner =
            unsafe { crate::transport::Boot::new(|| crate::secure_step(vmcb, regs, platform)) };
        recovery::stage(
            &mut crate::recovery_faults::staging::Pending(&mut owner),
            &mut crate::recovery_faults::staging::Disk::new(&mut disk),
            COMPONENT,
            include_bytes!(env!("TRUSTY_CANDIDATE")),
            ROOT,
            1,
            scratch,
        )
        .unwrap();
        crate::log(
            "monitor-runtime: signed Trusty inactive slot flushed authenticated and pending\n",
        );
        crate::halt();
    }
    let decision = recovery::prepare_boot(&mut owner, &mut disk, ROOT, [1, 1], scratch)
        .unwrap_or_else(|_| {
            crate::log("monitor-runtime: firmware recovery required; no older image admitted\n");
            crate::halt();
        });
    let identity = decision.selected(COMPONENT);
    if identity == Identity::INITIAL {
        assert!(decision.rolled_back());
        let mut avb = Avb::connect(owner).unwrap();
        assert_eq!(avb.read_rollback(27).unwrap(), 7);
        avb.close().unwrap();
        crate::log(
            "monitor-runtime: exhausted Trusty trial rolled back with persistent service data intact\n",
        );
        crate::halt();
    }
    let trial = decision.trial().is_some();
    release(&mut owner);
    drop(owner);
    let loaded = store::load(&mut disk, COMPONENT, identity, ROOT, 1, scratch).unwrap();
    unsafe { crate::secure_boot::restart(loaded.image, vmcb, regs, platform, trial) };
    let mut owner =
        unsafe { crate::transport::Boot::new(|| crate::secure_step(vmcb, regs, platform)) };
    assert_eq!(generation(&mut owner), identity.generation);
    drop(owner);
    if trial {
        let mut avb = Avb::connect(unsafe {
            crate::transport::Boot::cold(|| crate::secure_step(vmcb, regs, platform))
        })
        .unwrap();
        assert_eq!(avb.read_rollback(27).unwrap(), 7);
        assert!(crate::secure_boot::completed_reads() > 0);
        assert!(avb.write_rollback(27, 8).is_err());
        avb.close().unwrap();
        drop(avb);
        assert!(crate::secure_boot::blocked_writes() > 0);
        crate::log(
            "monitor-runtime: distinct Trusty trial code executed with persistent transport fenced\n",
        );
        crate::recovery_faults::interrupt(1);
        // Trial forwarded authenticated reads but no persistent mutation.
        // Recovery resolves commitment; the candidate cannot publish it.
        unsafe { crate::secure_boot::restart(crate::TRUSTY, vmcb, regs, platform, false) };
        let mut owner =
            unsafe { crate::transport::Boot::new(|| crate::secure_step(vmcb, regs, platform)) };
        let committed = decision
            .commit(&mut crate::recovery_faults::LostCommitAck::new(&mut owner))
            .unwrap_or_else(|_| {
                crate::log("monitor-runtime: firmware commitment unresolved; recovery required\n");
                crate::halt();
            });
        assert_eq!(committed.committed(COMPONENT), identity);
        release(&mut owner);
        drop(owner);
        // Admit services only from a fresh boot of the authenticated committed
        // bytes, discarding every volatile trial mutation and pending request.
        let loaded = store::load(&mut disk, COMPONENT, identity, ROOT, 1, scratch).unwrap();
        unsafe { crate::secure_boot::restart(loaded.image, vmcb, regs, platform, false) };
    }
    let mut owner =
        unsafe { crate::transport::Boot::new(|| crate::secure_step(vmcb, regs, platform)) };
    assert_eq!(generation(&mut owner), identity.generation);
    let mut avb = Avb::connect(owner).unwrap();
    avb.read_rollback(0).unwrap();
    assert_eq!(avb.read_rollback(27).unwrap(), 7);
    avb.close().unwrap();
    drop(avb);
    crate::log(if trial {
        "monitor-runtime: disk-selected Trusty trial committed and rebooted into secure services\n"
    } else {
        "monitor-runtime: committed distinct Trusty recovered and secure services continued\n"
    });
    crate::halt();
}
