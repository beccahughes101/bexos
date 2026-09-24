use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec;
use alloc::vec::Vec;

use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory, live_migration::Resource};
use ethernet_fidl::{
    DeviceFeatures, DeviceGetFifoRequest, DeviceGetFifoResponse, DeviceGetInfoRequest,
    DeviceGetInfoResponse, DeviceRegisterBufferRequest, DeviceRegisterBufferResponse,
    DeviceStartRequest, DeviceStartResponse, DeviceStopRequest, DeviceStopResponse,
    DeviceUnregisterBufferRequest, DeviceUnregisterBufferResponse, EthernetInfo, FidlDecode,
    FidlEncode, FrameEntry, FrameFlags, FrameOpcode, HandleRef, MacAddress, Status,
};

use crate::switch::VirtualSwitch;

const MAX_BUFFERS: usize = 16;
const MAX_FIFOS: usize = 8;
const MAX_PENDING_RX: usize = 256;

#[derive(Clone, Copy)]
pub struct Buffer {
    pub handle: u64,
    pub vaddr: u64,
    pub size: u32,
    pub mapped_len: u64,
}

pub struct VirtualDevice {
    pub control: Channel,
    pub fifos: Vec<Channel>,
    pub buffers: BTreeMap<u32, Buffer>,
    pub next_buffer_id: u32,
    pub running: bool,
    pub rx_supplies: VecDeque<(u64, FrameEntry)>,
}

impl VirtualDevice {
    pub fn new(control: Channel) -> Self {
        Self {
            control,
            fifos: Vec::new(),
            buffers: BTreeMap::new(),
            next_buffer_id: 1,
            running: false,
            rx_supplies: VecDeque::new(),
        }
    }

    pub fn close(&mut self) {
        let _ = Memory::close(self.control.0);
        for fifo in self.fifos.drain(..) {
            let _ = Memory::close(fifo.0);
        }
        for buffer in core::mem::take(&mut self.buffers).into_values() {
            let _ = Memory::unmap(buffer.vaddr, buffer.mapped_len);
            let _ = Memory::close(buffer.handle);
        }
    }
}

pub fn poll_devices(
    devices: &mut BTreeMap<u64, VirtualDevice>,
    switch: &mut VirtualSwitch,
) -> bool {
    let mut changed = false;
    let ids = devices.keys().copied().collect::<Vec<_>>();
    for port_id in ids {
        let Some(policy) = switch.port(port_id).map(|port| port.policy) else {
            continue;
        };
        let Some(device) = devices.get_mut(&port_id) else {
            continue;
        };
        match device.control.try_recv() {
            Ok(message) => {
                changed = true;
                handle_control(device, policy.source_mac, &message.bytes, &message.handles);
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                device.running = false;
            }
            Err(_) => {}
        }
        let fifos = device.fifos.clone();
        for fifo in fifos {
            loop {
                let Ok(message) = fifo.try_recv() else { break };
                changed = true;
                let Ok(entry) = FrameEntry::decode(&message.bytes, &[]) else {
                    continue;
                };
                match entry.opcode {
                    FrameOpcode::RxSupply if device.running => {
                        if valid_range(device, entry).is_ok()
                            && device.rx_supplies.len() < MAX_PENDING_RX
                        {
                            device.rx_supplies.push_back((fifo.0, entry));
                        } else {
                            complete(fifo, entry, FrameOpcode::RxComplete, 0, true);
                        }
                    }
                    FrameOpcode::TxSend if device.running => {
                        let status = frame_bytes(device, entry)
                            .and_then(|bytes| switch.stage_egress(port_id, &bytes));
                        complete(
                            fifo,
                            entry,
                            FrameOpcode::TxComplete,
                            if status.is_ok() { entry.length } else { 0 },
                            status.is_err(),
                        );
                    }
                    _ => complete(fifo, entry, completion_opcode(entry.opcode), 0, true),
                }
            }
        }
        while device.running && !device.rx_supplies.is_empty() {
            let Some(frame) = switch.receive(port_id).ok().flatten() else {
                break;
            };
            let (fifo, entry) = device.rx_supplies.pop_front().expect("checked supply");
            let capacity = usize::from(entry.length);
            let length = frame.bytes.len().min(capacity);
            let copied = buffer_slice_mut(device, entry, length).map(|out| {
                out.copy_from_slice(&frame.bytes[..length]);
            });
            complete(
                Channel(fifo),
                entry,
                FrameOpcode::RxComplete,
                if copied.is_ok() { length as u16 } else { 0 },
                copied.is_err() || frame.bytes.len() > capacity,
            );
            changed = true;
        }
    }
    changed
}

