use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec;
use alloc::vec::Vec;

use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory, Rpc, live_migration::Resource};
use ethernet_fidl::{
    DeviceGetFifoRequest, DeviceGetInfoRequest, DevicePublicClient, DeviceRegisterBufferRequest,
    DeviceStartRequest, FidlDecode, FidlEncode, FrameEntry, FrameFlags, FrameOpcode, HandleRef,
    Status,
};

use crate::switch::{MAX_FRAME_SIZE, QueuedFrame, VirtualSwitch};

const SLOT_SIZE: usize = 9216;
const SLOT_COUNT: usize = 64;
const BUFFER_BYTES: u64 = (SLOT_SIZE * SLOT_COUNT) as u64;

pub struct PhysicalDevice {
    pub control: Channel,
    pub fifo: Channel,
    pub rx_vmo: u64,
    pub tx_vmo: u64,
    pub rx_vaddr: u64,
    pub tx_vaddr: u64,
    pub rx_vmo_id: u32,
    pub tx_vmo_id: u32,
    pub mtu: u32,
    pub mac: [u8; 6],
    pub next_request: u64,
    pub supplied_rx: BTreeMap<u64, FrameEntry>,
    pub pending_tx: BTreeMap<u64, FrameEntry>,
    pub outbound: VecDeque<QueuedFrame>,
}

impl PhysicalDevice {
    pub fn connect(endpoint: u64) -> Result<Self, Status> {
        let control = Channel(endpoint);
        let mut client = DevicePublicClient::new(Rpc(control));
        let mut req = [0; 64];
        let mut resp = [0; 256];
        let mut req_handles = [HandleRef { raw: 0 }; 4];
        let mut resp_handles = [HandleRef { raw: 0 }; 4];
        let info = client
            .get_info(
                &DeviceGetInfoRequest {},
                &mut req,
                &mut req_handles,
                &mut resp,
                &mut resp_handles,
            )
            .map_err(|_| Status::ErrInvalidArgs)?;
        let rx_vmo = Memory::create(BUFFER_BYTES, 0).map_err(map_status)?;
        let tx_vmo = Memory::create(BUFFER_BYTES, 0).map_err(map_status)?;
        let rx_vmo_id = register(&mut client, duplicate(rx_vmo)?)?;
        let tx_vmo_id = register(&mut client, duplicate(tx_vmo)?)?;
        let fifo = client
            .get_fifo(
                &DeviceGetFifoRequest {},
                &mut req,
                &mut req_handles,
                &mut resp,
                &mut resp_handles,
            )
            .map_err(|_| Status::ErrInvalidArgs)?;
        if fifo.status != Status::Ok {
            return Err(fifo.status);
        }
        let start = client
            .start(
                &DeviceStartRequest {},
                &mut req,
                &mut req_handles,
                &mut resp,
                &mut resp_handles,
            )
            .map_err(|_| Status::ErrInvalidArgs)?;
        if start.status != Status::Ok {
            return Err(start.status);
        }
        let rx_vaddr = Memory::map(rx_vmo, BUFFER_BYTES, 6).map_err(map_status)?;
        let tx_vaddr = Memory::map(tx_vmo, BUFFER_BYTES, 6).map_err(map_status)?;
        let mut device = Self {
            control,
            fifo: Channel(fifo.fifo_handle.raw),
            rx_vmo,
            tx_vmo,
            rx_vaddr,
            tx_vaddr,
            rx_vmo_id,
            tx_vmo_id,
            mtu: info.info.mtu,
            mac: info.info.mac.octets,
            next_request: 1,
            supplied_rx: BTreeMap::new(),
            pending_tx: BTreeMap::new(),
            outbound: VecDeque::new(),
        };
        device.fill_rx();
        Ok(device)
    }

    pub fn enqueue(&mut self, frames: Vec<QueuedFrame>) -> Result<(), Status> {
        if self.outbound.len().saturating_add(frames.len()) > 256 {
            return Err(Status::ErrNoMemory);
        }
        self.outbound.extend(frames);
        Ok(())
    }

    pub fn can_enqueue(&self, count: usize) -> bool {
        self.outbound.len().saturating_add(count) <= 256
    }

