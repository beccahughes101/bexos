#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

pub const INTEL_VENDOR_ID: u16 = 0x8086;
pub const E1000E_DEVICE_IDS: [u16; 3] = [0x10d3, 0x1539, 0x15b8];
pub const IGB_DEVICE_IDS: [u16; 2] = [0x10c9, 0x1521];
pub const DEFAULT_RING_SIZE: usize = 256;

pub mod register {
    pub const CTRL: u32 = 0x0000;
    pub const STATUS: u32 = 0x0008;
    pub const ICR: u32 = 0x00c0;
    pub const IMS: u32 = 0x00d0;
    pub const IMC: u32 = 0x00d8;
    pub const RCTL: u32 = 0x0100;
    pub const TCTL: u32 = 0x0400;
    pub const RDBAL: u32 = 0x2800;
    pub const RDBAH: u32 = 0x2804;
    pub const RDLEN: u32 = 0x2808;
    pub const RDH: u32 = 0x2810;
    pub const RDT: u32 = 0x2818;
    pub const TDBAL: u32 = 0x3800;
    pub const TDBAH: u32 = 0x3804;
    pub const TDLEN: u32 = 0x3808;
    pub const TDH: u32 = 0x3810;
    pub const TDT: u32 = 0x3818;
    pub const RAL0: u32 = 0x5400;
    pub const RAH0: u32 = 0x5404;
}

const CTRL_RST: u32 = 1 << 26;
const RCTL_EN: u32 = 1 << 1;
const TCTL_EN: u32 = 1 << 1;
const DESC_STATUS_DD: u8 = 1;
const RX_STATUS_EOP: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceFamily {
    E1000e,
    Igb,
}

