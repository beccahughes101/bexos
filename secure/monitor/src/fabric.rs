//! Interrupt fabric confined to one domain. IPI destinations resolve only to
//! this domain's vCPUs; INIT/SIPI requests never target physical processors.
#[path = "fabric_state.rs"]
mod state;
pub use state::CheckpointIdentity;
#[cfg(test)]
#[path = "fabric_state_tests.rs"]
mod state_tests;
use crate::{
    hpet::Hpet,
    ioapic::IoApic,
    lapic::{Ipi, IpiKind, LocalApic},
    legacy_irq::LegacyIrq,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuState {
    Running,
    WaitingForStartup,
    Start(u8),
}
#[derive(Clone, Copy)]
pub struct Delivery {
    pub vector: u8,
    pic: bool,
}
pub struct Fabric<const N: usize> {
    apics: [LocalApic; N],
    states: [CpuState; N],
    pub hpet: Hpet,
    pub ioapic: IoApic,
    pub legacy: LegacyIrq,
}
impl<const N: usize> Fabric<N> {
    pub fn new() -> Option<Self> {
        if N == 0 || N > 32 {
            return None;
        }
        let mut states = [CpuState::WaitingForStartup; N];
        states[0] = CpuState::Running;
        Some(Self {
            apics: core::array::from_fn(|i| LocalApic::new(i as u8)),
            states,
            hpet: Hpet::new(),
            ioapic: IoApic::new(N as u8)?,
            legacy: LegacyIrq::new(),
        })
    }
    pub fn state(&self, cpu: usize) -> Option<CpuState> {
        self.states.get(cpu).copied()
    }
    /// Called after initializing the stopped vCPU from the accepted SIPI state.
    pub fn started(&mut self, cpu: usize, vector: u8) -> bool {
        if self.state(cpu) != Some(CpuState::Start(vector)) {
            return false;
        }
        self.states[cpu] = CpuState::Running;
        true
    }
    fn ipi(&mut self, sender: usize, request: Ipi) -> bool {
        if sender >= N || request.shorthand == 0 && usize::from(request.destination) >= N {
            return false;
        }
        for target in 0..N {
            let selected = match request.shorthand {
                0 => target == usize::from(request.destination),
                1 => target == sender,
                2 => true,
                3 => target != sender,
                _ => return false,
            };
            if !selected {
                continue;
            }
            match request.kind {
                IpiKind::Init => {
                    self.apics[target] = LocalApic::new(target as u8);
                    self.states[target] = CpuState::WaitingForStartup;
                }
                IpiKind::Startup(vector) => {
                    if self.states[target] == CpuState::WaitingForStartup {
                        self.states[target] = CpuState::Start(vector);
                    }
                }
                IpiKind::Fixed(vector) => {
                    self.apics[target].raise(vector);
                }
            }
        }
        true
    }
    pub fn read_lapic(&self, cpu: usize, offset: u16, now: u64) -> Option<u32> {
        self.apics.get(cpu)?.read(offset, now)
    }
    pub fn write_lapic(&mut self, cpu: usize, offset: u16, value: u32, now: u64) -> bool {
        let Some(apic) = self.apics.get_mut(cpu) else {
            return false;
        };
        match apic.write(offset, value, now) {
            Ok(None) => true,
            Ok(Some(ipi)) => self.ipi(cpu, ipi),
            Err(()) => false,
        }
    }
    pub fn before_entry(&mut self, cpu: usize, now: u64) -> Option<Delivery> {
        if self.state(cpu) != Some(CpuState::Running) {
            return None;
        }
        if let Some(gsi) = self.hpet.tick(now) {
            if let Some((destination, vector)) = self.ioapic.route(gsi) {
                self.apics[usize::from(destination)].raise(vector);
            }
        }
        self.legacy.tick(now);
        self.apics[cpu].tick(now);
        if let Some(vector) = self.apics[cpu].pending() {
            Some(Delivery { vector, pic: false })
        } else if cpu == 0 {
            self.legacy
                .pending()
                .map(|vector| Delivery { vector, pic: true })
        } else {
            None
        }
    }
    pub fn delivered(&mut self, cpu: usize, delivery: Delivery) -> bool {
        let Some(apic) = self.apics.get_mut(cpu) else {
            return false;
        };
        if delivery.pic {
            if cpu != 0 {
                return false;
            }
            self.legacy.delivered(delivery.vector);
        } else {
            apic.delivered(delivery.vector);
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_cpu_startup_and_ipi_delivery_stay_in_the_calling_domain() {
        let mut fabric = Fabric::<4>::new().unwrap();
        let other = Fabric::<4>::new().unwrap();
        for cpu in 1..4 {
            assert!(fabric.write_lapic(0, 0x310, (cpu as u32) << 24, 0));
            assert!(fabric.write_lapic(0, 0x300, 0xc500, 0));
            assert!(fabric.write_lapic(0, 0x300, 0x8500, 0));
            assert!(fabric.write_lapic(0, 0x300, 0x608, 0));
            assert_eq!(fabric.state(cpu), Some(CpuState::Start(8)));
            assert!(fabric.started(cpu, 8));
            assert!(fabric.write_lapic(0, 0x300, 0x609, 0)); // Repeated SIPI cannot restart running code.
            assert_eq!(fabric.state(cpu), Some(CpuState::Running));
            assert_eq!(other.state(cpu), Some(CpuState::WaitingForStartup));
            assert!(fabric.write_lapic(cpu, 0xf0, 0x1ff, 0));
        }
        assert!(fabric.write_lapic(0, 0x300, (3 << 18) | 33, 0));
        for cpu in 1..4 {
            assert_eq!(fabric.before_entry(cpu, 1).unwrap().vector, 33);
        }
        assert!(fabric.write_lapic(0, 0x310, 4 << 24, 0));
        assert!(!fabric.write_lapic(0, 0x300, 33, 0));
        assert_eq!(other.read_lapic(0, 0x210, 0), Some(0));
    }
    #[test]
    fn hpet_interrupt_traverses_virtual_ioapic_to_selected_cpu() {
        let mut fabric = Fabric::<4>::new().unwrap();
        assert!(fabric.write_lapic(0, 0xf0, 0x1ff, 0));
        assert!(fabric.ioapic.write(0, 0x30));
        assert!(fabric.ioapic.write(0x10, 80));
        assert!(fabric.hpet.write(0x10, 1, 0));
        assert!(fabric.hpet.write(0x100, (16 << 9) | 4, 0));
        assert!(fabric.hpet.write(0x108, 100, 0));
        assert!(fabric.before_entry(0, 999).is_none());
        let delivery = fabric.before_entry(0, 1000).unwrap();
        assert_eq!(delivery.vector, 80);
        assert!(fabric.delivered(0, delivery));
        assert!(fabric.before_entry(0, 1001).is_none());
        assert!(fabric.write_lapic(0, 0xb0, 0, 1001));
        assert!(fabric.before_entry(0, 1002).is_none());
    }
}
