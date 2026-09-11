//! Product firmware transactions run only at permanent guest boundaries.
//! No registry borrow or candidate upload reference is held while guests run.
use crate::{normal::Normal, nucleus::Monitor, platform::Platform};
use bexos_secure_firmware::{
    Architecture, Component,
    selection::{Identity, State},
    store::{self, BlockDevice},
};
use bexos_secure_monitor::{
    firmware_disk::{Disk, Native},
    svm::Vmcb,
    vcpu::Registers,
};
use bexos_secure_monitor_abi::{Status, firmware as abi};
use bexos_trusty_boot::{avb::Avb, recovery};
use core::cell::RefCell;

const ROOT: &[u8] = include_bytes!(env!("VERIFIED_ROOT"));
static mut REPORT: [u64; 4] = [abi::IDLE, 0, 0, 0];
static mut REVISION: u64 = 0;
pub fn query(field: u64) -> Result<u64, Status> {
    if field == abi::QUERY_REVISION {
        return Ok(unsafe { REVISION });
    }
    if (abi::QUERY_TRUSTY_GENERATION..=abi::QUERY_MONITOR_GENERATION).contains(&field) {
        return Ok(crate::firmware_generations::active_generations()
            [(field - abi::QUERY_TRUSTY_GENERATION) as usize]);
    }
    unsafe { (&*core::ptr::addr_of!(REPORT)).get(field as usize).copied() }
        .ok_or(Status::InvalidArgs)
}
pub fn busy() -> bool {
    matches!(
        query(0),
        Ok(abi::APPLYING | abi::PENDING | abi::RECOVERY_REQUIRED)
    )
}
pub fn record(outcome: u64, component: Component, generation: u64, slot: u64) {
    unsafe {
        REPORT = [
            outcome,
            generation,
            bexos_secure_firmware::selection::component_number(component) as u64,
            slot,
        ];
        REVISION = REVISION.wrapping_add(1);
    }
}
pub fn boot_result(state: &State) {
    if let Some((op, component, image)) = state.last_change() {
        record(
            if op == bexos_secure_firmware::selection::Operation::Abort {
                abi::ROLLED_BACK
            } else {
                abi::COMMITTED
            },
            component,
            image.generation,
            image.slot.number() as u64,
        );
    }
}
fn recovery_required() -> ! {
    unsafe {
        REPORT[0] = abi::RECOVERY_REQUIRED;
    }
    crate::log(
        "monitor-runtime: firmware recovery required; authenticated transaction unresolved\n",
    );
    crate::halt()
}
fn required<T, E>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|_| recovery_required())
}
fn now() -> u64 {
    unsafe { bexos_secure_monitor::clock::now_ns() }
}

