pub const PCI_VENDOR_INVALID: u16 = 0xffff;
pub const PCI_CLASS_MASS_STORAGE: u8 = 0x01;
pub const PCI_SUBCLASS_NVME: u8 = 0x08;
pub const PCI_PROGIF_NVME: u8 = 0x02;

pub const COMMAND_IO_SPACE: u16 = 1 << 0;
pub const COMMAND_MEMORY_SPACE: u16 = 1 << 1;
pub const COMMAND_BUS_MASTER: u16 = 1 << 2;

pub const BAR_IO_SPACE: u32 = 1 << 0;
pub const BAR_TYPE_MASK: u32 = 0b11 << 1;
pub const BAR_TYPE_64: u32 = 0b10 << 1;
pub const BAR_PREFETCHABLE: u32 = 1 << 3;
pub const BAR_MEM_MASK: u32 = 0xffff_fff0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciAddress {
    pub const fn ecam_offset(self, register: u16) -> u64 {
        ((self.bus as u64) << 20)
            | ((self.device as u64) << 15)
            | ((self.function as u64) << 12)
            | (register as u64 & 0xfff)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciDeviceId {
    pub vendor_id: u16,
    pub device_id: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciClass {
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

impl PciClass {
    pub const fn is_nvme(self) -> bool {
        self.class == PCI_CLASS_MASS_STORAGE
            && self.subclass == PCI_SUBCLASS_NVME
            && self.prog_if == PCI_PROGIF_NVME
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciBar {
    pub index: u8,
    pub base: u64,
    pub size: u64,
    pub is_64_bit: bool,
    pub prefetchable: bool,
}

pub const fn decode_bar_size(low_mask: u32, high_mask: u32, is_64_bit: bool) -> u64 {
    if is_64_bit {
        let mask = ((high_mask as u64) << 32) | ((low_mask & BAR_MEM_MASK) as u64);
        if mask == 0 {
            0
        } else {
            (!mask).wrapping_add(1)
        }
    } else {
        let mask = low_mask & BAR_MEM_MASK;
        if mask == 0 {
            0
        } else {
            (!mask).wrapping_add(1) as u64
        }
    }
}

pub const fn align_resource_base(base: u64, size: u64) -> u64 {
    if size == 0 {
        base
    } else {
        (base + size - 1) & !(size - 1)
    }
}

pub const fn command_with_memory_and_bus_master(command: u16) -> u16 {
    (command | COMMAND_MEMORY_SPACE | COMMAND_BUS_MASTER) & !COMMAND_IO_SPACE
}
