//! Restricted recovery before any normal-world client is admitted. The signed
//! EFI nucleus always uses its embedded Trusty only to authenticate selection;
//! that instance is discarded before selected firmware exposes services.
use crate::{nucleus::Monitor, platform::Platform};
use bexos_secure_firmware::{Component, selection::Identity, store};
use bexos_secure_monitor::{
    firmware_disk::{Disk, Native},
    svm::Vmcb,
    vcpu::Registers,
};
use bexos_trusty_boot::{avb::Avb, ql::Transport, recovery, selection};
const ROOT: &[u8] = include_bytes!(env!("VERIFIED_ROOT"));

fn required<T, E>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|_| recovery_required())
}
fn recovery_required() -> ! {
    crate::log("monitor-runtime: firmware recovery required; no older image admitted\n");
    crate::halt()
}
fn release(owner: &mut impl Transport) {
    if owner.exchange(0x80000000, 1, &mut [1]) != Ok(0) {
        recovery_required();
    }
}
fn generation(owner: &mut impl Transport) -> Option<u64> {
    let mut bytes = [0; 8];
    (owner.exchange(0x80000003, 8, &mut bytes) == Ok(8)).then(|| u64::from_le_bytes(bytes))
}
unsafe fn restart_selected(
    disk: &mut Disk<Native>,
    identity: Identity,
    floor: u64,
    scratch: &mut [u8],
    vmcb: &mut Vmcb,
    regs: &mut Registers,
    platform: &mut Platform<1>,
    fenced: bool,
) {
    if identity == Identity::INITIAL {
        unsafe { crate::secure_boot::restart(crate::TRUSTY, vmcb, regs, platform, fenced) };
    } else {
        let loaded = required(store::load(
            disk,
            Component::Trusty,
            identity,
            ROOT,
            floor,
            scratch,
        ));
        unsafe { crate::secure_boot::restart(loaded.image, vmcb, regs, platform, fenced) };
    }
}