struct World<'a> {
    monitor: &'a mut Monitor,
    normal: &'a mut Normal,
    vmcb: &'a mut Vmcb,
    regs: &'a mut Registers,
    platform: &'a mut Platform<1>,
    cutover: Option<u64>,
    trial: bool,
    committing: bool,
    failed: bool,
}
impl World<'_> {
    fn step(&mut self) {
        if self.failed && self.committing {
            // A commit may already be durable. Resident execution alone keeps
            // the storage transport moving until an authenticated read resolves
            // it; no previous image is selected on a lost acknowledgement.
            unsafe {
                self.normal.step();
                crate::secure_step(self.vmcb, self.regs, self.platform);
            }
            return;
        }
        let budget = self.cutover.map_or(150_000_000, |start| {
            150_000_000u64.saturating_sub(now().saturating_sub(start))
        });
        let okay = budget != 0
            && unsafe {
                self.monitor
                    .turn(self.normal, self.vmcb, self.regs, self.platform, budget)
            };
        if !okay {
            if !self.trial {
                recovery_required();
            }
            self.failed = true;
            self.cutover = None;
            if self.committing {
                self.step();
                return;
            }
            unsafe {
                self.monitor.rollback();
            }
            self.trial = false;
            if !unsafe {
                self.monitor.turn(
                    self.normal,
                    self.vmcb,
                    self.regs,
                    self.platform,
                    150_000_000,
                )
            } {
                recovery_required();
            }
        }
    }
}
struct Storage<'a, 'b> {
    disk: Disk<Native>,
    world: &'a RefCell<World<'b>>,
    started: u64,
    operations: usize,
}
impl Storage<'_, '_> {
    fn boundary(&mut self) -> Result<(), store::Error> {
        if now()
            .checked_sub(self.started)
            .is_none_or(|n| n >= 30_000_000_000)
        {
            return Err(store::Error::Device);
        }
        self.operations += 1;
        if self.operations & 2047 == 0 {
            self.world.borrow_mut().step();
        }
        Ok(())
    }
}
impl BlockDevice for Storage<'_, '_> {
    fn sectors(&self) -> u64 {
        self.disk.sectors()
    }
    fn read(&mut self, sector: u64, bytes: &mut [u8; 512]) -> Result<(), store::Error> {
        self.boundary()?;
        self.disk.read(sector, bytes)
    }
    fn write(&mut self, sector: u64, bytes: &[u8; 512]) -> Result<(), store::Error> {
        self.boundary()?;
        self.disk.write(sector, bytes)
    }
    fn flush(&mut self) -> Result<(), store::Error> {
        self.boundary()?;
        self.disk.flush()
    }
}
struct Erase<'a>(&'a mut [u8]);
impl Drop for Erase<'_> {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub unsafe fn run(
    monitor: &mut Monitor,
    normal: &mut Normal,
    vmcb: &mut Vmcb,
    regs: &mut Registers,
    platform: &mut Platform<1>,
) {
    let candidate = unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(super::CANDIDATE);
        if !slot.as_ref().is_some_and(|c| c.activation.is_some()) {
            return;
        }
        slot.take().unwrap()
    };
    let component = candidate.component;
    let generation = candidate.generation;
    let started = candidate.staging.started_ns();
    let mode = candidate.activation.unwrap();
    let Ok(bytes) = candidate.staging.release(candidate.owner, now()) else {
        record(abi::REJECTED, component, generation, 0);
        return;
    };
    let mut owned = Erase(bytes);
    let world = RefCell::new(World {
        monitor,
        normal,
        vmcb,
        regs,
        platform,
        cutover: None,
        trial: false,
        committing: false,
        failed: false,
    });
    let Ok(disk) = Disk::identify(unsafe { Native::acquire() }) else {
        record(abi::REJECTED, component, generation, 0);
        return;
    };
    let mut disk = Storage {
        disk,
        world: &world,
        started,
        operations: 0,
    };
    let floor = required(crate::firmware_generations::approved(component)).floor();
    let mut owner = unsafe { crate::transport::Boot::new(|| world.borrow_mut().step()) };
    let state =
        match recovery::stage_in_place(&mut owner, &mut disk, component, owned.0, ROOT, floor) {
            Ok(state) => state,
            Err(recovery::Error::Storage(_)) | Err(recovery::Error::Busy) => {
                record(abi::REJECTED, component, generation, 0);
                return;
            }
            Err(_) => recovery_required(),
        };
    let (pending_component, image) = state.pending.unwrap_or_else(|| recovery_required());
    if pending_component != component || image.generation != generation {
        recovery_required();
    }
    if mode == abi::ON_REBOOT {
        record(
            abi::PENDING,
            component,
            generation,
            image.slot.number() as u64,
        );
        crate::log(
            "monitor-runtime: authenticated firmware pending reboot; committed image retained\n",
        );
        return;
    }
    if component != Component::Hypervisor {
        recovery_required();
    }
    // The reread replaced the upload bytes. Reauthenticate that exact snapshot
    // before copying any executable segment into the inactive policy bank.
    let verified = required(bexos_secure_firmware::verify(
        owned.0,
        ROOT,
        Architecture::X86_64,
        component,
        state.committed(component).generation,
        floor,
    ));
    let tag = required(unsafe { world.borrow().monitor.prepare(verified.image) });
    let decision = required(recovery::begin_live_monitor(&mut owner, image));
    drop(owner);
    let mut avb = required(Avb::connect(unsafe {
        crate::transport::Boot::new(|| world.borrow_mut().step())
    }));
    let floor_before = required(avb.read_rollback(0));
    required(avb.begin_read_rollback(0));
    let (owner, client) = avb.suspend();
    unsafe {
        owner.detach();
    }
    let preparation_expired = now()
        .checked_sub(started)
        .is_none_or(|n| n >= 30_000_000_000);
    let before = world.borrow().monitor.progress();
    let cutover = now();
    if !preparation_expired {
        let mut w = world.borrow_mut();
        unsafe {
            w.monitor.activate(tag);
        }
        w.cutover = Some(cutover);
        w.trial = true;
    }
    for _ in 0..6 {
        world.borrow_mut().step();
    }
    let mut avb = required(unsafe {
        Avb::restore_protected(
            crate::transport::Boot::resume(|| world.borrow_mut().step()),
            &client,
        )
    });
    if required(avb.finish_read_rollback()) != floor_before {
        recovery_required();
    }
    let readiness = now().saturating_sub(cutover);
    let after = world.borrow().monitor.progress();
    let healthy = !preparation_expired
        && !world.borrow().failed
        && readiness <= 150_000_000
        && (0..2).all(|i| after[i] > before[i]);
    {
        let mut w = world.borrow_mut();
        if !healthy && w.trial {
            unsafe {
                w.monitor.rollback();
            }
            w.trial = false;
        }
        w.cutover = None;
    }
    required(avb.close());
    drop(avb);
    let healthy = healthy && !world.borrow().failed;
    world.borrow_mut().committing = healthy;
    let mut owner = unsafe { crate::transport::Boot::new(|| world.borrow_mut().step()) };
    let resolved = required(if healthy {
        decision.commit(&mut owner)
    } else {
        decision.abort(&mut owner)
    });
    if healthy {
        if resolved.committed(component) != image || world.borrow().failed {
            recovery_required();
        }
        crate::firmware_generations::select(resolved.committed);
        unsafe {
            world.borrow_mut().monitor.reclaim();
        }
        record(
            abi::COMMITTED,
            component,
            generation,
            image.slot.number() as u64,
        );
        crate::log(
            "monitor-runtime: product distinct monitor committed after retained service readiness ns=\n",
        );
        crate::hex(readiness);
        crate::log("monitor-runtime: product old monitor code data and stack reclaimed\n");
    } else {
        record(
            abi::ROLLED_BACK,
            component,
            generation,
            image.slot.number() as u64,
        );
        crate::log(
            "monitor-runtime: product monitor rolled back without rewinding guest progress\n",
        );
    }
}
