use bexos_d1_pci::{
    BusType, ConfigSpace, PciDevice, PciError, PciRootBus, RootBusConfig, device_node,
};
use bexos_kernel_core::pci::{
    BAR_TYPE_64, COMMAND_BUS_MASTER, COMMAND_MEMORY_SPACE, PCI_PROGIF_NVME, PciAddress,
};

const CONFIG_SIZE: usize = 32 * 8 * 4096;

fn bridge_config() -> MockConfig {
    let mut config = MockConfig::new();
    config.bytes.resize(bexos_d1_pci::ECAM_SIZE as usize, 0xff);
    for device in [4, 5] {
        let address = PciAddress {
            bus: 0,
            device,
            function: 0,
        };
        config.add_device(address, 0x1b36, 0x0c);
        config.set8(address, 0x0e, 1);
        config.set8(address, 0x0b, 6);
        config.set8(address, 0x0a, 4);
        config.set8(address, 0x34, 0x40);
        config.set8(address, 0x40, 0x10);
        config.set8(address, 0x41, 0);
        config.write16(address, 0x42, 0x100).unwrap();
        config.write32(address, 0x54, 0x12).unwrap();
        config.write16(address, 0x58, 0xf00).unwrap();
    }
    config
}

#[test]
fn downstream_allocations_stay_inside_reserved_windows_and_discovery_is_read_only() {
    let config = bridge_config();
    let limits = RootBusConfig::qemu_virt();
    let mut bus = PciRootBus::new(config, limits);
    let mut ports = bus.configure_root_ports().unwrap();
    assert_eq!(ports.len(), 2);
    assert_eq!((ports[0].secondary, ports[1].secondary), (1, 2));
    assert!(ports[0].limit <= ports[1].base);
    let root_cursor = bus.allocation_cursor();
    let mut config = bus.into_config();
    assert_eq!(config.read16(ports[0].address, 0x58).unwrap(), 0x100);
    assert_eq!(config.read32(ports[0].address, 0x18).unwrap(), 0x0001_0100);
    let child = PciAddress {
        bus: 1,
        device: 0,
        function: 0,
    };
    config.add_device(child, 0x1af4, 0x1052);
    config.set_bar0_mask(child, 0xffff_c004, 0xffff_ffff);
    let mut bus = PciRootBus::restore(config, limits, root_cursor).unwrap();
    let devices = bus.discover_bus(1).unwrap();
    assert_eq!(devices.len(), 1);
    let node = bus
        .initialize_downstream(&mut ports[0], devices[0])
        .unwrap();
    assert_eq!(node.node_id, 0x10000);
    assert_eq!(node.bars[0].base, ports[0].base);
    assert_eq!(ports[0].cursor, ports[0].base + 0x4000);
    assert_eq!(ports[1].cursor, ports[1].base);
    assert_eq!(bus.allocation_cursor(), root_cursor);
    let config = bus.into_config();
    let before = config.bytes.clone();
    let bus = PciRootBus::restore(config, limits, root_cursor).unwrap();
    assert_eq!(bus.discover_bus(1).unwrap(), devices);
    assert!(bus.discover_bus(16).is_err());
    assert_eq!(bus.into_config().bytes, before);
}

#[test]
fn oversized_downstream_bar_rolls_back_without_touching_root_allocation() {
    let mut config = bridge_config();
    let child = PciAddress {
        bus: 1,
        device: 0,
        function: 0,
    };
    config.add_device(child, 0x1af4, 0x1052);
    config.set_bar0_mask(child, 0xfe00_0004, 0xffff_ffff);
    let mut bus = PciRootBus::new(config, RootBusConfig::qemu_virt());
    let mut ports = bus.configure_root_ports().unwrap();
    let before = ports.clone();
    let cursor = bus.allocation_cursor();
    let child = bus.discover_bus(1).unwrap()[0];
    assert_eq!(
        bus.initialize_downstream(&mut ports[0], child),
        Err(PciError::BarTooLarge)
    );
    assert_eq!(ports, before);
    assert_eq!(bus.allocation_cursor(), cursor);
    assert_eq!(bus.into_config().read16(child.address, 4).unwrap(), 0);
}

#[derive(Clone, Debug)]
struct MockConfig {
    bytes: Vec<u8>,
    bar_masks: Vec<(PciAddress, u16, u32)>,
}

impl MockConfig {
    fn new() -> Self {
        let mut bytes = vec![0xff; CONFIG_SIZE];
        for device in 0..32u8 {
            for function in 0..8u8 {
                let offset = PciAddress {
                    bus: 0,
                    device,
                    function,
                }
                .ecam_offset(0) as usize;
                bytes[offset] = 0xff;
                bytes[offset + 1] = 0xff;
            }
        }
        Self {
            bytes,
            bar_masks: Vec::new(),
        }
    }

