use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use bexos_intel_nic::{Controller, DmaRange, LegacyDescriptor, RegisterBank, register};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory};
use bexos_userspace::live_migration::{Resource, Source};
use ethernet_fidl::{
    DeviceFeatures, DeviceGetFifoResponse, DeviceGetInfoResponse, DeviceRegisterBufferRequest,
    DeviceRegisterBufferResponse, DeviceStartResponse, DeviceStopResponse,
    DeviceUnregisterBufferRequest, DeviceUnregisterBufferResponse, EthernetInfo, FidlDecode,
    FidlEncode, FrameEntry, FrameFlags, FrameOpcode, HandleRef, MacAddress, Status,
};

use super::{MmioRegisters, Runtime, envelope};

const PAGE_SIZE: u64 = 4096;
const RING_BYTES: u64 = PAGE_SIZE * 2;
const RX_RING_OFFSET: u64 = 0;
const TX_RING_OFFSET: u64 = PAGE_SIZE;
const DESCRIPTOR_BYTES: u64 = 16;
const MAX_BUFFERS: usize = 256;
const MAX_PENDING: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Buffer {
    handle: u64,
    vaddr: u64,
    dma: u64,
    token: u64,
    size: u64,
}

#[derive(Clone, Copy)]
struct PendingFrame {
    fifo: Channel,
    entry: FrameEntry,
    descriptor: usize,
}

pub(super) struct EthernetRuntime {
    mac: [u8; 6],
    next_buffer_id: u32,
    running: bool,
    buffers: BTreeMap<u32, Buffer>,
    fifos: Vec<Channel>,
    rx_pending: VecDeque<PendingFrame>,
    tx_pending: VecDeque<PendingFrame>,
    ring_handle: u64,
    ring_vaddr: u64,
    ring_dma: u64,
    ring_token: u64,
}

impl EthernetRuntime {
    pub(super) fn new<R: RegisterBank>(
        iommu: u64,
        mac: [u8; 6],
        controller: &mut Controller<R>,
    ) -> Result<Self, kernel_fidl::Status> {
        let ring_handle = Memory::create(RING_BYTES, 0)?;
        let ring_vaddr = match Memory::map(ring_handle, RING_BYTES, 6) {
            Ok(value) => value,
            Err(error) => {
                let _ = Memory::close(ring_handle);
                return Err(error);
            }
        };
        if let Err(error) = Memory::commit_range(ring_vaddr, RING_BYTES) {
            let _ = Memory::unmap(ring_vaddr, RING_BYTES);
            let _ = Memory::close(ring_handle);
            return Err(error);
        }
        unsafe { core::ptr::write_bytes(ring_vaddr as *mut u8, 0, RING_BYTES as usize) };
        let (ring_dma, ring_token) = match Memory::map_dma(iommu, ring_handle, 0, RING_BYTES, 2 | 4)
        {
            Ok(value) => value,
            Err(error) => {
                let _ = Memory::unmap(ring_vaddr, RING_BYTES);
                let _ = Memory::close(ring_handle);
                return Err(error);
            }
        };
        if controller
            .program_rings(ring_dma + RX_RING_OFFSET, ring_dma + TX_RING_OFFSET)
            .is_err()
        {
            let _ = Memory::unmap_dma(ring_token);
            let _ = Memory::unmap(ring_vaddr, RING_BYTES);
            let _ = Memory::close(ring_handle);
            return Err(kernel_fidl::Status::ErrInvalidArgs);
        }
        Ok(Self {
            mac,
            next_buffer_id: 1,
            running: false,
            buffers: BTreeMap::new(),
            fifos: Vec::new(),
            rx_pending: VecDeque::new(),
            tx_pending: VecDeque::new(),
            ring_handle,
            ring_vaddr,
            ring_dma,
            ring_token,
        })
    }

    pub(super) const fn rx_dma(&self) -> u64 {
        self.ring_dma + RX_RING_OFFSET
    }

    pub(super) const fn tx_dma(&self) -> u64 {
        self.ring_dma + TX_RING_OFFSET
    }

    fn info(&self) -> EthernetInfo {
        EthernetInfo {
            mac: MacAddress { octets: self.mac },
            mtu: 1500,
            features: DeviceFeatures::DMA_64BIT,
        }
    }