fn handle_control(device: &mut VirtualDevice, mac: [u8; 6], bytes: &[u8], handles: &[u64]) {
    let (ordinal, request) = envelope(bytes);
    let refs = handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    match ordinal {
        1 => {
            let response = if DeviceGetInfoRequest::decode(request, &refs).is_ok() {
                DeviceGetInfoResponse {
                    info: EthernetInfo {
                        mac: MacAddress { octets: mac },
                        mtu: 1500,
                        features: DeviceFeatures(0),
                    },
                }
            } else {
                return;
            };
            reply(device.control, &response);
        }
        2 => {
            let response = match DeviceRegisterBufferRequest::decode(request, &refs) {
                Ok(request) => match register_buffer(device, request.vmo.raw, request.size_bytes) {
                    Ok(vmo_id) => DeviceRegisterBufferResponse {
                        status: Status::Ok,
                        vmo_id,
                    },
                    Err(status) => DeviceRegisterBufferResponse { status, vmo_id: 0 },
                },
                Err(_) => DeviceRegisterBufferResponse {
                    status: Status::ErrInvalidArgs,
                    vmo_id: 0,
                },
            };
            reply(device.control, &response);
        }
        3 => {
            let response = DeviceUnregisterBufferRequest::decode(request, &refs)
                .map(|request| DeviceUnregisterBufferResponse {
                    status: unregister_buffer(device, request.vmo_id),
                })
                .unwrap_or(DeviceUnregisterBufferResponse {
                    status: Status::ErrInvalidArgs,
                });
            reply(device.control, &response);
        }
        4 => {
            let response = if DeviceGetFifoRequest::decode(request, &refs).is_err()
                || device.fifos.len() >= MAX_FIFOS
            {
                DeviceGetFifoResponse {
                    status: Status::ErrShouldWait,
                    fifo_handle: HandleRef { raw: 0 },
                }
            } else if let Ok((server, client)) = Channel::pair() {
                device.fifos.push(server);
                DeviceGetFifoResponse {
                    status: Status::Ok,
                    fifo_handle: HandleRef { raw: client.0 },
                }
            } else {
                DeviceGetFifoResponse {
                    status: Status::ErrNoMemory,
                    fifo_handle: HandleRef { raw: 0 },
                }
            };
            reply(device.control, &response);
        }
        5 => {
            let status = if DeviceStartRequest::decode(request, &refs).is_ok() {
                device.running = true;
                Status::Ok
            } else {
                Status::ErrInvalidArgs
            };
            reply(device.control, &DeviceStartResponse { status });
        }
        6 => {
            let status = if DeviceStopRequest::decode(request, &refs).is_ok() {
                device.running = false;
                Status::Ok
            } else {
                Status::ErrInvalidArgs
            };
            reply(device.control, &DeviceStopResponse { status });
        }
        _ => {}
    }
}

fn register_buffer(device: &mut VirtualDevice, handle: u64, size: u32) -> Result<u32, Status> {
    if handle == 0 || size == 0 || device.buffers.len() >= MAX_BUFFERS {
        let _ = Memory::close(handle);
        return Err(Status::ErrInvalidArgs);
    }
    let mapped_len = u64::from(size)
        .checked_add(4095)
        .ok_or(Status::ErrInvalidArgs)?
        & !4095;
    let vaddr = Memory::map(handle, mapped_len, 6).map_err(map_status)?;
    let id = device.next_buffer_id;
    device.next_buffer_id = device
        .next_buffer_id
        .checked_add(1)
        .ok_or(Status::ErrNoMemory)?;
    device.buffers.insert(
        id,
        Buffer {
            handle,
            vaddr,
            size,
            mapped_len,
        },
    );
    Ok(id)
}

fn unregister_buffer(device: &mut VirtualDevice, id: u32) -> Status {
    if device
        .rx_supplies
        .iter()
        .any(|(_, entry)| entry.vmo_id == id)
    {
        return Status::ErrShouldWait;
    }
    let Some(buffer) = device.buffers.remove(&id) else {
        return Status::ErrInvalidArgs;
    };
    let _ = Memory::unmap(buffer.vaddr, buffer.mapped_len);
    let _ = Memory::close(buffer.handle);
    Status::Ok
}

fn frame_bytes(
    device: &VirtualDevice,
    entry: FrameEntry,
) -> Result<Vec<u8>, crate::switch::SwitchError> {
    let range = valid_range(device, entry)?;
    let mut bytes = vec![0; range.1];
    unsafe {
        core::ptr::copy_nonoverlapping(range.0 as *const u8, bytes.as_mut_ptr(), range.1);
    }
    Ok(bytes)
}

