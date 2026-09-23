//! Product firmware transactions run only at permanent guest boundaries.
//! No registry borrow or candidate upload reference is held while guests run.
use crate::{normal::Normal, nucleus::Monitor, platform::Platform};
use bexos_secure_firmware::{
    Architecture, Component,
    selection::State,
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
static mut TRANSPORT_GENERATION: u64 = 1;
const TRUSTY_MIGRATION_ABI: u64 = 1;
const REQUIRED_TRUSTY_SERVICES: u64 = 0x3f;
const ROOT_GENERATION: u32 = 0x8000_0003;
const ROOT_MIGRATION_ABI: u32 = 0x8000_0004;
const ROOT_SERVICE_PROBE: u32 = 0x8000_0005;
pub fn query(field: u64) -> Result<u64, Status> {
    if field == abi::QUERY_CAPABILITIES {
        return Ok(abi::CAP_TRUSTY_REBOOT
            | abi::CAP_TRUSTY_LIVE
            | abi::CAP_MONITOR_REBOOT
            | abi::CAP_MONITOR_LIVE);
    }
    if field == abi::QUERY_MIGRATION_ABI {
        return Ok(TRUSTY_MIGRATION_ABI);
    }
    if field == abi::QUERY_TRANSPORT_GENERATION {
        return Ok(unsafe { TRANSPORT_GENERATION });
    }
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

fn trusty_probe(candidate: &mut crate::trusty_owner::Candidate) -> Result<(u64, u64, u64), ()> {
    use bexos_trusty_boot::ql::Transport;
    let mut owner = unsafe { crate::transport::Boot::candidate(|| candidate.step()) };
    let mut word = |operation| {
        let mut bytes = [0; 8];
        (owner.exchange(operation, bytes.len(), &mut bytes) == Ok(8))
            .then(|| u64::from_le_bytes(bytes))
            .ok_or(())
    };
    let result = (
        word(ROOT_GENERATION),
        word(ROOT_MIGRATION_ABI),
        word(ROOT_SERVICE_PROBE),
    );
    drop(owner);
    if !candidate.healthy() {
        return Err(());
    }
    Ok((result.0?, result.1?, result.2?))
}

fn abort_trusty(
    decision: recovery::Boot,
    world: &RefCell<World<'_>>,
    component: Component,
    generation: u64,
    slot: u64,
) {
    crate::secure_boot::finish_live_trial();
    unsafe {
        crate::transport::finish_candidate();
    }
    let mut owner = unsafe { crate::transport::Boot::new(|| world.borrow_mut().step()) };
    let state = required(decision.abort(&mut owner));
    if state.pending.is_some() {
        recovery_required();
    }
    record(abi::ROLLED_BACK, component, generation, slot);
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
    unsafe fn activate_trusty(&mut self, candidate: &mut crate::trusty_owner::Candidate) {
        unsafe { candidate.activate(self.vmcb, self.regs, self.platform) };
    }
    unsafe fn rollback_trusty(&mut self, candidate: &mut crate::trusty_owner::Candidate) {
        unsafe { candidate.rollback(self.vmcb, self.regs, self.platform) };
    }
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
    fn architecture(&self) -> bexos_secure_firmware::Architecture {
        bexos_secure_firmware::Architecture::X86_64
    }
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
    if component == Component::Trusty {
        let decision = required(recovery::begin_live(&mut owner, component, image));
        drop(owner);
        let mut candidate = match unsafe {
            crate::trusty_owner::Candidate::prepare(
                verified.image,
                world.borrow().platform.memory_base(),
            )
        } {
            Ok(candidate) => candidate,
            Err(_) => {
                abort_trusty(
                    decision,
                    &world,
                    component,
                    generation,
                    image.slot.number() as u64,
                );
                return;
            }
        };
        unsafe {
            crate::transport::begin_candidate(candidate.bank());
        }
        crate::secure_boot::begin_live_trial(candidate.bank());
        let prepared = now().saturating_sub(started) < 30_000_000_000
            && trusty_probe(&mut candidate)
                == Ok((generation, TRUSTY_MIGRATION_ABI, REQUIRED_TRUSTY_SERVICES));
        if !prepared {
            abort_trusty(
                decision,
                &world,
                component,
                generation,
                image.slot.number() as u64,
            );
            return;
        }
        crate::log(
            "monitor-runtime: distinct Trusty candidate executed with compatible migration and required services\n",
        );
        let cutover = now();
        {
            let mut w = world.borrow_mut();
            unsafe {
                w.activate_trusty(&mut candidate);
            }
        }
        let healthy = trusty_probe(&mut candidate)
            == Ok((generation, TRUSTY_MIGRATION_ABI, REQUIRED_TRUSTY_SERVICES))
            && now().saturating_sub(cutover) <= 150_000_000;
        let readiness = now().saturating_sub(cutover);
        if !healthy {
            let mut w = world.borrow_mut();
            unsafe {
                w.rollback_trusty(&mut candidate);
            }
            drop(w);
            abort_trusty(
                decision,
                &world,
                component,
                generation,
                image.slot.number() as u64,
            );
            return;
        }
        // The retained source remains the sole persistent writer until the
        // protected selection commit resolves. Candidate RPMB stays fenced.
        {
            let mut w = world.borrow_mut();
            unsafe {
                w.rollback_trusty(&mut candidate);
            }
        }
        let mut owner = unsafe { crate::transport::Boot::new(|| world.borrow_mut().step()) };
        let resolved = required(decision.commit(&mut owner));
        drop(owner);
        if resolved.committed(component) != image {
            recovery_required();
        }
        {
            let mut w = world.borrow_mut();
            unsafe {
                w.activate_trusty(&mut candidate);
            }
        }
        crate::secure_boot::finish_live_trial();
        unsafe {
            crate::transport::finish_candidate();
        }
        crate::firmware_generations::select(resolved.committed);
        unsafe {
            TRANSPORT_GENERATION = TRANSPORT_GENERATION.checked_add(1).unwrap();
            candidate.reclaim();
        }
        record(
            abi::COMMITTED,
            component,
            generation,
            image.slot.number() as u64,
        );
        crate::log("monitor-runtime: live Trusty committed; retired private bank reclaimed\n");
        crate::log("monitor-runtime: Trusty cutover readiness ns=\n");
        crate::hex(readiness);
        return;
    }
    if component != Component::Hypervisor {
        recovery_required();
    }
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