    fn register_buffer(
        &mut self,
        iommu: u64,
        handle: u64,
        size: u32,
    ) -> Result<u32, Status> {
        if size == 0 || self.buffers.len() >= MAX_BUFFERS {
            return Err(if size == 0 {
                Status::ErrInvalidArgs
            } else {
                Status::ErrNoMemory
            });
        }
        let mapped = page_round(u64::from(size)).ok_or(Status::ErrInvalidArgs)?;
        let vaddr = Memory::map(handle, mapped, 6).map_err(map_kernel_status)?;
        let (dma, token) = match Memory::map_dma(iommu, handle, 0, mapped, 2 | 4) {
            Ok(value) => value,
            Err(error) => {
                let _ = Memory::unmap(vaddr, mapped);
                let _ = Memory::close(handle);
                return Err(map_kernel_status(error));
            }
        };
        let id = self.next_buffer_id;
        self.next_buffer_id = self
            .next_buffer_id
            .checked_add(1)
            .ok_or(Status::ErrNoMemory)?;
        self.buffers.insert(
            id,
            Buffer {
                handle,
                vaddr,
                dma,
                token,
                size: mapped,
            },
        );
        Ok(id)
    }

    fn unregister_buffer(&mut self, id: u32) -> Status {
        if self
            .rx_pending
            .iter()
            .chain(self.tx_pending.iter())
            .any(|pending| pending.entry.vmo_id == id)
        {
            return Status::ErrShouldWait;
        }
        let Some(buffer) = self.buffers.remove(&id) else {
            return Status::ErrInvalidArgs;
        };
        let _ = Memory::unmap_dma(buffer.token);
        let _ = Memory::unmap(buffer.vaddr, buffer.size);
        let _ = Memory::close(buffer.handle);
        Status::Ok
    }

    fn buffer_range(&self, entry: FrameEntry) -> Result<(Buffer, u64), Status> {
        let buffer = *self
            .buffers
            .get(&entry.vmo_id)
            .ok_or(Status::ErrInvalidArgs)?;
        let end = u64::from(entry.offset)
            .checked_add(u64::from(entry.length))
            .ok_or(Status::ErrInvalidArgs)?;
        if entry.length == 0 || end > buffer.size {
            return Err(Status::ErrInvalidArgs);
        }
        Ok((buffer, buffer.dma + u64::from(entry.offset)))
    }

    pub(super) fn resources(&self) -> Vec<Resource> {
        let mut resources = alloc::vec![
            Resource::Handle(self.ring_handle),
            Resource::Mapping {
                handle: self.ring_handle,
                offset: 0,
                va: self.ring_vaddr,
                size: RING_BYTES,
                rights: 6,
            },
        ];
        resources.extend(self.fifos.iter().map(|fifo| Resource::Handle(fifo.0)));
        for buffer in self.buffers.values() {
            resources.push(Resource::Handle(buffer.handle));
            resources.push(Resource::Mapping {
                handle: buffer.handle,
                offset: 0,
                va: buffer.vaddr,
                size: buffer.size,
                rights: 6,
            });
        }
        resources
    }

    pub(super) fn checkpoint(&self) -> Vec<u8> {
        let mut writer = Encoder::new();
        writer.word(1);
        writer.bytes(&self.mac);
        writer.word(self.next_buffer_id as u64);
        writer.word(self.running as u64);
        writer.word(self.ring_handle);
        writer.word(self.ring_vaddr);
        writer.word(self.ring_dma);
        writer.word(self.ring_token);
        writer.word(self.buffers.len() as u64);
        for (id, buffer) in &self.buffers {
            writer.word(*id as u64);
            for value in [
                buffer.handle,
                buffer.vaddr,
                buffer.dma,
                buffer.token,
                buffer.size,
            ] {
                writer.word(value);
            }
        }
        writer.word(self.fifos.len() as u64);
        for fifo in &self.fifos {
            writer.word(fifo.0);
        }
        encode_pending(&mut writer, &self.rx_pending);
        encode_pending(&mut writer, &self.tx_pending);
        writer.finish()
    }