fn buffer_slice_mut(
    device: &VirtualDevice,
    entry: FrameEntry,
    len: usize,
) -> Result<&mut [u8], crate::switch::SwitchError> {
    let (address, _) = valid_range_len(device, entry, len)?;
    Ok(unsafe { core::slice::from_raw_parts_mut(address as *mut u8, len) })
}

fn valid_range(
    device: &VirtualDevice,
    entry: FrameEntry,
) -> Result<(u64, usize), crate::switch::SwitchError> {
    valid_range_len(device, entry, usize::from(entry.length))
}

fn valid_range_len(
    device: &VirtualDevice,
    entry: FrameEntry,
    len: usize,
) -> Result<(u64, usize), crate::switch::SwitchError> {
    let buffer = device
        .buffers
        .get(&entry.vmo_id)
        .ok_or(crate::switch::SwitchError::InvalidFrame)?;
    let end = usize::try_from(entry.offset)
        .ok()
        .and_then(|offset| offset.checked_add(len))
        .ok_or(crate::switch::SwitchError::InvalidFrame)?;
    if end > buffer.size as usize {
        return Err(crate::switch::SwitchError::InvalidFrame);
    }
    Ok((buffer.vaddr + u64::from(entry.offset), len))
}

fn complete(fifo: Channel, entry: FrameEntry, opcode: FrameOpcode, length: u16, error: bool) {
    let response = FrameEntry {
        req_id: entry.req_id,
        opcode,
        vmo_id: entry.vmo_id,
        offset: entry.offset,
        length,
        flags: FrameFlags(if error { 1 } else { 0 }),
    };
    reply(fifo, &response);
}

fn completion_opcode(opcode: FrameOpcode) -> FrameOpcode {
    if opcode == FrameOpcode::RxSupply {
        FrameOpcode::RxComplete
    } else {
        FrameOpcode::TxComplete
    }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut bytes = [0; 256];
    let mut handles = [HandleRef { raw: 0 }; 4];
    if let Ok(encoded) = response.encode(&mut bytes, &mut handles) {
        let raw = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = channel.send(&bytes[..encoded.bytes], &raw);
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn map_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        _ => Status::ErrInvalidHandle,
    }
}

pub fn encode_device(w: &mut Encoder, device: &VirtualDevice) {
    w.word(device.control.0);
    w.word(device.next_buffer_id as u64);
    w.word(device.running as u64);
    w.word(device.fifos.len() as u64);
    for fifo in &device.fifos {
        w.word(fifo.0);
    }
    w.word(device.buffers.len() as u64);
    for (id, buffer) in &device.buffers {
        w.word(u64::from(*id));
        w.word(buffer.handle);
        w.word(buffer.vaddr);
        w.word(u64::from(buffer.size));
        w.word(buffer.mapped_len);
    }
    w.word(device.rx_supplies.len() as u64);
    for (fifo, entry) in &device.rx_supplies {
        w.word(*fifo);
        let mut bytes = [0; 64];
        let encoded = entry.encode(&mut bytes, &mut []).expect("frame encoding");
        w.bytes(&bytes[..encoded.bytes]);
    }
}

pub fn decode_device(r: &mut Decoder<'_>) -> Result<VirtualDevice, Error> {
    let control = Channel(r.word()?);
    let next_buffer_id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let running = r.flag()?;
    let mut fifos = Vec::new();
    for _ in 0..r.count(MAX_FIFOS)? {
        fifos.push(Channel(r.word()?));
    }
    let mut buffers = BTreeMap::new();
    for _ in 0..r.count(MAX_BUFFERS)? {
        let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let buffer = Buffer {
            handle: r.word()?,
            vaddr: r.word()?,
            size: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            mapped_len: r.word()?,
        };
        if buffers.insert(id, buffer).is_some() {
            return Err(Error::InvalidData);
        }
    }
    let mut rx_supplies = VecDeque::new();
    for _ in 0..r.count(MAX_PENDING_RX)? {
        let fifo = r.word()?;
        let entry = FrameEntry::decode(r.bytes(64)?, &[]).map_err(|_| Error::InvalidData)?;
        rx_supplies.push_back((fifo, entry));
    }
    Ok(VirtualDevice {
        control,
        fifos,
        buffers,
        next_buffer_id,
        running,
        rx_supplies,
    })
}

pub fn append_resources(resources: &mut Vec<Resource>, device: &VirtualDevice) {
    resources.push(Resource::Handle(device.control.0));
    resources.extend(device.fifos.iter().map(|fifo| Resource::Handle(fifo.0)));
    for buffer in device.buffers.values() {
        resources.push(Resource::Handle(buffer.handle));
        resources.push(Resource::Mapping {
            handle: buffer.handle,
            offset: 0,
            va: buffer.vaddr,
            size: buffer.mapped_len,
            rights: 6,
        });
    }
}
