//! Q35 IOAPIC redirection and level-interrupt acknowledgement.
use super::io;
use crate::state::Global;
use core::sync::atomic::{AtomicU64, Ordering};

struct Controller {
    base: u64,
    pins: u32,
}
static CONTROLLER: Global<Controller> = Global::new(Controller { base: 0, pins: 0 });
static PENDING: AtomicU64 = AtomicU64::new(0);

impl Controller {
    fn read(&self, register: u32) -> u32 {
        unsafe {
            io::write32(self.base, register);
            io::read32(self.base + 16)
        }
    }
    fn write(&self, register: u32, value: u32) {
        unsafe {
            io::write32(self.base, register);
            io::write32(self.base + 16, value);
        }
    }
}
/// Called with interrupts masked; Q35 exposes one IOAPIC starting at GSI zero.
pub fn initialize(base: u64, mask_all: bool) {
    CONTROLLER.with(|controller| {
        controller.base = base;
        controller.pins = ((controller.read(1) >> 16) & 255) + 1;
        assert!(controller.pins <= 64, "unsupported IOAPIC pin count");
        if mask_all {
            for pin in 0..controller.pins {
                controller.write(0x10 + pin * 2, 1 << 16);
            }
        }
    });
}
pub fn base() -> u64 {
    CONTROLLER.with(|controller| controller.base)
}
/// Install a physical APIC destination with explicit trigger and polarity.
/// The caller must mask local interrupts while accessing IOREGSEL/IOWIN.
pub fn route(gsi: u32, apic: u8, active_low: bool, level: bool) -> bool {
    CONTROLLER.with(|controller| {
        if controller.base == 0 || gsi >= controller.pins {
            return false;
        }
        let register = 0x10 + gsi * 2;
        controller.write(register, 1 << 16);
        controller.write(register + 1, u32::from(apic) << 24);
        controller.write(
            register,
            64 + gsi | (u32::from(active_low) << 13) | (u32::from(level) << 15),
        );
        true
    })
}
pub fn mask(gsi: u32, masked: bool) -> bool {
    CONTROLLER.with(|controller| {
        if controller.base == 0 || gsi >= controller.pins {
            return false;
        }
        let register = 0x10 + gsi * 2;
        let previous = controller.read(register);
        controller.write(
            register,
            if masked {
                previous | (1 << 16)
            } else {
                previous & !(1 << 16)
            },
        );
        true
    })
}
/// Mask before LAPIC EOI so a level source cannot re-enter until acknowledged.
pub fn dispatch(vector: u64) -> bool {
    if !(64..128).contains(&vector) {
        return false;
    }
    let gsi = (vector - 64) as u32;
    if !mask(gsi, true) {
        return false;
    }
    PENDING.fetch_or(1u64 << gsi, Ordering::Release);
    true
}
pub fn pending(gsi: u32) -> bool {
    gsi < 64 && PENDING.load(Ordering::Acquire) & (1u64 << gsi) != 0
}
pub fn acknowledge(gsi: u32) -> bool {
    if gsi >= 64 {
        return false;
    }
    PENDING.fetch_and(!(1u64 << gsi), Ordering::AcqRel);
    mask(gsi, false)
}