    pub(super) fn adopt(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Decoder::new(bytes);
        if reader.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mac: [u8; 6] = reader
            .bytes(6)?
            .try_into()
            .map_err(|_| Error::InvalidData)?;
        let next_buffer_id = u32::try_from(reader.word()?).map_err(|_| Error::InvalidData)?;
        let running = reader.flag()?;
        let ring_handle = reader.word()?;
        let ring_vaddr = reader.word()?;
        let ring_dma = reader.word()?;
        let ring_token = reader.word()?;
        let mut buffers = BTreeMap::new();
        for _ in 0..reader.count(MAX_BUFFERS)? {
            let id = u32::try_from(reader.word()?).map_err(|_| Error::InvalidData)?;
            let buffer = Buffer {
                handle: reader.word()?,
                vaddr: reader.word()?,
                dma: reader.word()?,
                token: reader.word()?,
                size: reader.word()?,
            };
            if id == 0
                || id >= next_buffer_id
                || buffer.handle == 0
                || buffer.vaddr == 0
                || buffer.dma == 0
                || buffer.token == 0
                || buffer.size == 0
                || buffers.insert(id, buffer).is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        let mut fifos = Vec::new();
        for _ in 0..reader.count(16)? {
            let fifo = reader.word()?;
            if fifo == 0 {
                return Err(Error::InvalidData);
            }
            fifos.push(Channel(fifo));
        }
        let rx_pending = decode_pending(&mut reader)?;
        let tx_pending = decode_pending(&mut reader)?;
        reader.finish()?;
        if next_buffer_id == 0
            || ring_handle == 0
            || ring_vaddr == 0
            || ring_dma == 0
            || ring_token == 0
            || rx_pending
                .iter()
                .chain(tx_pending.iter())
                .any(|pending| !buffers.contains_key(&pending.entry.vmo_id))
        {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            mac,
            next_buffer_id,
            running,
            buffers,
            fifos,
            rx_pending,
            tx_pending,
            ring_handle,
            ring_vaddr,
            ring_dma,
            ring_token,
        })
    }
}

pub(super) fn poll_endpoints(state: &mut Runtime, source: &mut Source) {
    let mut index = 0;
    while index < state.endpoints.len() {
        match state.endpoints[index].channel.try_recv() {
            Ok(message) => {
                handle_control(state, index, message.bytes, message.handles);
                source.changed_keys([0, 1, 2]);
                index += 1;
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                state.endpoints.remove(index);
                source.changed(0);
            }
            Err(_) => index += 1,
        }
    }
}

fn handle_control(state: &mut Runtime, endpoint_index: usize, bytes: Vec<u8>, handles: Vec<u64>) {
    let (ordinal, request) = envelope(&bytes);
    let endpoint = &state.endpoints[endpoint_index];
    if !endpoint.allows(ordinal) {
        return;
    }
    let channel = endpoint.channel;
    let refs = handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    let ethernet = state.ethernet.as_mut().unwrap();
    let mut out = [0; 256];
    let mut out_handles = [HandleRef { raw: 0 }; 4];
    let encoded = match ordinal {
        1 => (DeviceGetInfoResponse {
            info: ethernet.info(),
        })
        .encode(&mut out, &mut out_handles),
        2 => {
            let result = DeviceRegisterBufferRequest::decode(request, &refs)
                .map_err(|_| Status::ErrInvalidArgs)
                .and_then(|request| {
                    ethernet.register_buffer(state.iommu, request.vmo.raw, request.size_bytes)
                });
            let (status, vmo_id) = match result {
                Ok(id) => (Status::Ok, id),
                Err(status) => (status, 0),
            };
            (DeviceRegisterBufferResponse { status, vmo_id }).encode(&mut out, &mut out_handles)
        }
        3 => {
            let status = DeviceUnregisterBufferRequest::decode(request, &refs)
                .map(|request| ethernet.unregister_buffer(request.vmo_id))
                .unwrap_or(Status::ErrInvalidArgs);
            (DeviceUnregisterBufferResponse { status }).encode(&mut out, &mut out_handles)
        }
        4 => {
            let result = Channel::pair();
            let (status, fifo_handle) = match result {
                Ok((provider, client)) => {
                    ethernet.fifos.push(provider);
                    (Status::Ok, client.0)
                }
                Err(error) => (map_kernel_status(error), 0),
            };
            (DeviceGetFifoResponse {
                status,
                fifo_handle: HandleRef { raw: fifo_handle },
            })
            .encode(&mut out, &mut out_handles)
        }
        5 => {
            ethernet.running = true;
            if let Some(controller) = &mut state.controller {
                controller.start();
            }
            (DeviceStartResponse { status: Status::Ok }).encode(&mut out, &mut out_handles)
        }
        6 => {
            ethernet.running = false;
            if let Some(controller) = &mut state.controller {
                controller.quiesce();
            }
            (DeviceStopResponse { status: Status::Ok }).encode(&mut out, &mut out_handles)
        }
        _ => return,
    };
    if let Ok(encoded) = encoded {
        let response_handles = out_handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = channel.send(&out[..encoded.bytes], &response_handles);
    }
}