pub unsafe fn select(vmcb: &mut Vmcb, regs: &mut Registers, platform: &mut Platform<1>) -> Monitor {
    crate::log("monitor-runtime: firmware selection identify disk\n");
    let mut disk = required(Disk::identify(unsafe { Native::acquire() }));
    crate::log("monitor-runtime: firmware selection disk ready\n");
    let scratch = unsafe { crate::replacement::boot_workspace() };
    let mut monitor = unsafe { Monitor::initialize() };
    crate::log("monitor-runtime: firmware selection connect rollback service\n");
    let mut avb = required(Avb::connect(unsafe {
        crate::transport::Boot::cold(|| crate::secure_step(vmcb, regs, platform))
    }));
    let floors = [
        required(avb.read_rollback(30)).max(1),
        required(avb.read_rollback(31)).max(1),
    ];
    required(avb.close());
    drop(avb);
    crate::log("monitor-runtime: firmware selection rollback floors ready\n");
    let mut owner =
        unsafe { crate::transport::Boot::cold(|| crate::secure_step(vmcb, regs, platform)) };
    crate::log("monitor-runtime: firmware selection prepare protected state\n");
    let decision = required(recovery::prepare_boot(
        &mut owner, &mut disk, ROOT, floors, scratch,
    ));
    crate::log("monitor-runtime: firmware selection protected state ready\n");
    let mut selected = [
        decision.selected(Component::Trusty),
        decision.selected(Component::Hypervisor),
    ];
    let trial = decision.trial();
    release(&mut owner);
    drop(owner);
    let committed_monitor = decision.committed(Component::Hypervisor);
    if committed_monitor != Identity::INITIAL {
        crate::log("monitor-runtime: loading committed monitor policy from protected selection\n");
        let loaded = required(store::load(
            &mut disk,
            Component::Hypervisor,
            committed_monitor,
            ROOT,
            floors[1],
            scratch,
        ));
        crate::log(
            "monitor-runtime: committed monitor policy authenticated from persistent slot\n",
        );
        let tag = required(unsafe { monitor.prepare(loaded.image) });
        crate::log("monitor-runtime: committed monitor policy prepared in inactive bank\n");
        unsafe { monitor.activate(tag) };
        crate::log("monitor-runtime: committed monitor policy entered\n");
        for _ in 0..6 {
            if !unsafe { monitor.secure_turn(vmcb, regs, platform) } {
                recovery_required();
            }
        }
        // The authenticated committed image, not the embedded baseline, is
        // the retained rollback destination for every subsequent trial.
        unsafe { monitor.reclaim() };
        crate::log("monitor-runtime: embedded monitor policy retired after committed entry\n");
    }
    let mut retained_monitor = selected[1] != committed_monitor;
    if retained_monitor {
        let loaded = required(store::load(
            &mut disk,
            Component::Hypervisor,
            selected[1],
            ROOT,
            floors[1],
            scratch,
        ));
        let tag = required(unsafe { monitor.prepare(loaded.image) });
        unsafe { monitor.activate(tag) };
    }
    if let Some((component, _)) = trial {
        unsafe {
            restart_selected(
                &mut disk,
                selected[0],
                floors[0],
                scratch,
                vmcb,
                regs,
                platform,
                true,
            )
        };
        let started = unsafe { bexos_secure_monitor::clock::now_ns() };
        let mut healthy = true;
        for _ in 0..6 {
            if !unsafe { monitor.secure_turn(vmcb, regs, platform) } {
                healthy = false;
                break;
            }
        }
        if healthy {
            let mut policy_healthy = true;
            let service_healthy = {
                let mut owner = unsafe {
                    crate::transport::Boot::cold(|| {
                        if policy_healthy {
                            policy_healthy = monitor.secure_turn(vmcb, regs, platform);
                        } else {
                            crate::secure_step(vmcb, regs, platform);
                        }
                    })
                };
                let matching = generation(&mut owner) == Some(selected[0].generation);
                match Avb::connect(owner) {
                    Ok(mut avb) => {
                        let readable = avb.read_rollback(0).is_ok();
                        matching && readable && avb.close().is_ok()
                    }
                    Err(_) => false,
                }
            };
            healthy =
                service_healthy && policy_healthy && crate::secure_boot::completed_reads() > 0;
        }
        healthy &= unsafe { bexos_secure_monitor::clock::now_ns() }
            .checked_sub(started)
            .is_some_and(|elapsed| elapsed < 600_000_000_000);
        crate::log(if healthy {
            "monitor-runtime: selected firmware trial healthy with persistent services fenced\n"
        } else {
            "monitor-runtime: selected firmware trial failed before client admission\n"
        });
        // Nothing from the fenced trial can remain queued when persistent
        // access is restored. Recovery code resolves the protected transaction.
        unsafe { crate::secure_boot::restart(crate::TRUSTY, vmcb, regs, platform, false) };
        let mut owner =
            unsafe { crate::transport::Boot::cold(|| crate::secure_step(vmcb, regs, platform)) };
        let state = if healthy {
            required(decision.commit(&mut owner))
        } else {
            required(decision.abort(&mut owner))
        };
        selected = state.committed;
        if !healthy && component == Component::Hypervisor {
            unsafe { monitor.rollback() };
            retained_monitor = false;
        }
        release(&mut owner);
        drop(owner);
    }
    // Even generation one gets a fresh service instance. Recovery's unsealed
    // boot authority and any volatile trial state never reach normal clients.
    unsafe {
        restart_selected(
            &mut disk,
            selected[0],
            floors[0],
            scratch,
            vmcb,
            regs,
            platform,
            false,
        )
    };
    for _ in 0..6 {
        if !unsafe { monitor.secure_turn(vmcb, regs, platform) } {
            recovery_required();
        }
    }
    let mut owner =
        unsafe { crate::transport::Boot::cold(|| crate::secure_step(vmcb, regs, platform)) };
    if generation(&mut owner) != Some(selected[0].generation) {
        recovery_required();
    }
    let observed = required(selection::query(&mut owner));
    if observed.pending.is_some() || observed.committed != selected {
        recovery_required();
    }
    crate::replacement::activation::boot_result(&observed);
    drop(owner);
    if retained_monitor {
        unsafe { monitor.reclaim() };
    }
    crate::firmware_generations::select(selected);
    crate::log("monitor-runtime: selected monitor generation=\n");
    crate::hex(selected[1].generation);
    scratch.fill(0);
    crate::log(
        "monitor-runtime: authenticated firmware selection complete; recovery instance discarded\n",
    );
    monitor
}
