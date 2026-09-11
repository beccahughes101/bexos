//! One named virtio-serial port, with nonblocking queues and retained DMA.
use bexos_virtio_hal::BexHal;
use core::ptr::NonNull;
use kernel_fidl::Status;
use virtio_drivers::queue::VirtQueue;
use virtio_drivers::transport::{Transport, pci::PciTransport};
use virtio_drivers::{BufferDirection, Hal};

const QSIZE: usize = 8;
const CONTROL_BYTES: usize = 512;
const PORT: u32 = 1;
const RX_QUEUE: u16 = 4;
const TX_QUEUE: u16 = 5;
const MULTIPORT: u64 = 1 << 1;
const VERSION_1: u64 = 1 << 32;
const ACCESS_PLATFORM: u64 = 1 << 33;

struct Buffer {
    ptr: NonNull<u8>,
    pa: u64,
    pages: usize,
    owned: bool,
}
impl Buffer {
    fn new(pages: usize) -> Result<Self, Status> {
        let (pa, ptr) = BexHal::dma_alloc(pages, BufferDirection::Both);
        if pa == 0 {
            return Err(Status::ErrAccessDenied);
        }
        Ok(Self {
            ptr,
            pa,
            pages,
            owned: true,
        })
    }
    unsafe fn bytes(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.pages * 4096) }
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        if self.owned {
            unsafe {
                BexHal::dma_dealloc(self.pa, self.ptr, self.pages);
            }
        }
    }
}

pub struct Console {
    transport: PciTransport,
    queues: [VirtQueue<BexHal, QSIZE>; 4],
    rx: Buffer,
    tx: Buffer,
    control_rx: Buffer,
    control_tx: Buffer,
    rx_token: u16,
    ctrl_tokens: [u16; QSIZE],
    tx_token: Option<u16>,
    tx_len: usize,
    cursor: usize,
    available: usize,
    port_kind: u8,
    port_open: bool,
    owned: bool,
}

impl Console {
    pub fn new(mut transport: PciTransport) -> Result<Self, Status> {
        use virtio_drivers::transport::DeviceStatus;
        transport.set_status(DeviceStatus::empty());
        transport.set_status(DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER);
        let features = transport.read_device_features() & (MULTIPORT | VERSION_1 | ACCESS_PLATFORM);
        if features & (MULTIPORT | VERSION_1) != MULTIPORT | VERSION_1 {
            return Err(Status::ErrInvalidArgs);
        }
        transport.write_driver_features(features);
        transport.set_status(
            DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER | DeviceStatus::FEATURES_OK,
        );
        if !transport.get_status().contains(DeviceStatus::FEATURES_OK) {
            return Err(Status::ErrInvalidArgs);
        }
        transport.set_guest_page_size(4096);
        let mut queues = [
            VirtQueue::new(&mut transport, RX_QUEUE, false, false).map_err(error)?,
            VirtQueue::new(&mut transport, TX_QUEUE, false, false).map_err(error)?,
            VirtQueue::new(&mut transport, 2, false, false).map_err(error)?,
            VirtQueue::new(&mut transport, 3, false, false).map_err(error)?,
        ];
        for queue in &mut queues {
            queue.set_dev_notify(false);
        }
        let mut rx = Buffer::new(1)?;
        let mut control_rx = Buffer::new(1)?;
        let rx_token = unsafe { queues[0].add(&[], &mut [rx.bytes()]) }.map_err(error)?;
        let mut ctrl_tokens = [0; QSIZE];
        for (index, token) in ctrl_tokens.iter_mut().enumerate() {
            let bytes = unsafe { control_rx.bytes() };
            *token = unsafe {
                queues[2].add(
                    &[],
                    &mut [&mut bytes[index * CONTROL_BYTES..(index + 1) * CONTROL_BYTES]],
                )
            }
            .map_err(error)?;
        }
        transport.finish_init();
        transport.notify(RX_QUEUE);
        transport.notify(2);
        let mut this = Self {
            transport,
            queues,
            rx,
            control_rx,
            tx: Buffer::new(2)?,
            control_tx: Buffer::new(1)?,
            rx_token,
            ctrl_tokens,
            tx_token: None,
            tx_len: 0,
            cursor: 0,
            available: 0,
            port_kind: 0,
            port_open: false,
            owned: true,
        };
        this.send_control(0, 0, 1)?; // DEVICE_READY
        Ok(this)
    }