    pub fn poll(&mut self, interface: u64, switch: &mut VirtualSwitch) -> bool {
        let mut changed = false;
        loop {
            let Ok(message) = self.fifo.try_recv() else {
                break;
            };
            let Ok(entry) = FrameEntry::decode(&message.bytes, &[]) else {
                continue;
            };
            changed = true;
            match entry.opcode {
                FrameOpcode::RxComplete => {
                    if self.supplied_rx.remove(&entry.req_id).is_some()
                        && usize::from(entry.length) <= SLOT_SIZE
                    {
                        let mut bytes = vec![0; usize::from(entry.length)];
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                (self.rx_vaddr + u64::from(entry.offset)) as *const u8,
                                bytes.as_mut_ptr(),
                                bytes.len(),
                            );
                        }
                        switch.ingress(interface, &bytes);
                        self.supply_rx_slot(entry.offset);
                    }
                }
                FrameOpcode::TxComplete => {
                    self.pending_tx.remove(&entry.req_id);
                }
                _ => {}
            }
        }
        while self.pending_tx.len() < SLOT_COUNT {
            let Some(frame) = self.outbound.pop_front() else {
                break;
            };
            if self.transmit(&frame.bytes).is_err() {
                self.outbound.push_front(frame);
                break;
            }
            changed = true;
        }
        changed
    }

    fn transmit(&mut self, bytes: &[u8]) -> Result<(), Status> {
        if bytes.is_empty() || bytes.len() > SLOT_SIZE || bytes.len() > self.mtu as usize + 32 {
            return Err(Status::ErrInvalidArgs);
        }
        let slot = (0..SLOT_COUNT)
            .find(|slot| {
                !self
                    .pending_tx
                    .values()
                    .any(|entry| entry.offset as usize == slot * SLOT_SIZE)
            })
            .ok_or(Status::ErrShouldWait)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.tx_vaddr + (slot * SLOT_SIZE) as u64) as *mut u8,
                bytes.len(),
            );
        }
        let entry = FrameEntry {
            req_id: self.take_request(),
            opcode: FrameOpcode::TxSend,
            vmo_id: self.tx_vmo_id,
            offset: (slot * SLOT_SIZE) as u32,
            length: bytes.len() as u16,
            flags: FrameFlags(0),
        };
        send(self.fifo, entry)?;
        self.pending_tx.insert(entry.req_id, entry);
        Ok(())
    }

    fn fill_rx(&mut self) {
        for slot in 0..SLOT_COUNT {
            self.supply_rx_slot((slot * SLOT_SIZE) as u32);
        }
    }

    fn supply_rx_slot(&mut self, offset: u32) {
        let entry = FrameEntry {
            req_id: self.take_request(),
            opcode: FrameOpcode::RxSupply,
            vmo_id: self.rx_vmo_id,
            offset,
            length: SLOT_SIZE as u16,
            flags: FrameFlags(0),
        };
        if send(self.fifo, entry).is_ok() {
            self.supplied_rx.insert(entry.req_id, entry);
        }
    }

    fn take_request(&mut self) -> u64 {
        let request = self.next_request;
        self.next_request = self.next_request.wrapping_add(1).max(1);
        request
    }
}

fn register(client: &mut DevicePublicClient<Rpc>, vmo: u64) -> Result<u32, Status> {
    let mut req = [0; 64];
    let mut resp = [0; 64];
    let mut req_handles = [HandleRef { raw: 0 }; 2];
    let mut resp_handles = [HandleRef { raw: 0 }; 2];
    let response = client
        .register_buffer(
            &DeviceRegisterBufferRequest {
                vmo: HandleRef { raw: vmo },
                size_bytes: BUFFER_BYTES as u32,
            },
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    if response.status == Status::Ok {
        Ok(response.vmo_id)
    } else {
        Err(response.status)
    }
}

fn duplicate(handle: u64) -> Result<u64, Status> {
    Memory::duplicate(handle, 1 | 2 | 4 | 16 | 32).map_err(map_status)
}

fn send(fifo: Channel, entry: FrameEntry) -> Result<(), Status> {
    let mut bytes = [0; 64];
    let encoded = entry
        .encode(&mut bytes, &mut [])
        .map_err(|_| Status::ErrInvalidArgs)?;
    fifo.send(&bytes[..encoded.bytes], &[]).map_err(map_status)
}

fn map_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        kernel_fidl::Status::ErrPeerClosed => Status::ErrPeerClosed,
        kernel_fidl::Status::ErrTimedOut => Status::ErrShouldWait,
        _ => Status::ErrInvalidHandle,
    }
}

