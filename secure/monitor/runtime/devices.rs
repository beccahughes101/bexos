//! Virtual MMIO and interrupt devices owned by one domain.
use crate::memory::DomainMemory;
use bexos_secure_monitor::{
    fabric::{Delivery, Fabric},
    mmio::{self, Operation},
    svm::Vmcb,
    vcpu::Registers,
};

pub struct Devices<const N: usize> {
    pub fabric: Fabric<N>,
    pub pci: Option<crate::pci::Pci>,
    pub(crate) armed: [Option<Delivery>; N],
    pub(crate) delivered: bool,
}
impl<const N: usize> Devices<N> {
    pub fn new() -> Self {
        Self {
            pci: None,
            fabric: Fabric::new().unwrap(),
            armed: [None; N],
            delivered: false,
        }
    }
    pub fn before_entry(&mut self, cpu: usize, vmcb: &mut Vmcb, now: u64) {
        self.armed[cpu] = self.fabric.before_entry(cpu, now);
        vmcb.virtual_interrupt(self.armed[cpu].map(|delivery| delivery.vector));
    }
    pub fn after_exit(&mut self, cpu: usize, vmcb: &Vmcb) {
        if let Some(delivery) = self.armed[cpu].take() {
            if !vmcb.virtual_interrupt_pending() {
                assert!(self.fabric.delivered(cpu, delivery));
                if !self.delivered {
                    crate::log("monitor-runtime: virtual timer interrupt delivered\n");
                    self.delivered = true;
                }
            }
        }
    }
    pub unsafe fn mmio(
        &mut self,
        cpu: usize,
        memory: DomainMemory,
        vmcb: &mut Vmcb,
        registers: &mut Registers,
        now: u64,
    ) -> bool {
        let (fault, physical) = vmcb.exit_info();
        if fault & 16 != 0 {
            return false;
        }
        let (code, length) = unsafe { memory.instruction(vmcb) };
        let mut values = registers.values(vmcb);
        let Some(access) = mmio::decode(&code[..length], vmcb.rip(), &values, vmcb.long_code())
        else {
            crate::log("monitor-runtime: unsupported MMIO instruction=\n");
            for byte in &code[..length.min(8)] {
                crate::hex(u64::from(*byte));
            }
            return false;
        };
        if unsafe { memory.resolve(vmcb, access.address) } != Some(physical) {
            return false;
        }
        match access.operation {
            Operation::Read { .. } | Operation::Test { .. } | Operation::Alu { .. }
                if fault & 2 == 0 =>
            {
                let value = match physical {
                    0xfee00000..0xfee01000 if access.bytes == 4 => self
                        .fabric
                        .read_lapic(cpu, (physical - 0xfee00000) as u16, now)
                        .map(u64::from),
                    0xfec00000..0xfec01000 if access.bytes == 4 => self
                        .fabric
                        .ioapic
                        .read((physical - 0xfec00000) as u16)
                        .map(u64::from),
                    0xfed00000..0xfed01000 if access.bytes == 8 => {
                        self.fabric.hpet.read((physical - 0xfed00000) as u16, now)
                    }
                    // No assigned PCI devices yet; bus zero is an empty virtual bus.
                    0xe0000000..0xe0100000 => {
                        if let Some(pci) = &self.pci {
                            unsafe { pci.read_config(physical, access.bytes) }
                        } else {
                            Some(u64::MAX)
                        }
                    }
                    _ => self
                        .pci
                        .as_ref()
                        .and_then(|pci| unsafe { pci.read(physical, access.bytes) }),
                };
                let Some(value) = value else {
                    return false;
                };
                if let Operation::Read { register, .. } = access.operation {
                    values[register as usize] = access
                        .read_result(values[register as usize], value)
                        .unwrap();
                    registers.set_values(vmcb, &values);
                } else if let Operation::Alu { register, .. } = access.operation {
                    let (result, flags) = access
                        .alu_result(values[register as usize], value, vmcb.rflags())
                        .unwrap();
                    values[register as usize] = result;
                    registers.set_values(vmcb, &values);
                    vmcb.set_rflags(flags);
                } else {
                    vmcb.set_rflags(access.test_flags(vmcb.rflags(), value).unwrap());
                }
            }
            Operation::Write { value } if fault & 2 != 0 => {
                let valid = match physical {
                    0xfee00000..0xfee01000 if access.bytes == 4 => self.fabric.write_lapic(
                        cpu,
                        (physical - 0xfee00000) as u16,
                        value as u32,
                        now,
                    ),
                    0xfec00000..0xfec01000 if access.bytes == 4 => self
                        .fabric
                        .ioapic
                        .write((physical - 0xfec00000) as u16, value as u32),
                    0xfed00000..0xfed01000 if access.bytes == 8 => {
                        self.fabric
                            .hpet
                            .write((physical - 0xfed00000) as u16, value, now)
                    }
                    0xe0000000..0xe0100000 => self.pci.as_mut().is_some_and(|pci| unsafe {
                        pci.write_config(physical, access.bytes, value)
                    }),
                    _ => self
                        .pci
                        .as_mut()
                        .is_some_and(|pci| unsafe { pci.write(physical, access.bytes, value) }),
                };
                if !valid {
                    crate::log("monitor-runtime: unsupported device write=\n");
                    crate::hex(physical);
                    crate::hex(value);
                    return false;
                }
            }
            _ => return false,
        }
        let Some(next) = vmcb.rip().checked_add(u64::from(access.length)) else {
            return false;
        };
        vmcb.set_rip(next);
        true
    }
}
