use bexos_intel_nic::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Registers(BTreeMap<u32, u32>);
impl RegisterBank for Registers {
    fn read(&self, offset: u32) -> u32 {
        self.0.get(&offset).copied().unwrap_or(0)
    }
    fn write(&mut self, offset: u32, value: u32) {
        self.0.insert(offset, value);
    }
}

fn registers() -> Registers {
    let mut registers = Registers::default();
    registers.0.insert(register::RAL0, 0x3322_1102);
    registers.0.insert(register::RAH0, 0x0000_5544);
    registers
}

#[test]
fn supported_ids_select_the_right_model() {
    for id in E1000E_DEVICE_IDS {
        assert_eq!(
            DeviceFamily::from_pci(INTEL_VENDOR_ID, id),
            Some(DeviceFamily::E1000e)
        );
    }
    for id in IGB_DEVICE_IDS {
        assert_eq!(
            DeviceFamily::from_pci(INTEL_VENDOR_ID, id),
            Some(DeviceFamily::Igb)
        );
    }
    assert_eq!(DeviceFamily::from_pci(INTEL_VENDOR_ID, 0xffff), None);
}

#[test]
fn ring_wraparound_and_dma_bounds_are_enforced() {
    let mut ring = DescriptorRing::new(8).unwrap();
    let range = DmaRange {
        base: 0x1000,
        length: 0x1000,
    };
    assert_eq!(ring.submit_tx(0xfff, 1, range), Err(RingError::DmaBounds));
    for index in 0..8 {
        let slot = ring.submit_tx(0x1000 + index * 16, 16, range).unwrap();
        ring.descriptor_mut(slot).unwrap().status_or_errors = 1;
        ring.complete_tx().unwrap();
    }
    assert_eq!(ring.head(), 0);
    assert_eq!(ring.tail(), 0);
}

#[test]
fn malformed_rx_and_reset_are_safe() {
    let range = DmaRange {
        base: 0x1000,
        length: 0x1000,
    };
    let mut ring = DescriptorRing::new(8).unwrap();
    let slot = ring.post_rx(0x1000, 64, range).unwrap();
    ring.descriptor_mut(slot).unwrap().command_or_status = 1;
    assert_eq!(ring.complete_rx(range), Err(RingError::MalformedDescriptor));

    let mut controller = Controller::discover(registers(), DeviceFamily::Igb, 8).unwrap();
    controller.start();
    controller.reset();
    assert!(!controller.started);
    assert_eq!(controller.registers.0[&register::IMC], u32::MAX);
}

#[test]
fn migration_preserves_descriptor_state() {
    let mut controller = Controller::discover(registers(), DeviceFamily::E1000e, 8).unwrap();
    controller
        .tx
        .submit_tx(
            0x2000,
            64,
            DmaRange {
                base: 0x2000,
                length: 4096,
            },
        )
        .unwrap();
    controller.start();
    let checkpoint = controller.checkpoint();
    let adopted = Controller::adopt(registers(), &checkpoint).unwrap();
    assert_eq!(adopted.family, DeviceFamily::E1000e);
    assert_eq!(adopted.mac, controller.mac);
    assert_eq!(adopted.tx, controller.tx);
    assert!(adopted.started);
}
