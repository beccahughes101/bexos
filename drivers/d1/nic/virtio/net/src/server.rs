use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use bexos_migration::Error;
use bexos_migration::codec::{Decoder, Encoder};

use ethernet_fidl::{EthernetInfo, FrameEntry, FrameFlags, FrameOpcode, MacAddress, Status};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Buffer {
    pub handle: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub token: u64,
    pub size: u32,
}

#[derive(Debug)]
pub struct EthernetServer {
    info: EthernetInfo,
    next_vmo_id: u32,
    buffers: BTreeMap<u32, Buffer>,
    running: bool,
}

impl EthernetServer {
    pub fn new(mac: [u8; 6]) -> Self {
        Self {
            info: EthernetInfo {
                mac: MacAddress { octets: mac },
                mtu: 1500,
                features: ethernet_fidl::DeviceFeatures::DMA_64BIT,
            },
            next_vmo_id: 1,
            buffers: BTreeMap::new(),
            running: false,
        }
    }

    pub const fn info(&self) -> EthernetInfo {
        self.info
    }

    pub fn register_buffer(&mut self, buffer: Buffer) -> Result<u32, Status> {
        if buffer.size == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let id = self.next_vmo_id;
        self.next_vmo_id = self.next_vmo_id.checked_add(1).ok_or(Status::ErrNoMemory)?;
        self.buffers.insert(id, buffer);
        Ok(id)
    }

    pub fn unregister_buffer(&mut self, vmo_id: u32) -> Result<Buffer, Status> {
        self.buffers.remove(&vmo_id).ok_or(Status::ErrInvalidArgs)
    }

    pub fn start(&mut self) {
        self.running = true;
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub const fn running(&self) -> bool {
        self.running
    }

    pub fn buffers(&self) -> &BTreeMap<u32, Buffer> {
        &self.buffers
    }

    pub fn next_vmo_id(&self) -> u32 {
        self.next_vmo_id
    }

    pub fn restore_registrations(&mut self, next_vmo_id: u32, buffers: BTreeMap<u32, Buffer>) {
        self.next_vmo_id = next_vmo_id;
        self.buffers = buffers;
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.bytes(&self.info.mac.octets);
        w.word(self.info.mtu as u64);
        w.word(self.info.features.0 as u64);
        w.word(self.next_vmo_id as u64);
        w.word(self.running as u64);
        w.word(self.buffers.len() as u64);
        for (id, buffer) in &self.buffers {
            w.word(*id as u64);
            for value in [
                buffer.handle,
                buffer.vaddr,
                buffer.paddr,
                buffer.token,
                buffer.size as u64,
            ] {
                w.word(value);
            }
        }
        w.finish()
    }

    pub fn adopt(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mac = r.bytes(6)?;
        if mac.len() != 6 {
            return Err(Error::InvalidData);
        }
        let mtu = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let features = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let next_vmo_id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let running = r.flag()?;
        let mut buffers = BTreeMap::new();
        for _ in 0..r.count(1024)? {
            let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let buffer = Buffer {
                handle: r.word()?,
                vaddr: r.word()?,
                paddr: r.word()?,
                token: r.word()?,
                size: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            };
            if id == 0 || buffer.size == 0 || buffers.insert(id, buffer).is_some() {
                return Err(Error::InvalidData);
            }
        }
        r.finish()?;
        Ok(Self {
            info: EthernetInfo {
                mac: MacAddress {
                    octets: [mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]],
                },
                mtu,
                features: ethernet_fidl::DeviceFeatures(features),
            },
            next_vmo_id,
            buffers,
            running,
        })
    }

    pub fn registrations_match(&self) -> bool {
        self.buffers.iter().all(|(id, buffer)| {
            *id != 0
                && *id < self.next_vmo_id
                && buffer.handle != 0
                && buffer.vaddr != 0
                && buffer.paddr != 0
                && buffer.token != 0
                && buffer.size != 0
        })
    }

    pub fn frame_slice(&mut self, entry: FrameEntry) -> Result<&mut [u8], Status> {
        if !self.running {
            return Err(Status::ErrShouldWait);
        }
        let buffer = self
            .buffers
            .get(&entry.vmo_id)
            .ok_or(Status::ErrInvalidArgs)?;
        let start = entry.offset as u64;
        let end = start
            .checked_add(entry.length as u64)
            .ok_or(Status::ErrInvalidArgs)?;
        if end > buffer.size as u64 {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(unsafe {
            core::slice::from_raw_parts_mut(
                (buffer.vaddr + start) as *mut u8,
                entry.length as usize,
            )
        })
    }
}

pub fn tx_complete(entry: FrameEntry, status: Status) -> FrameEntry {
    FrameEntry {
        opcode: FrameOpcode::TxComplete,
        flags: if status == Status::Ok {
            FrameFlags(0)
        } else {
            FrameFlags::TRUNCATED
        },
        ..entry
    }
}

pub fn rx_complete(entry: FrameEntry, length: usize) -> FrameEntry {
    FrameEntry {
        opcode: FrameOpcode::RxComplete,
        length: length.min(u16::MAX as usize) as u16,
        flags: if length > u16::MAX as usize {
            FrameFlags::TRUNCATED
        } else {
            FrameFlags(0)
        },
        ..entry
    }
}

pub fn rx_error(entry: FrameEntry) -> FrameEntry {
    FrameEntry {
        opcode: FrameOpcode::RxComplete,
        flags: FrameFlags::TRUNCATED,
        ..entry
    }
}