pub fn encode_device(w: &mut Encoder, device: &PhysicalDevice) {
    for word in [
        device.control.0,
        device.fifo.0,
        device.rx_vmo,
        device.tx_vmo,
        device.rx_vaddr,
        device.tx_vaddr,
        u64::from(device.rx_vmo_id),
        u64::from(device.tx_vmo_id),
        u64::from(device.mtu),
        device.next_request,
    ] {
        w.word(word);
    }
    w.bytes(&device.mac);
    encode_entries(w, device.supplied_rx.values());
    encode_entries(w, device.pending_tx.values());
    w.word(device.outbound.len() as u64);
    for frame in &device.outbound {
        w.word(frame.generation);
        w.bytes(&frame.bytes);
    }
}

fn encode_entries<'a>(w: &mut Encoder, entries: impl Iterator<Item = &'a FrameEntry>) {
    let entries = entries.collect::<Vec<_>>();
    w.word(entries.len() as u64);
    for entry in entries {
        let mut bytes = [0; 64];
        let encoded = entry.encode(&mut bytes, &mut []).expect("frame encoding");
        w.bytes(&bytes[..encoded.bytes]);
    }
}

pub fn decode_device(r: &mut Decoder<'_>) -> Result<PhysicalDevice, Error> {
    let control = Channel(r.word()?);
    let fifo = Channel(r.word()?);
    let rx_vmo = r.word()?;
    let tx_vmo = r.word()?;
    let rx_vaddr = r.word()?;
    let tx_vaddr = r.word()?;
    let rx_vmo_id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let tx_vmo_id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let mtu = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let next_request = r.word()?;
    let mac = r.bytes(6)?.try_into().map_err(|_| Error::InvalidData)?;
    let supplied_rx = decode_entries(r, FrameOpcode::RxSupply)?;
    let pending_tx = decode_entries(r, FrameOpcode::TxSend)?;
    let mut outbound = VecDeque::new();
    for _ in 0..r.count(256)? {
        outbound.push_back(QueuedFrame {
            generation: r.word()?,
            bytes: r.bytes(MAX_FRAME_SIZE)?.to_vec(),
        });
    }
    Ok(PhysicalDevice {
        control,
        fifo,
        rx_vmo,
        tx_vmo,
        rx_vaddr,
        tx_vaddr,
        rx_vmo_id,
        tx_vmo_id,
        mtu,
        mac,
        next_request,
        supplied_rx,
        pending_tx,
        outbound,
    })
}

fn decode_entries(
    r: &mut Decoder<'_>,
    opcode: FrameOpcode,
) -> Result<BTreeMap<u64, FrameEntry>, Error> {
    let mut entries = BTreeMap::new();
    for _ in 0..r.count(SLOT_COUNT)? {
        let entry = FrameEntry::decode(r.bytes(64)?, &[]).map_err(|_| Error::InvalidData)?;
        if entry.opcode != opcode || entries.insert(entry.req_id, entry).is_some() {
            return Err(Error::InvalidData);
        }
    }
    Ok(entries)
}

pub fn append_resources(resources: &mut Vec<Resource>, device: &PhysicalDevice) {
    resources.extend([
        Resource::Handle(device.control.0),
        Resource::Handle(device.fifo.0),
        Resource::Handle(device.rx_vmo),
        Resource::Handle(device.tx_vmo),
        Resource::Mapping {
            handle: device.rx_vmo,
            offset: 0,
            va: device.rx_vaddr,
            size: BUFFER_BYTES,
            rights: 6,
        },
        Resource::Mapping {
            handle: device.tx_vmo,
            offset: 0,
            va: device.tx_vaddr,
            size: BUFFER_BYTES,
            rights: 6,
        },
    ]);
}
