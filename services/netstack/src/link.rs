use alloc::collections::VecDeque;
use alloc::vec::Vec;

use bexos_userspace::{Channel, Memory};
use ethernet_fidl::{FidlDecode, FidlEncode, FrameEntry, FrameFlags, FrameOpcode};
use net_fidl::Status;
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
mod snapshot;
pub use snapshot::BACKLOG_CHUNKS;

use crate::ethernet::{EthernetLink, RX_BYTES, TX_BYTES};

pub const SLOT_SIZE: usize = 2048;
pub const SLOT_COUNT: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkResources {
    pub control: u64,
    pub fifo: u64,
    pub rx_vmo: u64,
    pub tx_vmo: u64,
    pub rx_vaddr: u64,
    pub tx_vaddr: u64,
    pub rx_vmo_id: u32,
    pub tx_vmo_id: u32,
    pub mtu: u32,
    pub mac: [u8; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketFrame {
    pub req_id: u64,
    pub len: usize,
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkModel {
    next_req_id: u64,
    next_tx_slot: usize,
    next_rx_slot: usize,
    supplied_rx: Vec<FrameEntry>,
    completed_rx: VecDeque<FrameEntry>,
    in_flight_tx: Vec<FrameEntry>,
    completed_tx: Vec<FrameEntry>,
}

impl Default for LinkModel {
    fn default() -> Self {
        Self {
            next_req_id: 1,
            next_tx_slot: 0,
            next_rx_slot: 0,
            supplied_rx: Vec::new(),
            completed_rx: VecDeque::new(),
            in_flight_tx: Vec::new(),
            completed_tx: Vec::new(),
        }
    }
}

impl LinkModel {
    pub fn tx_entry(&mut self, vmo_id: u32, len: usize) -> Result<FrameEntry, Status> {
        if len == 0 || len > SLOT_SIZE || len > u16::MAX as usize {
            return Err(Status::ErrInvalidArgs);
        }
        let slot = (0..SLOT_COUNT)
            .map(|step| (self.next_tx_slot + step) % SLOT_COUNT)
            .find(|slot| {
                !self
                    .in_flight_tx
                    .iter()
                    .any(|entry| entry.offset as usize == slot * SLOT_SIZE)
            })
            .ok_or(Status::ErrShouldWait)?;
        self.next_tx_slot = (slot + 1) % SLOT_COUNT;
        let entry = FrameEntry {
            req_id: self.take_req_id(),
            opcode: FrameOpcode::TxSend,
            vmo_id,
            offset: (slot * SLOT_SIZE) as u32,
            length: len as u16,
            flags: FrameFlags(0),
        };
        self.in_flight_tx.push(entry);
        Ok(entry)
    }

    pub fn tx_available(&self) -> bool {
        self.in_flight_tx.len() < SLOT_COUNT
    }

    fn cancel_tx(&mut self, req_id: u64) {
        self.in_flight_tx.retain(|entry| entry.req_id != req_id);
    }

    pub fn rx_supply_entries(&mut self, vmo_id: u32, target: usize) -> Vec<FrameEntry> {
        let mut entries = Vec::new();
        while self.supplied_rx.len() + self.completed_rx.len() < target.min(SLOT_COUNT) {
            let Some(slot) = (0..SLOT_COUNT)
                .map(|step| (self.next_rx_slot + step) % SLOT_COUNT)
                .find(|slot| {
                    !self
                        .supplied_rx
                        .iter()
                        .chain(self.completed_rx.iter())
                        .any(|entry| entry.offset as usize == slot * SLOT_SIZE)
                })
            else {
                break;
            };
            self.next_rx_slot = (slot + 1) % SLOT_COUNT;
            let entry = FrameEntry {
                req_id: self.take_req_id(),
                opcode: FrameOpcode::RxSupply,
                vmo_id,
                offset: (slot * SLOT_SIZE) as u32,
                length: SLOT_SIZE as u16,
                flags: FrameFlags(0),
            };
            self.supplied_rx.push(entry);
            entries.push(entry);
        }
        entries
    }

    pub fn complete(&mut self, entry: FrameEntry) {
        match entry.opcode {
            FrameOpcode::RxComplete => {
                if !self.supplied_rx.iter().any(|candidate| {
                    candidate.req_id == entry.req_id
                        && candidate.vmo_id == entry.vmo_id
                        && candidate.offset == entry.offset
                        && entry.length <= candidate.length
                }) {
                    return;
                }
                self.supplied_rx
                    .retain(|candidate| candidate.req_id != entry.req_id);
                if entry.length != 0 {
                    self.completed_rx.push_back(entry);
                }
            }
            FrameOpcode::TxComplete => {
                if self
                    .in_flight_tx
                    .iter()
                    .any(|candidate| candidate.req_id == entry.req_id)
                {
                    self.cancel_tx(entry.req_id);
                    self.completed_tx.push(entry);
                }
            }
            _ => {}
        }
    }

    pub fn pop_rx(&mut self) -> Option<PacketFrame> {
        self.completed_rx.pop_front().map(|entry| PacketFrame {
            req_id: entry.req_id,
            len: entry.length as usize,
            offset: entry.offset,
        })
    }

    pub fn drain_tx_completions(&mut self) -> usize {
        let count = self.completed_tx.len();
        self.completed_tx.clear();
        count
    }

    pub fn idle(&self) -> bool {
        self.completed_rx.is_empty() && self.completed_tx.is_empty()
    }

    fn take_req_id(&mut self) -> u64 {
        let id = self.next_req_id;
        self.next_req_id = self.next_req_id.wrapping_add(1).max(1);
        id
    }
}

pub struct PacketLink {
    pub resources: LinkResources,
    model: LinkModel,
    rx_backlog: VecDeque<Vec<u8>>,
    queues_restored: bool,
}

impl PacketLink {
    pub fn new(link: EthernetLink) -> Result<Self, Status> {
        let rx_vaddr = Memory::map(link.rx_vmo, RX_BYTES, 6).map_err(map_kernel_status)?;
        let tx_vaddr = Memory::map(link.tx_vmo, TX_BYTES, 6).map_err(map_kernel_status)?;
        let mut packet_link = Self {
            resources: LinkResources {
                control: link.control.0,
                fifo: link.fifo.0,
                rx_vmo: link.rx_vmo,
                tx_vmo: link.tx_vmo,
                rx_vaddr,
                tx_vaddr,
                rx_vmo_id: link.rx_vmo_id,
                tx_vmo_id: link.tx_vmo_id,
                mtu: link.mtu,
                mac: link.mac,
            },
            model: LinkModel::default(),
            rx_backlog: VecDeque::new(),
            queues_restored: true,
        };
        let _ = packet_link.supply_rx();
        Ok(packet_link)
    }

    pub fn from_resources(resources: LinkResources) -> Self {
        Self {
            resources,
            model: LinkModel::default(),
            rx_backlog: VecDeque::new(),
            queues_restored: true,
        }
    }

    pub fn transmit(&mut self, bytes: &[u8]) -> Status {
        let entry = match self.model.tx_entry(self.resources.tx_vmo_id, bytes.len()) {
            Ok(entry) => entry,
            Err(status) => return status,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.resources.tx_vaddr + entry.offset as u64) as *mut u8,
                bytes.len(),
            );
        }
        let status = send_frame(Channel(self.resources.fifo), entry);
        if status != Status::Ok {
            self.model.cancel_tx(entry.req_id);
        }
        if status != Status::ErrPeerClosed || self.reconnect().is_err() {
            return status;
        }
        let entry = match self.model.tx_entry(self.resources.tx_vmo_id, bytes.len()) {
            Ok(entry) => entry,
            Err(status) => return status,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.resources.tx_vaddr + entry.offset as u64) as *mut u8,
                bytes.len(),
            );
        }
        let status = send_frame(Channel(self.resources.fifo), entry);
        if status != Status::Ok {
            self.model.cancel_tx(entry.req_id);
        }
        status
    }

    pub fn poll(&mut self) -> Status {
        let fifo = Channel(self.resources.fifo);
        loop {
            match fifo.try_recv() {
                Ok(message) => match FrameEntry::decode(&message.bytes, &[]) {
                    Ok(entry) => self.model.complete(entry),
                    Err(_) => return Status::ErrInvalidArgs,
                },
                Err(kernel_fidl::Status::ErrTimedOut) => break,
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    if let Err(status) = self.reconnect() {
                        return status;
                    }
                    return self.supply_rx();
                }
                Err(status) => return map_kernel_status(status),
            }
        }
        self.model.drain_tx_completions();
        self.supply_rx()
    }

    pub fn receive(&mut self, out: &mut [u8]) -> Result<usize, Status> {
        if let Some(bytes) = self.rx_backlog.pop_front() {
            if bytes.len() > out.len() {
                return Err(Status::ErrBufferTooSmall);
            }
            out[..bytes.len()].copy_from_slice(&bytes);
            return Ok(bytes.len());
        }
        self.receive_device(out)
    }

    /// Consume only newly completed DMA frames. Frames deferred to smoltcp
    /// must not re-enter the protocol classification loop that deferred them.
    pub fn receive_device(&mut self, out: &mut [u8]) -> Result<usize, Status> {
        let frame = self.model.pop_rx().ok_or(Status::ErrShouldWait)?;
        if frame.len > out.len() {
            return Err(Status::ErrBufferTooSmall);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                (self.resources.rx_vaddr + frame.offset as u64) as *const u8,
                out.as_mut_ptr(),
                frame.len,
            );
        }
        Ok(frame.len)
    }

    pub fn push_rx_backlog(&mut self, frame: &[u8]) -> Status {
        if frame.len() > SLOT_SIZE {
            return Status::ErrBufferTooSmall;
        }
        if self.rx_backlog.len() >= SLOT_COUNT {
            return Status::ErrResourceExhausted;
        }
        self.rx_backlog.push_back(frame.to_vec());
        Status::Ok
    }

    pub fn drain_for_quiesce(&mut self) -> bool {
        let _ = self.poll();
        self.model.idle()
    }

    fn supply_rx(&mut self) -> Status {
        for entry in self
            .model
            .rx_supply_entries(self.resources.rx_vmo_id, SLOT_COUNT)
        {
            let status = send_frame(Channel(self.resources.fifo), entry);
            if status == Status::ErrPeerClosed && self.reconnect().is_ok() {
                let status = send_frame(Channel(self.resources.fifo), entry);
                if status != Status::Ok {
                    return status;
                }
            } else if status != Status::Ok {
                return status;
            }
        }
        Status::Ok
    }

    fn reconnect(&mut self) -> Result<(), Status> {
        let link = crate::ethernet::reconnect(self.resources).map_err(map_ethernet_status)?;
        self.resources.fifo = link.fifo.0;
        self.resources.rx_vmo_id = link.rx_vmo_id;
        self.resources.tx_vmo_id = link.tx_vmo_id;
        self.resources.mtu = link.mtu;
        self.resources.mac = link.mac;
        self.model = LinkModel::default();
        Ok(())
    }
}

