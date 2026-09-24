#![no_std]
extern crate alloc;
use alloc::vec::Vec;
use alloc::{format, string::String, vec};
mod bridge;
pub use bridge::{ECAM_SIZE, MAX_BUSES, RootPort};

use bexos_kernel_core::pci::{
    BAR_IO_SPACE, BAR_MEM_MASK, BAR_PREFETCHABLE, BAR_TYPE_64, BAR_TYPE_MASK, PCI_VENDOR_INVALID,
    PciAddress, align_resource_base, command_with_memory_and_bus_master, decode_bar_size,
};

pub const QEMU_VIRT_ECAM_BASE: u64 = 0x3f00_0000;
pub const QEMU_VIRT_PCI_MMIO_BASE: u64 = if cfg!(bexos_arch_x86_64) {
    0xc000_0000
} else {
    0x1000_0000
};
pub const QEMU_VIRT_PCI_MMIO_LIMIT: u64 = if cfg!(bexos_arch_x86_64) {
    0xfebf_0000
} else {
    0x3eff_0000
};

const REG_VENDOR_ID: u16 = 0x00;
const REG_COMMAND: u16 = 0x04;
const REG_PROG_IF: u16 = 0x09;
const REG_SUBCLASS: u16 = 0x0a;
const REG_CLASS: u16 = 0x0b;
const REG_HEADER_TYPE: u16 = 0x0e;
const REG_BAR0: u16 = 0x10;
const TYPE0_BAR_COUNT: u8 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusType {
    Pci,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceProperty {
    pub key: &'static str,
    pub value: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceNodeInfo {
    pub node_id: u64,
    pub bus: BusType,
    pub topological_path: String,
    pub properties: Vec<DeviceProperty>,
    pub bars: Vec<BarAssignment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciDevice {
    pub address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarAssignment {
    pub index: u8,
    pub base: u64,
    pub size: u64,
    pub is_64_bit: bool,
    pub prefetchable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PciError {
    ConfigAccess,
    IoBarUnsupported,
    EmptyBar,
    BarTooLarge,
}

pub trait ConfigSpace {
    fn read8(&self, address: PciAddress, register: u16) -> Result<u8, PciError>;
    fn read16(&self, address: PciAddress, register: u16) -> Result<u16, PciError>;
    fn read32(&self, address: PciAddress, register: u16) -> Result<u32, PciError>;
    fn write16(&mut self, address: PciAddress, register: u16, value: u16) -> Result<(), PciError>;
    fn write32(&mut self, address: PciAddress, register: u16, value: u32) -> Result<(), PciError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EcamConfigSpace {
    base: usize,
}

impl EcamConfigSpace {
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    fn ptr<T>(&self, address: PciAddress, register: u16) -> *mut T {
        (self.base + address.ecam_offset(register) as usize) as *mut T
    }
}

impl ConfigSpace for EcamConfigSpace {
    fn read8(&self, address: PciAddress, register: u16) -> Result<u8, PciError> {
        Ok(unsafe { core::ptr::read_volatile(self.ptr::<u8>(address, register).cast_const()) })
    }

    fn read16(&self, address: PciAddress, register: u16) -> Result<u16, PciError> {
        Ok(unsafe { core::ptr::read_volatile(self.ptr::<u16>(address, register).cast_const()) })
    }

    fn read32(&self, address: PciAddress, register: u16) -> Result<u32, PciError> {
        Ok(unsafe { core::ptr::read_volatile(self.ptr::<u32>(address, register).cast_const()) })
    }

    fn write16(&mut self, address: PciAddress, register: u16, value: u16) -> Result<(), PciError> {
        unsafe {
            core::ptr::write_volatile(self.ptr::<u16>(address, register), value);
        }
        Ok(())
    }

    fn write32(&mut self, address: PciAddress, register: u16, value: u32) -> Result<(), PciError> {
        unsafe {
            core::ptr::write_volatile(self.ptr::<u32>(address, register), value);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootBusConfig {
    pub mmio_base: u64,
    pub mmio_limit: u64,
}

impl RootBusConfig {
    pub const fn qemu_virt() -> Self {
        Self {
            mmio_base: QEMU_VIRT_PCI_MMIO_BASE,
            mmio_limit: QEMU_VIRT_PCI_MMIO_LIMIT,
        }
    }

    pub const fn new(mmio_base: u64, mmio_limit: u64) -> Result<Self, PciError> {
        if mmio_base == 0 || mmio_base >= mmio_limit || mmio_base & 0xfff != 0 {
            return Err(PciError::ConfigAccess);
        }
        Ok(Self {
            mmio_base,
            mmio_limit,
        })
    }
}

pub struct PciRootBus<C> {
    config: C,
    bus: RootBusConfig,
    next_mmio_base: u64,
}

impl<C: ConfigSpace> PciRootBus<C> {
    pub fn new(config: C, bus: RootBusConfig) -> Self {
        Self {
            config,
            next_mmio_base: bus.mmio_base,
            bus,
        }
    }

    pub fn allocation_cursor(&self) -> u64 {
        self.next_mmio_base
    }
    /// Restore only logical allocator state; no BARs or device queues are reset.
    pub fn restore(config: C, bus: RootBusConfig, cursor: u64) -> Result<Self, PciError> {
        if cursor < bus.mmio_base || cursor > bus.mmio_limit {
            return Err(PciError::BarTooLarge);
        }
        Ok(Self {
            config,
            bus,
            next_mmio_base: cursor,
        })
    }
    pub fn enumerate_bus0(&mut self) -> Result<Vec<DeviceNodeInfo>, PciError> {
        self.discover_bus0()?
            .into_iter()
            // DeviceRegistry reserves zero as the absent-parent sentinel.  The
            // root-complex function conventionally occupies 0000:00:00.0 and
            // remains discoverable for bridge configuration, but is not a
            // bindable child device.
            .filter(|device| node_id(device.address) != 0)
            .map(|device| self.initialize_device(device))
            .collect()
    }
    /// Read-only presence inventory. Hotplug polling must never size or relocate
    /// an already-running device's BARs.
    pub fn discover_bus0(&self) -> Result<Vec<PciDevice>, PciError> {
        self.discover_bus(0)
    }
    pub fn discover_bus(&self, bus: u8) -> Result<Vec<PciDevice>, PciError> {
        if bus >= MAX_BUSES {
            return Err(PciError::ConfigAccess);
        }
        let mut devices = Vec::new();
        for device in 0..32u8 {
            let base_address = PciAddress {
                bus,
                device,
                function: 0,
            };
            let header = self.config.read8(base_address, REG_HEADER_TYPE)?;
            let function_count = if header & 0x80 != 0 { 8 } else { 1 };
            for function in 0..function_count {
                let address = PciAddress {
                    bus,
                    device,
                    function,
                };
                let vendor_id = self.config.read16(address, REG_VENDOR_ID)?;
                if vendor_id == PCI_VENDOR_INVALID {
                    continue;
                }
                let pci_device = PciDevice {
                    address,
                    vendor_id,
                    device_id: self.config.read16(address, REG_VENDOR_ID + 2)?,
                    class: self.config.read8(address, REG_CLASS)?,
                    subclass: self.config.read8(address, REG_SUBCLASS)?,
                    prog_if: self.config.read8(address, REG_PROG_IF)?,
                };
                devices.push(pci_device);
            }
        }
        Ok(devices)
    }

    /// Assign a newly discovered endpoint transactionally. Callers must exclude
    /// addresses already registered with appd before invoking this operation.
    pub fn initialize_device(&mut self, device: PciDevice) -> Result<DeviceNodeInfo, PciError> {
        let address = device.address;
        if self.config.read16(address, REG_VENDOR_ID)? != device.vendor_id
            || self.config.read16(address, REG_VENDOR_ID + 2)? != device.device_id
        {
            return Err(PciError::ConfigAccess);
        }
        if self.config.read8(address, REG_HEADER_TYPE)? & 0x7f != 0 {
            // Bridge registers at 0x18 and later are not endpoint BARs.
            return Ok(device_node(device, Vec::new()));
        }
        let mut original = [0; TYPE0_BAR_COUNT as usize];
        for (i, bar) in original.iter_mut().enumerate() {
            *bar = self.config.read32(address, REG_BAR0 + i as u16 * 4)?;
        }
        let command = self.config.read16(address, REG_COMMAND)?;
        self.config.write16(address, REG_COMMAND, command & !6)?;
        let cursor = self.next_mmio_base;
        match self
            .assign_memory_bars(device)
            .and_then(|bars| {
                // Decoding the assigned BARs is safe before a driver binds, but
                // DMA remains disabled until appd commits the binding through
                // PciDeviceControl.
                let command = command_with_memory_and_bus_master(command) & !(1 << 2);
                self.config.write16(address, REG_COMMAND, command)?;
                Ok(device_node(device, bars))
            })
        {
            Ok(node) => Ok(node),
            Err(error) => {
                self.next_mmio_base = cursor;
                for (i, bar) in original.into_iter().enumerate() {
                    self.config.write32(address, REG_BAR0 + i as u16 * 4, bar)?;
                }
                self.config.write16(address, REG_COMMAND, command)?;
                Err(error)
            }
        }
    }

    pub fn into_config(self) -> C {
        self.config
    }

    fn assign_memory_bars(&mut self, device: PciDevice) -> Result<Vec<BarAssignment>, PciError> {
        let mut bars = Vec::new();
        let mut index = 0;
        while index < TYPE0_BAR_COUNT {
            if let Some(bar) = self.assign_bar(device, index)? {
                index += if bar.is_64_bit { 2 } else { 1 };
                bars.push(bar);
            } else {
                index += 1;
            }
        }
        Ok(bars)
    }

    fn assign_bar(
        &mut self,
        device: PciDevice,
        index: u8,
    ) -> Result<Option<BarAssignment>, PciError> {
        let register = REG_BAR0 + u16::from(index) * 4;
        let original_low = self.config.read32(device.address, register)?;
        if original_low & BAR_IO_SPACE != 0 {
            return Ok(None);
        }

        self.config.write32(device.address, register, 0xffff_ffff)?;
        let low_mask = self.config.read32(device.address, register)?;
        let is_64_bit = low_mask & BAR_TYPE_MASK == BAR_TYPE_64;
        if is_64_bit && index + 1 >= TYPE0_BAR_COUNT {
            self.config
                .write32(device.address, register, original_low)?;
            return Err(PciError::BarTooLarge);
        }
        let mut high_mask = 0;
        let mut original_high = 0;
        if is_64_bit {
            original_high = self.config.read32(device.address, register + 4)?;
            self.config
                .write32(device.address, register + 4, 0xffff_ffff)?;
            high_mask = self.config.read32(device.address, register + 4)?;
        }

        let size = decode_bar_size(low_mask, high_mask, is_64_bit);
        if size == 0 {
            self.restore_bar(device, register, original_low, original_high, is_64_bit)?;
            return Ok(None);
        }
        let base = align_resource_base(self.next_mmio_base, size);
        if base.checked_add(size).unwrap_or(u64::MAX) > self.bus.mmio_limit {
            self.restore_bar(device, register, original_low, original_high, is_64_bit)?;
            return Err(PciError::BarTooLarge);
        }

        self.config
            .write32(device.address, register, (base as u32) & BAR_MEM_MASK)?;
        if is_64_bit {
            self.config
                .write32(device.address, register + 4, (base >> 32) as u32)?;
        }
        self.next_mmio_base = base + size;

        Ok(Some(BarAssignment {
            index,
            base,
            size,
            is_64_bit,
            prefetchable: original_low & BAR_PREFETCHABLE != 0,
        }))
    }

    fn restore_bar(
        &mut self,
        device: PciDevice,
        register: u16,
        original_low: u32,
        original_high: u32,
        is_64_bit: bool,
    ) -> Result<(), PciError> {
        self.config
            .write32(device.address, register, original_low)?;
        if is_64_bit {
            self.config
                .write32(device.address, register + 4, original_high)?;
        }
        Ok(())
    }

    /// Bus mastering is committed only after appd has retained every recovery
    /// endpoint and marked the node active.
    pub fn set_bus_master(&mut self, address: PciAddress, enabled: bool) -> Result<(), PciError> {
        let command = self.config.read16(address, REG_COMMAND)?;
        let next = if enabled {
            command_with_memory_and_bus_master(command)
        } else {
            command & !(1 << 2)
        };
        self.config.write16(address, REG_COMMAND, next)
    }

    pub fn reset_device(&mut self, address: PciAddress) -> Result<(), PciError> {
        let command = self.config.read16(address, REG_COMMAND)?;
        self.config.write16(address, REG_COMMAND, command & !0x7)
    }

    pub fn resume_device(&mut self, address: PciAddress) -> Result<(), PciError> {
        let command = self.config.read16(address, REG_COMMAND)?;
        self.config
            .write16(address, REG_COMMAND, command | (1 << 1))
    }
}

pub fn device_node(device: PciDevice, bars: Vec<BarAssignment>) -> DeviceNodeInfo {
    DeviceNodeInfo {
        node_id: node_id(device.address),
        bus: BusType::Pci,
        topological_path: format!(
            "pci/0000:{:02x}:{:02x}.{}",
            device.address.bus, device.address.device, device.address.function
        ),
        properties: vec![
            DeviceProperty {
                key: "pci.segment",
                value: 0,
            },
            DeviceProperty {
                key: "pci.bus",
                value: device.address.bus as u32,
            },
            DeviceProperty {
                key: "pci.device",
                value: device.address.device as u32,
            },
            DeviceProperty {
                key: "pci.function",
                value: device.address.function as u32,
            },
            DeviceProperty {
                key: "pci.vendor_id",
                value: device.vendor_id as u32,
            },
            DeviceProperty {
                key: "pci.device_id",
                value: device.device_id as u32,
            },
            DeviceProperty {
                key: "pci.class",
                value: device.class as u32,
            },
            DeviceProperty {
                key: "pci.subclass",
                value: device.subclass as u32,
            },
            DeviceProperty {
                key: "pci.prog_if",
                value: device.prog_if as u32,
            },
        ],
        bars,
    }
}

pub const fn node_id(address: PciAddress) -> u64 {
    ((address.bus as u64) << 16) | ((address.device as u64) << 8) | address.function as u64
}

pub const fn address_from_node(node: u64) -> Option<PciAddress> {
    if node > 0xff1f07 || node & 0xff > 7 || (node >> 8) & 0xff > 31 {
        return None;
    }
    Some(PciAddress {
        bus: (node >> 16) as u8,
        device: (node >> 8) as u8,
        function: node as u8,
    })
}