pub(super) fn poll_packets(state: &mut Runtime, source: &mut Source) {
    let ethernet = state.ethernet.as_mut().unwrap();
    for fifo in ethernet.fifos.clone() {
        let Ok(message) = fifo.try_recv() else {
            continue;
        };
        let Ok(entry) = FrameEntry::decode(&message.bytes, &[]) else {
            continue;
        };
        let result = if !ethernet.running {
            Err(Status::ErrShouldWait)
        } else {
            match entry.opcode {
                FrameOpcode::TxSend => submit_tx(ethernet, state.controller.as_mut().unwrap(), fifo, entry),
                FrameOpcode::RxSupply => submit_rx(ethernet, state.controller.as_mut().unwrap(), fifo, entry),
                _ => Err(Status::ErrInvalidArgs),
            }
        };
        if let Err(status) = result {
            reply_frame(fifo, completion(entry, status));
        }
        source.changed_keys([1, 2]);
    }
    complete_tx(ethernet, state.controller.as_mut().unwrap());
    complete_rx(ethernet, state.controller.as_mut().unwrap());
    if bexos_userspace::Interrupt(state.interrupt)
        .pending()
        .unwrap_or(false)
    {
        state.controller.as_mut().unwrap().acknowledge_interrupt();
        let _ = bexos_userspace::Interrupt(state.interrupt).acknowledge();
    }
}

fn submit_tx(
    ethernet: &mut EthernetRuntime,
    controller: &mut Controller<MmioRegisters>,
    fifo: Channel,
    entry: FrameEntry,
) -> Result<(), Status> {
    if ethernet.tx_pending.len() >= MAX_PENDING {
        return Err(Status::ErrNoMemory);
    }
    let (buffer, dma) = ethernet.buffer_range(entry)?;
    let descriptor = controller
        .tx
        .submit_tx(
            dma,
            entry.length,
            DmaRange {
                base: buffer.dma,
                length: buffer.size,
            },
        )
        .map_err(map_ring_error)?;
    write_tx_descriptor(ethernet, descriptor, dma, entry.length);
    ethernet.tx_pending.push_back(PendingFrame {
        fifo,
        entry,
        descriptor,
    });
    controller
        .registers
        .write(register::TDT, controller.tx.tail() as u32);
    Ok(())
}

fn submit_rx(
    ethernet: &mut EthernetRuntime,
    controller: &mut Controller<MmioRegisters>,
    fifo: Channel,
    entry: FrameEntry,
) -> Result<(), Status> {
    if ethernet.rx_pending.len() >= MAX_PENDING {
        return Err(Status::ErrNoMemory);
    }
    let (buffer, dma) = ethernet.buffer_range(entry)?;
    let descriptor = controller
        .rx
        .post_rx(
            dma,
            entry.length,
            DmaRange {
                base: buffer.dma,
                length: buffer.size,
            },
        )
        .map_err(map_ring_error)?;
    write_rx_descriptor(ethernet, descriptor, dma);
    ethernet.rx_pending.push_back(PendingFrame {
        fifo,
        entry,
        descriptor,
    });
    controller.registers.write(register::RDT, descriptor as u32);
    Ok(())
}

fn complete_tx(ethernet: &mut EthernetRuntime, controller: &mut Controller<MmioRegisters>) {
    while let Some(pending) = ethernet.tx_pending.front().copied() {
        let raw = descriptor_address(ethernet, TX_RING_OFFSET, pending.descriptor);
        let status = unsafe { core::ptr::read_volatile((raw + 12) as *const u8) };
        if status & 1 == 0 {
            break;
        }
        if let Some(descriptor) = controller.tx.descriptor_mut(pending.descriptor) {
            descriptor.status_or_errors = status;
        }
        let completion_status = controller
            .tx
            .complete_tx()
            .map(|_| Status::Ok)
            .unwrap_or(Status::ErrInvalidArgs);
        ethernet.tx_pending.pop_front();
        reply_frame(pending.fifo, completion(pending.entry, completion_status));
    }
}

fn complete_rx(ethernet: &mut EthernetRuntime, controller: &mut Controller<MmioRegisters>) {
    while let Some(pending) = ethernet.rx_pending.front().copied() {
        let raw = descriptor_address(ethernet, RX_RING_OFFSET, pending.descriptor);
        let status = unsafe { core::ptr::read_volatile((raw + 12) as *const u8) };
        if status & 1 == 0 {
            break;
        }
        let length = unsafe { core::ptr::read_volatile((raw + 8) as *const u16) };
        let errors = unsafe { core::ptr::read_volatile((raw + 13) as *const u8) };
        let buffer = ethernet.buffers[&pending.entry.vmo_id];
        if let Some(descriptor) = controller.rx.descriptor_mut(pending.descriptor) {
            *descriptor = LegacyDescriptor {
                address: buffer.dma + u64::from(pending.entry.offset),
                length,
                command_or_status: status,
                status_or_errors: errors,
                ..LegacyDescriptor::default()
            };
        }
        let completion_status = controller
            .rx
            .complete_rx(DmaRange {
                base: buffer.dma,
                length: buffer.size,
            })
            .map(|_| Status::Ok)
            .unwrap_or(Status::ErrInvalidArgs);
        ethernet.rx_pending.pop_front();
        let mut response = completion(pending.entry, completion_status);
        response.length = length.min(pending.entry.length);
        reply_frame(pending.fifo, response);
    }
}