    fn add_device(&mut self, address: PciAddress, vendor_id: u16, device_id: u16) {
        self.write_bytes(address, 0x00, &vendor_id.to_le_bytes());
        self.write_bytes(address, 0x02, &device_id.to_le_bytes());
        self.set8(address, 0x0e, 0);
        self.write_bytes(address, 0x04, &0u16.to_le_bytes());
        self.write_bytes(address, 0x10, &0u32.to_le_bytes());
        self.write_bytes(address, 0x14, &0u32.to_le_bytes());
    }

    fn set8(&mut self, address: PciAddress, register: u16, value: u8) {
        let offset = self.offset(address, register);
        self.bytes[offset] = value;
    }

    fn set_bar0_mask(&mut self, address: PciAddress, low: u32, high: u32) {
        self.set_bar_mask(address, 0x10, low);
        self.set_bar_mask(address, 0x14, high);
    }

    fn set_bar_mask(&mut self, address: PciAddress, register: u16, mask: u32) {
        self.bar_masks.push((address, register, mask));
    }

    fn offset(&self, address: PciAddress, register: u16) -> usize {
        address.ecam_offset(register) as usize
    }

    fn write_bytes(&mut self, address: PciAddress, register: u16, bytes: &[u8]) {
        let offset = self.offset(address, register);
        self.bytes[offset..offset + bytes.len()].copy_from_slice(bytes);
    }
}

impl ConfigSpace for MockConfig {
    fn read8(&self, address: PciAddress, register: u16) -> Result<u8, PciError> {
        self.bytes
            .get(self.offset(address, register))
            .copied()
            .ok_or(PciError::ConfigAccess)
    }

    fn read16(&self, address: PciAddress, register: u16) -> Result<u16, PciError> {
        let offset = self.offset(address, register);
        Ok(u16::from_le_bytes([
            *self.bytes.get(offset).ok_or(PciError::ConfigAccess)?,
            *self.bytes.get(offset + 1).ok_or(PciError::ConfigAccess)?,
        ]))
    }

    fn read32(&self, address: PciAddress, register: u16) -> Result<u32, PciError> {
        let offset = self.offset(address, register);
        Ok(u32::from_le_bytes([
            *self.bytes.get(offset).ok_or(PciError::ConfigAccess)?,
            *self.bytes.get(offset + 1).ok_or(PciError::ConfigAccess)?,
            *self.bytes.get(offset + 2).ok_or(PciError::ConfigAccess)?,
            *self.bytes.get(offset + 3).ok_or(PciError::ConfigAccess)?,
        ]))
    }

    fn write16(&mut self, address: PciAddress, register: u16, value: u16) -> Result<(), PciError> {
        self.write_bytes(address, register, &value.to_le_bytes());
        Ok(())
    }

    fn write32(&mut self, address: PciAddress, register: u16, value: u32) -> Result<(), PciError> {
        if value == 0xffff_ffff {
            let mask = if let Some((_, _, mask)) = self
                .bar_masks
                .iter()
                .find(|(bar_address, bar_register, _)| {
                    *bar_address == address && *bar_register == register
                })
                .copied()
            {
                mask
            } else if (0x10..=0x24).contains(&register) {
                0
            } else {
                value
            };
            self.write_bytes(address, register, &mask.to_le_bytes());
            return Ok(());
        }
        self.write_bytes(address, register, &value.to_le_bytes());
        Ok(())
    }
}

#[test]
fn mock_config_uses_ecam_offsets() {
    let mut config = MockConfig::new();
    let address = PciAddress {
        bus: 0,
        device: 3,
        function: 4,
    };
    config.add_device(address, 0x1234, 0xabcd);

    assert_eq!(config.read16(address, 0).unwrap(), 0x1234);
    assert_eq!(address.ecam_offset(0x10), 0x1c010);
}

#[test]
fn bdf_zero_is_discoverable_but_not_registered_as_a_device_node() {
    let mut config = MockConfig::new();
    let root = PciAddress {
        bus: 0,
        device: 0,
        function: 0,
    };
    config.add_device(root, 0x1b36, 0x0008);
    config.set8(root, 0x0b, 0x06);
    let endpoint = PciAddress {
        bus: 0,
        device: 1,
        function: 0,
    };
    config.add_device(endpoint, 0x1af4, 0x1041);

    let mut bus = PciRootBus::new(config, RootBusConfig::qemu_virt());
    assert_eq!(bus.discover_bus0().unwrap().len(), 2);
    let nodes = bus.enumerate_bus0().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_ne!(nodes[0].node_id, 0);
}

#[test]
fn scans_multifunction_devices_and_skips_invalid_vendor_ids() {
    let mut config = MockConfig::new();
    let fn0 = PciAddress {
        bus: 0,
        device: 2,
        function: 0,
    };
    let fn3 = PciAddress { function: 3, ..fn0 };
    config.add_device(fn0, 0x1111, 0x0001);
    config.set8(fn0, 0x0e, 0x80);
    config.add_device(fn3, 0x2222, 0x0002);

    let mut bus = PciRootBus::new(config, RootBusConfig::qemu_virt());
    let nodes = bus.enumerate_bus0().unwrap();

    assert_eq!(nodes.len(), 2);
    assert!(nodes.iter().any(|node| node.node_id == 0x0200));
    assert!(nodes.iter().any(|node| node.node_id == 0x0203));
}