    fn send_control(&mut self, port: u32, event: u16, value: u16) -> Result<(), Status> {
        let bytes = unsafe { self.control_tx.bytes() };
        bytes[..4].copy_from_slice(&port.to_le_bytes());
        bytes[4..6].copy_from_slice(&event.to_le_bytes());
        bytes[6..8].copy_from_slice(&value.to_le_bytes());
        let token = unsafe { self.queues[3].add(&[&bytes[..8]], &mut []) }.map_err(error)?;
        self.transport.notify(3);
        let deadline = deadline();
        while self.queues[3].peek_used() != Some(token) {
            if expired(deadline) {
                return Err(Status::ErrTimedOut);
            }
            bexos_userspace::yield_now();
        }
        unsafe { self.queues[3].pop_used(token, &[&bytes[..8]], &mut []) }.map_err(error)?;
        Ok(())
    }

    pub fn poll_control(&mut self) -> Result<(), Status> {
        self.transport.ack_interrupt();
        let Some(token) = self.queues[2].peek_used() else {
            return Ok(());
        };
        let index = self
            .ctrl_tokens
            .iter()
            .position(|value| *value == token)
            .ok_or(Status::ErrInvalidArgs)?;
        let bytes = unsafe { self.control_rx.bytes() };
        let slot = &mut bytes[index * CONTROL_BYTES..(index + 1) * CONTROL_BYTES];
        let len = unsafe { self.queues[2].pop_used(token, &[], &mut [&mut *slot]) }
            .map_err(error)? as usize;
        if !(8..=CONTROL_BYTES).contains(&len) {
            return Err(Status::ErrInvalidArgs);
        }
        let mut message = [0; CONTROL_BYTES];
        message[..len].copy_from_slice(&slot[..len]);
        // PORT_READY may immediately produce both NAME and OPEN. Keep receive
        // descriptors posted before acknowledging control events.
        self.ctrl_tokens[index] = unsafe { self.queues[2].add(&[], &mut [slot]) }.map_err(error)?;
        self.transport.notify(2);
        let bytes = &message;
        let port = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        let event = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        let value = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        bexos_userspace::log(&alloc::format!(
            "virtio-console: port={port} event={event} value={value}\n"
        ));
        if port != PORT {
            return Err(Status::ErrInvalidArgs);
        }
        match event {
            1 => {
                self.send_control(port, 3, 1)?;
            } // PORT_ADD -> PORT_READY
            2 => {
                self.port_open = false;
                return Err(Status::ErrPeerClosed);
            }
            6 => {
                self.port_open = value == 1;
            }
            7 => {
                let name = bytes[8..len].strip_suffix(&[0]).unwrap_or(&bytes[8..len]);
                let kind = port_kind(name).ok_or(Status::ErrAccessDenied)?;
                if self.port_kind != 0 && self.port_kind != kind {
                    return Err(Status::ErrAccessDenied);
                }
                self.port_kind = kind;
                self.send_control(port, 6, 1)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn ready(&self) -> bool {
        matches!(self.port_kind, 1 | 2) && self.port_open
    }

    pub fn debug_port(&self) -> bool {
        self.port_kind == 2
    }

    pub fn port_name(&self) -> &'static [u8] {
        match self.port_kind {
            1 => b"rpmb0",
            2 => b"debug0",
            _ => b"",
        }
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<(), Status> {
        if !self.ready() {
            return Err(Status::ErrPeerClosed);
        }
        if self.tx_token.is_some() || bytes.is_empty() || bytes.len() > 4100 {
            return Err(Status::ErrInvalidArgs);
        }
        let buffer = unsafe { self.tx.bytes() };
        buffer[..bytes.len()].copy_from_slice(bytes);
        self.tx_len = bytes.len();
        self.tx_token =
            Some(unsafe { self.queues[1].add(&[&buffer[..bytes.len()]], &mut []) }.map_err(error)?);
        self.transport.notify(TX_QUEUE);
        Ok(())
    }

    pub fn poll_send(&mut self) -> Result<bool, Status> {
        let Some(token) = self.tx_token else {
            return Ok(true);
        };
        if self.queues[1].peek_used() != Some(token) {
            return Ok(false);
        }
        let buffer = unsafe { self.tx.bytes() };
        unsafe { self.queues[1].pop_used(token, &[&buffer[..self.tx_len]], &mut []) }
            .map_err(error)?;
        self.tx_token = None;
        Ok(true)
    }

    pub fn receive(&mut self, out: &mut [u8]) -> Result<usize, Status> {
        if self.cursor == self.available {
            if self.queues[0].peek_used() != Some(self.rx_token) {
                return Ok(0);
            }
            self.available =
                unsafe { self.queues[0].pop_used(self.rx_token, &[], &mut [self.rx.bytes()]) }
                    .map_err(error)? as usize;
            if self.available > 4096 {
                return Err(Status::ErrInvalidArgs);
            }
            self.cursor = 0;
        }
        let n = out.len().min(self.available - self.cursor);
        let bytes = unsafe { self.rx.bytes() };
        out[..n].copy_from_slice(&bytes[self.cursor..self.cursor + n]);
        self.cursor += n;
        if self.cursor == self.available {
            self.rx_token =
                unsafe { self.queues[0].add(&[], &mut [self.rx.bytes()]) }.map_err(error)?;
            self.transport.notify(RX_QUEUE);
        }
        Ok(n)
    }

    pub fn tx_pending(&self) -> bool {
        self.tx_token.is_some()
    }

    pub fn snapshot(&self) -> alloc::vec::Vec<u8> {
        // Same-build, kernel-authorized heart transplant preserves every DMA
        // mapping. This opaque snapshot never crosses an untrusted boundary.
        let mut snapshot = alloc::vec::Vec::with_capacity(8 + core::mem::size_of::<Self>());
        snapshot.extend_from_slice(&2u64.to_le_bytes());
        snapshot.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                core::mem::size_of::<Self>(),
            )
        });
        snapshot
    }

