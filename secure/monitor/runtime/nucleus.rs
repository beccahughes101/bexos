//! Permanent execution owner. Replaceable scheduling code receives values at
//! completed guest boundaries and returns a checked decision. It never owns
//! CPU, device, interrupt, DMA, transport state, or a suspended hardware action.
use bexos_secure_monitor::{
    image::{Image, ImageError},
    policy_image::{self as abi, Work},
};
#[path = "policy_guard.rs"]
mod guard;

const SECOND: usize = bexos_secure_monitor::monitor_image::INACTIVE_BANK;
const RETIRING: usize = bexos_secure_monitor::monitor_image::RETIRING_ALIAS;
use core::sync::atomic::{AtomicU64, Ordering};
static BOUNDARIES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// Published only after a real VMRUN exit and its entire hardware transaction
/// complete. A halted/non-runnable scheduling attempt cannot count as progress.
pub fn completed_boundary(domain: usize) {
    BOUNDARIES[domain]
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
        .unwrap();
}
pub struct Monitor {
    active: usize,
    tag: u8,
    previous: Option<(usize, u8)>,
    sequence: u64,
}
impl Monitor {
    pub unsafe fn initialize() -> Self {
        let bytes = include_bytes!(env!("MONITOR_POLICY"));
        let tag = unsafe { load(bytes, abi::BASE) }.expect("embedded monitor policy ABI");
        Self {
            active: abi::BASE,
            tag,
            previous: None,
            sequence: 0,
        }
    }
    pub unsafe fn prepare(&self, bytes: &[u8]) -> Result<u8, ImageError> {
        if self.previous.is_some() {
            return Err(ImageError::Destination);
        }
        let next = if self.active == abi::BASE {
            SECOND
        } else {
            abi::BASE
        };
        unsafe { crate::root::map_staging(next) };
        unsafe { load(bytes, RETIRING) }
    }
    pub unsafe fn activate(&mut self, tag: u8) {
        assert!(self.previous.is_none());
        let next = if self.active == abi::BASE {
            SECOND
        } else {
            abi::BASE
        };
        unsafe { crate::root::map_policy(next, self.active) };
        self.previous = Some((self.active, self.tag));
        self.active = next;
        self.tag = tag;
    }
    pub unsafe fn rollback(&mut self) {
        let (previous, tag) = self.previous.take().expect("retained committed monitor");
        unsafe { crate::root::map_policy(previous, self.active) };
        self.active = previous;
        self.tag = tag;
    }
    /// Call only after readiness and authenticated commitment. The previous
    /// image's code, writable data and stack share this reclaimable bank.
    pub unsafe fn reclaim(&mut self) {
        assert!(self.previous.take().is_some());
        let old = unsafe { core::slice::from_raw_parts_mut(RETIRING as *mut u8, abi::BYTES) };
        old.fill(0xa5);
        assert!(old.iter().all(|byte| *byte == 0xa5));
    }
    pub fn progress(&self) -> [u64; 2] {
        core::array::from_fn(|i| BOUNDARIES[i].load(Ordering::Acquire))
    }
    unsafe fn work(&mut self, pending: bool, budget_ns: u64) -> Option<Work> {
        let reply = unsafe { guard::call(self.sequence, pending, budget_ns) };
        let work = Work::decode(reply, self.tag)?;
        self.sequence = self.sequence.checked_add(1).unwrap();
        Some(work)
    }
    /// Cold trial service while normal clients are still excluded. Candidate
    /// decisions cannot cause a normal VMRUN before authenticated commitment.
    pub unsafe fn secure_turn(
        &mut self,
        vmcb: &mut bexos_secure_monitor::svm::Vmcb,
        regs: &mut bexos_secure_monitor::vcpu::Registers,
        platform: &mut crate::platform::Platform<1>,
    ) -> bool {
        let Some(work) = (unsafe { self.work(true, 150_000_000) }) else {
            return false;
        };
        for _ in 0..work.secure {
            unsafe { crate::secure_step(vmcb, regs, platform) };
        }
        true
    }
    pub unsafe fn turn(
        &mut self,
        #[cfg(feature = "normal_world")] normal: &mut crate::normal::Normal,
        vmcb: &mut bexos_secure_monitor::svm::Vmcb,
        regs: &mut bexos_secure_monitor::vcpu::Registers,
        platform: &mut crate::platform::Platform<1>,
        budget_ns: u64,
    ) -> bool {
        let pending = {
            #[cfg(feature = "normal_world")]
            {
                unsafe { crate::transport::needs_secure_progress() }
            }
            #[cfg(not(feature = "normal_world"))]
            {
                false
            }
        };
        let Some(work) = (unsafe { self.work(pending, budget_ns) }) else {
            return false;
        };
        // Guard is disarmed before either domain executes. No candidate code
        // can run or fault in the middle of these permanent transactions.
        #[cfg(feature = "normal_world")]
        for _ in 0..work.normal {
            unsafe { normal.step() };
            if unsafe { crate::transport::needs_secure_progress() } {
                break;
            }
        }
        for _ in 0..work.secure {
            unsafe { crate::secure_step(vmcb, regs, platform) };
        }
        true
    }
}
unsafe fn load(bytes: &[u8], destination: usize) -> Result<u8, ImageError> {
    let image = Image::parse(bytes, abi::END)?;
    let tag = abi::validate(&image)?;
    let bank = unsafe { core::slice::from_raw_parts_mut(destination as *mut u8, abi::BYTES) };
    bank.fill(0);
    image.load_region(abi::BASE, bank)?;
    Ok(tag)
}