impl DeviceFamily {
    pub fn from_pci(vendor_id: u16, device_id: u16) -> Option<Self> {
        if vendor_id != INTEL_VENDOR_ID {
            return None;
        }
        if E1000E_DEVICE_IDS.contains(&device_id) {
            Some(Self::E1000e)
        } else if IGB_DEVICE_IDS.contains(&device_id) {
            Some(Self::Igb)
        } else {
            None
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LegacyDescriptor {
    pub address: u64,
    pub length: u16,
    pub checksum: u16,
    pub command_or_status: u8,
    pub status_or_errors: u8,
    pub special: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaRange {
    pub base: u64,
    pub length: u64,
}

impl DmaRange {
    pub fn contains(self, address: u64, length: u64) -> bool {
        length != 0
            && address >= self.base
            && address
                .checked_add(length)
                .is_some_and(|end| end <= self.base.saturating_add(self.length))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RingError {
    InvalidSize,
    Full,
    Empty,
    DmaBounds,
    MalformedDescriptor,
    NotComplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DescriptorRing {
    descriptors: Vec<LegacyDescriptor>,
    head: usize,
    tail: usize,
    used: usize,
}

impl DescriptorRing {
    pub fn new(size: usize) -> Result<Self, RingError> {
        if !(8..=4096).contains(&size) || !size.is_power_of_two() {
            return Err(RingError::InvalidSize);
        }
        Ok(Self {
            descriptors: alloc::vec![LegacyDescriptor::default(); size],
            head: 0,
            tail: 0,
            used: 0,
        })
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    pub fn head(&self) -> usize {
        self.head
    }

    pub fn tail(&self) -> usize {
        self.tail
    }

    pub fn submit_tx(
        &mut self,
        address: u64,
        length: u16,
        range: DmaRange,
    ) -> Result<usize, RingError> {
        if self.used == self.descriptors.len() {
            return Err(RingError::Full);
        }
        if !range.contains(address, u64::from(length)) {
            return Err(RingError::DmaBounds);
        }
        let index = self.tail;
        self.descriptors[index] = LegacyDescriptor {
            address,
            length,
            command_or_status: (1 << 0) | (1 << 3), // EOP | report status
            ..LegacyDescriptor::default()
        };
        self.tail = (self.tail + 1) & (self.descriptors.len() - 1);
        self.used += 1;
        Ok(index)
    }

    pub fn post_rx(
        &mut self,
        address: u64,
        capacity: u16,
        range: DmaRange,
    ) -> Result<usize, RingError> {
        if self.used == self.descriptors.len() {
            return Err(RingError::Full);
        }
        if !range.contains(address, u64::from(capacity)) {
            return Err(RingError::DmaBounds);
        }
        let index = self.tail;
        self.descriptors[index] = LegacyDescriptor {
            address,
            length: capacity,
            ..LegacyDescriptor::default()
        };
        self.tail = (self.tail + 1) & (self.descriptors.len() - 1);
        self.used += 1;
        Ok(index)
    }

    pub fn descriptor_mut(&mut self, index: usize) -> Option<&mut LegacyDescriptor> {
        self.descriptors.get_mut(index)
    }

    pub fn complete_tx(&mut self) -> Result<LegacyDescriptor, RingError> {
        let descriptor = *self.descriptors.get(self.head).ok_or(RingError::Empty)?;
        if self.used == 0 {
            return Err(RingError::Empty);
        }
        if descriptor.status_or_errors & DESC_STATUS_DD == 0 {
            return Err(RingError::NotComplete);
        }
        self.advance(descriptor)
    }

    pub fn complete_rx(&mut self, range: DmaRange) -> Result<LegacyDescriptor, RingError> {
        let descriptor = *self.descriptors.get(self.head).ok_or(RingError::Empty)?;
        if self.used == 0 {
            return Err(RingError::Empty);
        }
        if descriptor.command_or_status & (DESC_STATUS_DD | RX_STATUS_EOP)
            != DESC_STATUS_DD | RX_STATUS_EOP
            || descriptor.status_or_errors != 0
            || !range.contains(descriptor.address, u64::from(descriptor.length))
        {
            return Err(RingError::MalformedDescriptor);
        }
        self.advance(descriptor)
    }

    fn advance(&mut self, descriptor: LegacyDescriptor) -> Result<LegacyDescriptor, RingError> {
        self.descriptors[self.head] = LegacyDescriptor::default();
        self.head = (self.head + 1) & (self.descriptors.len() - 1);
        self.used -= 1;
        Ok(descriptor)
    }
}

pub trait RegisterBank {
    fn read(&self, offset: u32) -> u32;
    fn write(&mut self, offset: u32, value: u32);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Controller<R> {
    pub registers: R,
    pub family: DeviceFamily,
    pub mac: [u8; 6],
    pub rx: DescriptorRing,
    pub tx: DescriptorRing,
    pub started: bool,
    pub generation: u64,
}

impl<R: RegisterBank> Controller<R> {
    pub fn discover(
        registers: R,
        family: DeviceFamily,
        ring_size: usize,
    ) -> Result<Self, RingError> {
        let low = registers.read(register::RAL0).to_le_bytes();
        let high = registers.read(register::RAH0).to_le_bytes();
        let mac = [low[0], low[1], low[2], low[3], high[0], high[1]];
        if mac == [0; 6] || mac[0] & 1 != 0 {
            return Err(RingError::MalformedDescriptor);
        }
        Ok(Self {
            registers,
            family,
            mac,
            rx: DescriptorRing::new(ring_size)?,
            tx: DescriptorRing::new(ring_size)?,
            started: false,
            generation: 1,
        })
    }

    pub fn reset(&mut self) {
        self.registers.write(register::IMC, u32::MAX);
        self.registers.write(register::RCTL, 0);
        self.registers.write(register::TCTL, 0);
        self.registers.write(register::CTRL, CTRL_RST);
        self.started = false;
        self.generation = self.generation.saturating_add(1);
    }

    pub fn program_rings(&mut self, rx_dma: u64, tx_dma: u64) -> Result<(), RingError> {
        let rx_len = self
            .rx
            .len()
            .checked_mul(core::mem::size_of::<LegacyDescriptor>())
            .ok_or(RingError::InvalidSize)? as u32;
        let tx_len = self
            .tx
            .len()
            .checked_mul(core::mem::size_of::<LegacyDescriptor>())
            .ok_or(RingError::InvalidSize)? as u32;
        for (low, high, length, dma, len) in [
            (
                register::RDBAL,
                register::RDBAH,
                register::RDLEN,
                rx_dma,
                rx_len,
            ),
            (
                register::TDBAL,
                register::TDBAH,
                register::TDLEN,
                tx_dma,
                tx_len,
            ),
        ] {
            if dma & 0xf != 0 {
                return Err(RingError::DmaBounds);
            }
            self.registers.write(low, dma as u32);
            self.registers.write(high, (dma >> 32) as u32);
            self.registers.write(length, len);
        }
        self.registers.write(register::RDH, self.rx.head() as u32);
        self.registers.write(register::RDT, self.rx.tail() as u32);
        self.registers.write(register::TDH, self.tx.head() as u32);
        self.registers.write(register::TDT, self.tx.tail() as u32);
        Ok(())
    }

    pub fn start(&mut self) {
        let _ = self.registers.read(register::ICR);
        // Completion interrupts are sufficient for the FIFO data plane.  In
        // particular, do not enable link-status and management causes here:
        // legacy INTx is level-triggered and those causes can remain asserted
        // while multiple independent functions are coming up.
        self.registers.write(register::IMS, 0x81);
        self.registers.write(register::RCTL, RCTL_EN);
        self.registers.write(register::TCTL, TCTL_EN | (1 << 3));
        self.started = true;
        self.generation = self.generation.saturating_add(1);
    }

    pub fn acknowledge_interrupt(&mut self) -> u32 {
        self.registers.read(register::ICR)
    }

    pub fn quiesce(&mut self) {
        self.registers.write(register::IMC, u32::MAX);
        self.registers.write(register::RCTL, 0);
        self.registers.write(register::TCTL, 0);
        self.started = false;
        self.generation = self.generation.saturating_add(1);
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut writer = Encoder::new();
        writer.word(1);
        writer.word(match self.family {
            DeviceFamily::E1000e => 1,
            DeviceFamily::Igb => 2,
        });
        writer.bytes(&self.mac);
        writer.word(self.started as u64);
        writer.word(self.generation);
        encode_ring(&mut writer, &self.rx);
        encode_ring(&mut writer, &self.tx);
        writer.finish()
    }

    pub fn adopt(registers: R, bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Decoder::new(bytes);
        if reader.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let family = match reader.word()? {
            1 => DeviceFamily::E1000e,
            2 => DeviceFamily::Igb,
            _ => return Err(Error::InvalidData),
        };
        let mac: [u8; 6] = reader
            .bytes(6)?
            .try_into()
            .map_err(|_| Error::InvalidData)?;
        let started = reader.flag()?;
        let generation = reader.word()?;
        let rx = decode_ring(&mut reader)?;
        let tx = decode_ring(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            registers,
            family,
            mac,
            rx,
            tx,
            started,
            generation,
        })
    }
}

fn encode_ring(writer: &mut Encoder, ring: &DescriptorRing) {
    writer.word(ring.descriptors.len() as u64);
    writer.word(ring.head as u64);
    writer.word(ring.tail as u64);
    writer.word(ring.used as u64);
    for descriptor in &ring.descriptors {
        writer.word(descriptor.address);
        writer.word(descriptor.length as u64);
        writer.word(descriptor.checksum as u64);
        writer.word(descriptor.command_or_status as u64);
        writer.word(descriptor.status_or_errors as u64);
        writer.word(descriptor.special as u64);
    }
}

fn decode_ring(reader: &mut Decoder<'_>) -> Result<DescriptorRing, Error> {
    let size = reader.count(4096)?;
    let mut ring = DescriptorRing::new(size).map_err(|_| Error::InvalidData)?;
    ring.head = reader.count(size)?;
    ring.tail = reader.count(size)?;
    ring.used = reader.count(size)?;
    for descriptor in &mut ring.descriptors {
        *descriptor = LegacyDescriptor {
            address: reader.word()?,
            length: u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
            checksum: u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
            command_or_status: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
            status_or_errors: u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
            special: u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
        };
    }
    Ok(ring)
}
