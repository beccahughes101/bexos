pub const PCI_CLASS_SERIAL_BUS: u32 = 0x0c;
pub const PCI_SUBCLASS_USB: u32 = 0x03;
pub const PCI_PROG_IF_XHCI: u32 = 0x30;
pub const CAPLENGTH: usize = 0x00;
pub const HCSPARAMS1: usize = 0x04;
pub const HCSPARAMS2: usize = 0x08;
pub const HCCPARAMS1: usize = 0x10;
pub const DBOFF: usize = 0x14;
pub const RTSOFF: usize = 0x18;
pub const USBCMD: usize = 0x00;
pub const USBSTS: usize = 0x04;
pub const CRCR: usize = 0x18;
pub const DCBAAP: usize = 0x30;
pub const CONFIG: usize = 0x38;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilityRegisters {
    pub caplength: u8,
    pub max_slots: u8,
    pub max_interrupters: u16,
    pub max_ports: u8,
    pub scratchpads: u16,
    pub doorbell_offset: u32,
    pub runtime_offset: u32,
}

impl CapabilityRegisters {
    pub fn parse(registers: &[u8]) -> Option<Self> {
        let caplength = *registers.get(CAPLENGTH)?;
        if caplength < 0x20 {
            return None;
        }
        let hcs1 = le32(registers, HCSPARAMS1)?;
        let hcs2 = le32(registers, HCSPARAMS2)?;
        Some(Self {
            caplength,
            max_slots: (hcs1 & 0xff) as u8,
            max_interrupters: ((hcs1 >> 8) & 0x7ff) as u16,
            max_ports: ((hcs1 >> 24) & 0xff) as u8,
            scratchpads: (((hcs2 >> 21) & 0x1f) | (((hcs2 >> 27) & 0x1f) << 5)) as u16,
            doorbell_offset: le32(registers, DBOFF)? & !0x3,
            runtime_offset: le32(registers, RTSOFF)? & !0x1f,
        })
    }
}

fn le32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