fn map_ethernet_status(status: ethernet_fidl::Status) -> Status {
    match status {
        ethernet_fidl::Status::Ok => Status::Ok,
        ethernet_fidl::Status::ErrInvalidHandle => Status::ErrInvalidHandle,
        ethernet_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        ethernet_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        ethernet_fidl::Status::ErrBufferTooSmall => Status::ErrBufferTooSmall,
        ethernet_fidl::Status::ErrPeerClosed => Status::ErrPeerClosed,
        ethernet_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        ethernet_fidl::Status::ErrShouldWait => Status::ErrShouldWait,
        _ => Status::ErrInvalidArgs,
    }
}

impl phy::Device for PacketLink {
    type RxToken<'a>
        = FifoRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = FifoTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.model.tx_available() {
            return None;
        }
        let _ = self.poll();
        let mut scratch = [0; SLOT_SIZE];
        let len = PacketLink::receive(self, &mut scratch).ok()?;
        let bytes = scratch[..len].to_vec();
        Some((FifoRxToken { bytes }, FifoTxToken { link: self }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.model
            .tx_available()
            .then_some(FifoTxToken { link: self })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.resources.mtu as usize + 14;
        caps.max_burst_size = Some(1);
        caps
    }
}

pub struct FifoRxToken {
    bytes: Vec<u8>,
}

impl phy::RxToken for FifoRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.bytes)
    }
}

