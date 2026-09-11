//! Native reboot-selection diagnostic: real signed slots and protected trials.
//! No normal-world clients run in this target. The selected candidate must
//! execute and pass Trusty IPC before its exact resident ticket is committed.
use bexos_secure_firmware::{Component, selection::Identity};
use bexos_trusty_boot::{ql::Transport, recovery, selection};
const COMPONENT: Component = Component::Hypervisor;
const ROOT: &[u8] = include_bytes!(env!("RECOVERY_ROOT"));
#[unsafe(link_section = ".resident.recovery")]
#[used]
static mut TICKET: [u8; 320] = [0; 320];
#[cfg(not(feature = "monitor_candidate"))]
static mut SCRATCH: [u8; bexos_secure_firmware::MAX_BUNDLE_BYTES] =
    [0; bexos_secure_firmware::MAX_BUNDLE_BYTES];

#[cfg(not(feature = "monitor_candidate"))]
pub unsafe fn boot(
    vmcb: &mut bexos_secure_monitor::svm::Vmcb,
    regs: &mut bexos_secure_monitor::vcpu::Registers,
    platform: &mut crate::platform::Platform<1>,
) -> ! {
    use bexos_secure_firmware::store;
    use bexos_secure_monitor::firmware_disk::{Disk, Native};
    let mut disk = Disk::identify(unsafe { Native::acquire() }).expect("root firmware disk");
    let scratch = unsafe { &mut *core::ptr::addr_of_mut!(SCRATCH) };
    let mut owner =
        unsafe { crate::transport::Boot::new(|| crate::secure_step(vmcb, regs, platform)) };
    let state = selection::query(&mut owner).unwrap();
    if state.revision == 1 {
        recovery::stage(
            &mut crate::recovery_faults::staging::Pending(&mut owner),
            &mut crate::recovery_faults::staging::Disk::new(&mut disk),
            COMPONENT,
            include_bytes!(env!("MONITOR_CANDIDATE")),
            ROOT,
            1,
            scratch,
        )
        .unwrap();
        crate::log(
            "monitor-runtime: signed monitor inactive slot flushed authenticated and pending\n",
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
        crate::log(
            "monitor-runtime: exhausted or invalid monitor trial rolled back authentically\n",
        );
        crate::halt();
    }
    let mut ticket = [0; 320];
    ticket[..8].copy_from_slice(b"BEXRC001");
    ticket[8..64].copy_from_slice(&identity.encode());
    ticket[64..].copy_from_slice(&decision.commit_request().unwrap_or([0; 256]));
    unsafe { core::ptr::write_volatile(core::ptr::addr_of_mut!(TICKET), ticket) };
    drop(owner);
    let loaded = store::load(&mut disk, COMPONENT, identity, ROOT, 1, scratch).unwrap();
    crate::log(
        "monitor-runtime: authenticated reboot-selected monitor loaded from firmware disk\n",
    );
    crate::recovery_faults::interrupt(1);
    unsafe { crate::monitor_transfer::transfer_image(loaded.image, vmcb, regs, platform) }
}

#[cfg(feature = "monitor_candidate")]
pub fn ready(transport: &mut impl Transport) {
    // The previous executable wrote this resident object. A normal load lets
    // whole-program optimization infer this candidate's zero initializer.
    let ticket = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(TICKET)) };
    assert_eq!(&ticket[..8], b"BEXRC001");
    let identity = Identity::decode(&ticket[8..64], false).unwrap();
    let transport = &mut crate::recovery_faults::LostCommitAck::new(transport);
    let state = if ticket[64..] != [0; 256] {
        recovery::commit_prepared(transport, ticket[64..].try_into().unwrap()).unwrap_or_else(
            |_| {
                crate::log("monitor-runtime: firmware commitment unresolved; recovery required\n");
                crate::halt();
            },
        )
    } else {
        selection::query(transport).unwrap()
    };
    assert_eq!(state.committed(COMPONENT), identity);
    assert!(state.pending.is_none());
    crate::log(if ticket[64..] != [0; 256] {
        "monitor-runtime: executed selected monitor trial committed through authenticated read\n"
    } else {
        "monitor-runtime: executed committed monitor recovered from persistent firmware slot\n"
    });
}