    pub unsafe fn adopt(bytes: &[u8]) -> Result<Self, Status> {
        if bytes.len() != 8 + core::mem::size_of::<Self>() || bytes[..8] != 2u64.to_le_bytes() {
            return Err(Status::ErrInvalidArgs);
        }
        let bytes = &bytes[8..];
        let mut value = core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), value.as_mut_ptr().cast(), bytes.len());
        }
        let mut value = unsafe { value.assume_init() };
        value.owned = false;
        value.transport.disarm_migration_drop();
        for queue in &mut value.queues {
            queue.disarm_migration_drop();
        }
        for buffer in [
            &mut value.rx,
            &mut value.tx,
            &mut value.control_rx,
            &mut value.control_tx,
        ] {
            buffer.owned = false;
        }
        Ok(value)
    }

    pub fn activate(&mut self) {
        self.owned = true;
        self.transport.activate_migration_owner();
        for queue in &mut self.queues {
            queue.activate_migration_owner();
        }
        for buffer in [
            &mut self.rx,
            &mut self.tx,
            &mut self.control_rx,
            &mut self.control_tx,
        ] {
            buffer.owned = true;
        }
    }
}
fn port_kind(name: &[u8]) -> Option<u8> {
    match name {
        b"rpmb0" => Some(1),
        b"debug0" => Some(2),
        _ => None,
    }
}
#[cfg(test)]
mod role_tests {
    use super::*;
    #[test]
    fn only_exact_supported_port_names_select_a_protocol() {
        assert_eq!(port_kind(b"rpmb0"), Some(1));
        assert_eq!(port_kind(b"debug0"), Some(2));
        for name in [b"".as_slice(), b"rpmb", b"rpmb0-extra", b"debug0\0junk"] {
            assert_eq!(port_kind(name), None);
        }
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        if self.owned {
            for queue in [RX_QUEUE, TX_QUEUE, 2, 3] {
                self.transport.queue_unset(queue);
            }
        }
    }
}
fn error(_: virtio_drivers::Error) -> Status {
    Status::ErrInvalidArgs
}
pub fn deadline() -> u64 {
    bexos_userspace::syscall::ticks().saturating_add(bexos_userspace::syscall::frequency() * 30)
}
pub fn expired(end: u64) -> bool {
    bexos_userspace::syscall::ticks() >= end
}