#[test]
fn emits_nvme_class_device_node_properties() {
    let device = PciDevice {
        address: PciAddress {
            bus: 0,
            device: 1,
            function: 0,
        },
        vendor_id: 0x1b36,
        device_id: 0x0010,
        class: 0x01,
        subclass: 0x08,
        prog_if: PCI_PROGIF_NVME,
    };

    let node = device_node(device, Vec::new());

    assert_eq!(node.bus, BusType::Pci);
    assert!(
        node.properties
            .iter()
            .any(|property| property.key == "pci.class" && property.value == 0x01)
    );
    assert!(
        node.properties
            .iter()
            .any(|property| property.key == "pci.prog_if" && property.value == 0x02)
    );
}

#[test]
fn sizes_assigns_bar0_and_leaves_bus_mastering_disabled() {
    let mut config = MockConfig::new();
    let address = PciAddress {
        bus: 0,
        device: 4,
        function: 0,
    };
    config.add_device(address, 0x1b36, 0x0010);
    config.set_bar0_mask(address, 0xffff_c000 | BAR_TYPE_64, 0xffff_ffff);

    let mut bus = PciRootBus::new(
        config,
        RootBusConfig {
            mmio_base: 0x1000_1000,
            mmio_limit: 0x1001_0000,
        },
    );
    let nodes = bus.enumerate_bus0().unwrap();

    assert_eq!(nodes[0].bars[0].base, 0x1000_4000);
    assert_eq!(nodes[0].bars[0].size, 0x4000);
    let command = bus_test_command_register(bus, address);
    assert_ne!(command & COMMAND_MEMORY_SPACE, 0);
    assert_eq!(command & COMMAND_BUS_MASTER, 0);
}

#[test]
fn assigns_later_memory_bar_when_bar0_is_io_space() {
    let mut config = MockConfig::new();
    let address = PciAddress {
        bus: 0,
        device: 5,
        function: 0,
    };
    config.add_device(address, 0x1af4, 0x1041);
    config.write_bytes(address, 0x10, &1u32.to_le_bytes());
    config.set_bar_mask(address, 0x14, 0xffff_f000);

    let mut bus = PciRootBus::new(
        config,
        RootBusConfig {
            mmio_base: 0x1000_1000,
            mmio_limit: 0x1001_0000,
        },
    );
    let nodes = bus.enumerate_bus0().unwrap();

    assert_eq!(nodes[0].bars.len(), 1);
    assert_eq!(nodes[0].bars[0].index, 1);
    assert_eq!(nodes[0].bars[0].base, 0x1000_1000);
    assert_eq!(nodes[0].bars[0].size, 0x1000);
}

fn bus_test_command_register(bus: PciRootBus<MockConfig>, address: PciAddress) -> u16 {
    bus.into_config().read16(address, 0x04).unwrap()
}

#[test]
fn presence_rescan_does_not_reassign_live_bars_and_cursor_restores() {
    let address = PciAddress {
        bus: 0,
        device: 4,
        function: 0,
    };
    let mut config = MockConfig::new();
    config.add_device(address, 0x1af4, 0x1052);
    config.set_bar0_mask(address, 0xffff_c000 | BAR_TYPE_64, 0xffff_ffff);
    let limits = RootBusConfig {
        mmio_base: 0x1000_1000,
        mmio_limit: 0x1001_0000,
    };
    let mut bus = PciRootBus::new(config, limits);
    let assigned = bus.enumerate_bus0().unwrap();
    let cursor = bus.allocation_cursor();
    assert_eq!(bus.discover_bus0().unwrap().len(), 1);
    assert_eq!(bus.allocation_cursor(), cursor);
    let config = bus.into_config();
    assert_eq!(
        config.read32(address, 0x10).unwrap() & !15,
        assigned[0].bars[0].base as u32
    );
    let restored = PciRootBus::restore(config, limits, cursor).unwrap();
    assert_eq!(restored.discover_bus0().unwrap().len(), 1);
    assert_eq!(restored.allocation_cursor(), cursor);
}
#[test]
fn failed_new_endpoint_assignment_restores_all_bars_and_command() {
    let address = PciAddress {
        bus: 0,
        device: 4,
        function: 0,
    };
    let mut config = MockConfig::new();
    config.add_device(address, 0x1af4, 0x1052);
    config.set_bar_mask(address, 0x10, 0xffff_f000);
    config.set_bar_mask(address, 0x14, 0xffff_0000);
    let mut bus = PciRootBus::new(
        config,
        RootBusConfig {
            mmio_base: 0x1000_0000,
            mmio_limit: 0x1000_2000,
        },
    );
    assert!(bus.enumerate_bus0().is_err());
    assert_eq!(bus.allocation_cursor(), 0x1000_0000);
    let config = bus.into_config();
    assert_eq!(config.read32(address, 0x10).unwrap(), 0);
    assert_eq!(config.read32(address, 0x14).unwrap(), 0);
    assert_eq!(config.read16(address, 0x04).unwrap(), 0);
}
