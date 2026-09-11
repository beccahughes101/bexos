//! Signed live trials against the actual protected journal. Candidate failure
//! occurs after guest execution, so unchanged nucleus counters alone cannot
//! pass: the retained secure client must also complete its pending operation.
use crate::{nucleus::Monitor, platform::Platform};
use bexos_secure_firmware::{Component, selection::Identity, store};
use bexos_secure_monitor::{
    firmware_disk::{Disk, Native},
    svm::Vmcb,
    vcpu::Registers,
};
use bexos_trusty_boot::{avb::Avb, recovery};
const ROOT: &[u8] = include_bytes!(env!("RECOVERY_ROOT"));
static mut SCRATCH: [u8; 64 * 1024] = [0; 64 * 1024];

pub unsafe fn exercise(
    monitor: &mut Monitor,
    #[cfg(feature = "normal_world")] normal: &mut crate::normal::Normal,
    vmcb: &mut Vmcb,
    regs: &mut Registers,
    platform: &mut Platform<1>,
) {
    let mut disk = Disk::identify(unsafe { Native::acquire() }).unwrap();
    let scratch = unsafe { &mut *core::ptr::addr_of_mut!(SCRATCH) };
    // Product boot reads selection before admitting normal clients. Establish
    // that same protected-store startup boundary before timing live preparation.
    let mut recovery_owner =
        unsafe { crate::transport::Boot::cold(|| crate::secure_step(vmcb, regs, platform)) };
    bexos_trusty_boot::selection::query(&mut recovery_owner).unwrap();
    drop(recovery_owner);
    for (bundle, failure) in [
        (&include_bytes!(env!("POLICY_FAULT"))[..], true),
        (&include_bytes!(env!("POLICY_HANG"))[..], true),
        (&include_bytes!(env!("POLICY_CANDIDATE"))[..], false),
        (&include_bytes!(env!("POLICY_SUCCESSOR"))[..], false),
    ] {
        let mut avb = Avb::connect(unsafe {
            crate::transport::Boot::new(|| {
                #[cfg(feature = "normal_world")]
                crate::schedule::normal_turn(normal);
                crate::secure_step(vmcb, regs, platform);
            })
        })
        .unwrap();
        let floor = avb.read_rollback(0).unwrap();
        avb.begin_read_rollback(0).unwrap();
        let (owner, client) = avb.suspend();
        unsafe { owner.detach() };
        #[cfg(feature = "normal_world")]
        let mut preparation_turns = 0u64;
        let mut owner = unsafe {
            crate::transport::Boot::resume(|| {
                #[cfg(feature = "normal_world")]
                {
                    if preparation_turns & 31 == 0 {
                        normal.step();
                    }
                    preparation_turns = preparation_turns.wrapping_add(1);
                }
                crate::secure_step(vmcb, regs, platform);
            })
        };
        let start = unsafe { bexos_secure_monitor::clock::now_ns() };
        recovery::stage(
            &mut owner,
            &mut disk,
            Component::Hypervisor,
            bundle,
            ROOT,
            1,
            scratch,
        )
        .unwrap();
        let decision =
            recovery::prepare_boot(&mut owner, &mut disk, ROOT, [1, 1], scratch).unwrap();
        let identity = decision.selected(Component::Hypervisor);
        assert_ne!(identity, Identity::INITIAL);
        let loaded =
            store::load(&mut disk, Component::Hypervisor, identity, ROOT, 1, scratch).unwrap();
        let tag = unsafe { monitor.prepare(loaded.image) }.unwrap();
        unsafe { owner.detach() };
        crate::log("monitor-runtime: nucleus signed preparation duration ns=\n");
        crate::hex(
            unsafe { bexos_secure_monitor::clock::now_ns() }
                .checked_sub(start)
                .unwrap(),
        );
        assert!(
            unsafe { bexos_secure_monitor::clock::now_ns() }
                .checked_sub(start)
                .unwrap()
                < 30_000_000_000
        );
        let before = monitor.progress();
        let cutover = unsafe { bexos_secure_monitor::clock::now_ns() };
        unsafe { monitor.activate(tag) };
        let mut failed = false;
        for _ in 0..6 {
            let elapsed = unsafe { bexos_secure_monitor::clock::now_ns() }
                .checked_sub(cutover)
                .unwrap();
            if elapsed >= 150_000_000
                || !unsafe {
                    monitor.turn(
                        #[cfg(feature = "normal_world")]
                        normal,
                        vmcb,
                        regs,
                        platform,
                        150_000_000 - elapsed,
                    )
                }
            {
                failed = true;
                break;
            }
        }
        assert_eq!(failed, failure);
        let after = monitor.progress();
        assert!(after[1] > before[1]);
        #[cfg(feature = "normal_world")]
        assert!(after[0] > before[0]);
        if failed {
            unsafe { monitor.rollback() };
            assert_eq!(monitor.progress(), after);
            assert!(unsafe {
                monitor.turn(
                    #[cfg(feature = "normal_world")]
                    normal,
                    vmcb,
                    regs,
                    platform,
                    150_000_000,
                )
            });
        }
        let mut avb = unsafe {
            Avb::restore_protected(
                crate::transport::Boot::resume(|| {
                    crate::secure_step(vmcb, regs, platform);
                }),
                &client,
            )
        }
        .unwrap();
        assert_eq!(avb.finish_read_rollback().unwrap(), floor);
        avb.close().unwrap();
        drop(avb);
        let readiness = unsafe { bexos_secure_monitor::clock::now_ns() }
            .checked_sub(cutover)
            .unwrap();
        let mut owner = unsafe {
            crate::transport::Boot::new(|| {
                crate::secure_step(vmcb, regs, platform);
            })
        };
        if failed {
            decision.abort(&mut owner).unwrap();
            crate::log(
                "monitor-runtime: resident nucleus recovered candidate failure after guest progress without restoring guest state\n",
            );
        } else {
            assert!(readiness <= 150_000_000);
            crate::log(
                "monitor-runtime: resident nucleus distinct monitor retained client readiness ns=\n",
            );
            crate::hex(readiness);
            let committed = decision
                .commit(&mut crate::recovery_faults::LostCommitAck::new(&mut owner))
                .unwrap();
            assert_eq!(committed.committed(Component::Hypervisor), identity);
            unsafe { monitor.reclaim() };
            crate::log(
                "monitor-runtime: authenticated distinct monitor committed before old code data and stack reuse\n",
            );
        }
        drop(owner);
        let mut avb = Avb::connect(unsafe {
            crate::transport::Boot::new(|| {
                crate::secure_step(vmcb, regs, platform);
            })
        })
        .unwrap();
        assert_eq!(avb.read_rollback(0).unwrap(), floor);
        avb.close().unwrap();
    }
    crate::log(
        "monitor-runtime: nucleus repeated signed replacement and post-progress fault and hang recovery verified\n",
    );
}