pub struct FifoTxToken<'a> {
    link: &'a mut PacketLink,
}

impl phy::TxToken for FifoTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut bytes = [0u8; SLOT_SIZE];
        let result = f(&mut bytes[..len.min(SLOT_SIZE)]);
        let _ = self.link.transmit(&bytes[..len.min(SLOT_SIZE)]);
        result
    }
}

fn send_frame(fifo: Channel, entry: FrameEntry) -> Status {
    let mut out = [0u8; 64];
    let encoded = match entry.encode(&mut out, &mut []) {
        Ok(encoded) => encoded,
        Err(_) => return Status::ErrInvalidArgs,
    };
    fifo.send(&out[..encoded.bytes], &[])
        .map(|_| Status::Ok)
        .unwrap_or_else(map_kernel_status)
}

fn map_kernel_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::Ok => Status::Ok,
        kernel_fidl::Status::ErrInvalidHandle => Status::ErrInvalidHandle,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrBufferTooSmall => Status::ErrBufferTooSmall,
        kernel_fidl::Status::ErrPeerClosed => Status::ErrPeerClosed,
        kernel_fidl::Status::ErrTimedOut => Status::ErrShouldWait,
        kernel_fidl::Status::ErrAlreadyExists => Status::ErrAlreadyExists,
        kernel_fidl::Status::ErrResourceExhausted => Status::ErrResourceExhausted,
        _ => Status::ErrInvalidArgs,
    }
}