fn write_tx_descriptor(ethernet: &EthernetRuntime, index: usize, dma: u64, length: u16) {
    let address = descriptor_address(ethernet, TX_RING_OFFSET, index);
    unsafe {
        core::ptr::write_bytes(address as *mut u8, 0, DESCRIPTOR_BYTES as usize);
        core::ptr::write_volatile(address as *mut u64, dma.to_le());
        core::ptr::write_volatile((address + 8) as *mut u16, length.to_le());
        core::ptr::write_volatile((address + 11) as *mut u8, 0x0b);
    }
}

fn write_rx_descriptor(ethernet: &EthernetRuntime, index: usize, dma: u64) {
    let address = descriptor_address(ethernet, RX_RING_OFFSET, index);
    unsafe {
        core::ptr::write_bytes(address as *mut u8, 0, DESCRIPTOR_BYTES as usize);
        core::ptr::write_volatile(address as *mut u64, dma.to_le());
    }
}

fn descriptor_address(ethernet: &EthernetRuntime, offset: u64, index: usize) -> u64 {
    ethernet.ring_vaddr + offset + index as u64 * DESCRIPTOR_BYTES
}

fn completion(entry: FrameEntry, status: Status) -> FrameEntry {
    FrameEntry {
        opcode: match entry.opcode {
            FrameOpcode::RxSupply => FrameOpcode::RxComplete,
            _ => FrameOpcode::TxComplete,
        },
        flags: if status == Status::Ok {
            FrameFlags(0)
        } else {
            FrameFlags::TRUNCATED
        },
        ..entry
    }
}

fn reply_frame(fifo: Channel, entry: FrameEntry) {
    let mut bytes = [0; 64];
    if let Ok(encoded) = entry.encode(&mut bytes, &mut []) {
        let _ = fifo.send(&bytes[..encoded.bytes], &[]);
    }
}

fn encode_pending(writer: &mut Encoder, pending: &VecDeque<PendingFrame>) {
    writer.word(pending.len() as u64);
    for pending in pending {
        writer.word(pending.fifo.0);
        encode_frame(writer, pending.entry);
        writer.word(pending.descriptor as u64);
    }
}

fn decode_pending(reader: &mut Decoder<'_>) -> Result<VecDeque<PendingFrame>, Error> {
    let mut pending = VecDeque::new();
    for _ in 0..reader.count(MAX_PENDING)? {
        pending.push_back(PendingFrame {
            fifo: Channel(reader.word()?),
            entry: decode_frame(reader)?,
            descriptor: reader.count(4096)?,
        });
    }
    Ok(pending)
}

fn encode_frame(writer: &mut Encoder, entry: FrameEntry) {
    writer.word(entry.req_id);
    writer.word(entry.opcode as u64);
    writer.word(entry.vmo_id as u64);
    writer.word(entry.offset as u64);
    writer.word(entry.length as u64);
    writer.word(entry.flags.0 as u64);
}

fn decode_frame(reader: &mut Decoder<'_>) -> Result<FrameEntry, Error> {
    Ok(FrameEntry {
        req_id: reader.word()?,
        opcode: match reader.word()? {
            1 => FrameOpcode::RxSupply,
            2 => FrameOpcode::RxComplete,
            3 => FrameOpcode::TxSend,
            4 => FrameOpcode::TxComplete,
            _ => return Err(Error::InvalidData),
        },
        vmo_id: u32::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
        offset: u32::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
        length: u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
        flags: FrameFlags(u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?),
    })
}

fn page_round(value: u64) -> Option<u64> {
    value.checked_add(PAGE_SIZE - 1).map(|value| value & !(PAGE_SIZE - 1))
}

fn map_kernel_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrInvalidHandle => Status::ErrInvalidHandle,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        kernel_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        _ => Status::ErrInvalidArgs,
    }
}

fn map_ring_error(error: bexos_intel_nic::RingError) -> Status {
    match error {
        bexos_intel_nic::RingError::Full => Status::ErrShouldWait,
        _ => Status::ErrInvalidArgs,
    }
}
