use virtio_drivers::device::net::VirtIONetRaw;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{Cam, DeviceFunction, MmioCam, PciRoot};

use crate::hal::BexHal;
use alloc::vec::Vec;
use bexos_migration::Error;
use bexos_migration::codec::{Decoder, Encoder};
use bexos_virtio_hal::buffer::DmaBuffer;

const QUEUE_SIZE: usize = 16;
const QEMU_VIRT_ECAM_SIZE: u64 = 0x0100_0000;

pub struct Hardware {
    nic: VirtIONetRaw<BexHal, PciTransport, QUEUE_SIZE>,
    healthy: bool,
    ecam_handle: u64,
    ecam_vaddr: u64,
    owns_ecam: bool,
    rx: DmaBuffer,
    tx: DmaBuffer,
    rx_token: Option<u16>,
    tx_token: Option<u16>,
    tx_len: usize,
}

impl Hardware {
    pub fn connect(node_id: u64) -> Result<Self, kernel_fidl::Status> {
        let ecam_handle = bexos_userspace::Memory::physical(
            bexos_userspace::syscall::pci_ecam_base(),
            QEMU_VIRT_ECAM_SIZE,
        )
        .map_err(|_| kernel_fidl::Status::ErrAccessDenied)?;
        let ecam_vaddr = bexos_userspace::Memory::map(ecam_handle, QEMU_VIRT_ECAM_SIZE, 6)
            .map_err(|_| kernel_fidl::Status::ErrAccessDenied)?;
        let device_function = DeviceFunction {
            bus: ((node_id >> 16) & 0xff) as u8,
            device: ((node_id >> 8) & 0xff) as u8,
            function: (node_id & 0xff) as u8,
        };
        let mut root = PciRoot::new(unsafe { MmioCam::new(ecam_vaddr as *mut u8, Cam::Ecam) });
        let transport = PciTransport::new::<BexHal, _>(&mut root, device_function)
            .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
        let nic = VirtIONetRaw::<BexHal, _, QUEUE_SIZE>::new(transport)
            .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
        Ok(Self {
            nic,
            healthy: true,
            ecam_handle,
            ecam_vaddr,
            owns_ecam: true,
            rx: DmaBuffer::new(1).ok_or(kernel_fidl::Status::ErrNoMemory)?,
            tx: DmaBuffer::new(1).ok_or(kernel_fidl::Status::ErrNoMemory)?,
            rx_token: None,
            tx_token: None,
            tx_len: 0,
        })
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.nic.mac_address()
    }

    pub fn start(&mut self) {
        self.healthy = true;
        self.nic.disable_interrupts();
    }

    pub fn stop(&mut self) {
        self.nic.disable_interrupts();
        self.healthy = false;
    }

    pub async fn power_up(&mut self) -> Result<(), kernel_fidl::Status> {
        self.healthy = true;
        Ok(())
    }

    pub async fn power_down(&mut self) -> Result<(), kernel_fidl::Status> {
        self.stop();
        Ok(())
    }

    pub fn begin_transmit(&mut self, bytes: &[u8]) -> Result<(), kernel_fidl::Status> {
        if !self.healthy || self.tx_token.is_some() || bytes.len() > 2048 {
            return Err(kernel_fidl::Status::ErrInvalidArgs);
        }
        let frame = unsafe { self.tx.bytes() };
        let header = self
            .nic
            .fill_buffer_header(frame)
            .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
        self.tx_len = header + bytes.len();
        frame[header..self.tx_len].copy_from_slice(bytes);
        self.tx_token = Some(
            unsafe { self.nic.transmit_begin(&frame[..self.tx_len]) }
                .map_err(|_| kernel_fidl::Status::ErrTimedOut)?,
        );
        Ok(())
    }

    pub fn poll_transmit(&mut self) -> Result<bool, kernel_fidl::Status> {
        let Some(token) = self.tx_token else {
            return Ok(false);
        };
        if self.nic.poll_transmit() != Some(token) {
            return Ok(false);
        }
        unsafe {
            self.nic
                .transmit_complete(token, &self.tx.bytes()[..self.tx_len])
        }
        .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
        self.tx_token = None;
        Ok(true)
    }

    pub fn try_receive(&mut self, bytes: &mut [u8]) -> Result<Option<usize>, kernel_fidl::Status> {
        if !self.healthy {
            return Err(kernel_fidl::Status::ErrInvalidArgs);
        }
        if self.rx_token.is_none() {
            self.rx_token = Some(
                unsafe { self.nic.receive_begin(self.rx.bytes()) }
                    .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?,
            );
        }
        let token = self.rx_token.unwrap();
        if self.nic.poll_receive() != Some(token) {
            return Ok(None);
        }
        let frame = unsafe { self.rx.bytes() };
        let (header, packet) = unsafe { self.nic.receive_complete(token, frame) }
            .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
        self.rx_token = None;
        if packet > bytes.len() || header + packet > frame.len() {
            return Err(kernel_fidl::Status::ErrBufferTooSmall);
        }
        bytes[..packet].copy_from_slice(&frame[header..header + packet]);
        Ok(Some(packet))
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(2);
        w.word(self.ecam_handle);
        w.word(self.ecam_vaddr);
        w.word(self.healthy as u64);
        w.bytes(&self.nic.migration_snapshot());
        for word in self.rx.snapshot().into_iter().chain(self.tx.snapshot()) {
            w.word(word);
        }
        w.word(self.rx_token.map_or(0, |token| token as u64 + 1));
        w.word(self.tx_token.map_or(0, |token| token as u64 + 1));
        w.word(self.tx_len as u64);
        w.finish()
    }

    pub fn adopt(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 2 {
            return Err(Error::UnsupportedVersion);
        }
        let ecam_handle = r.word()?;
        let ecam_vaddr = r.word()?;
        let healthy = r.flag()?;
        let nic = unsafe {
            VirtIONetRaw::<BexHal, PciTransport, QUEUE_SIZE>::adopt_migration_snapshot(
                r.bytes(65536)?,
            )
        }
        .map_err(|_| Error::InvalidData)?;
        let rx = DmaBuffer::adopt([r.word()?, r.word()?, r.word()?]).ok_or(Error::InvalidData)?;
        let tx = DmaBuffer::adopt([r.word()?, r.word()?, r.word()?]).ok_or(Error::InvalidData)?;
        let mut token = || -> Result<Option<u16>, Error> {
            let value = r.word()?;
            if value > QUEUE_SIZE as u64 {
                return Err(Error::InvalidData);
            }
            Ok((value != 0).then(|| (value - 1) as u16))
        };
        let rx_token = token()?;
        let tx_token = token()?;
        let tx_len = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        if tx_len > 4096 {
            return Err(Error::InvalidData);
        }
        r.finish()?;
        Ok(Self {
            nic,
            healthy,
            ecam_handle,
            ecam_vaddr,
            owns_ecam: false,
            rx,
            tx,
            rx_token,
            tx_token,
            tx_len,
        })
    }

    pub fn ready_to_migrate(&self) -> bool {
        self.healthy
    }

    pub fn resources(&self) -> [bexos_userspace::live_migration::Resource; 2] {
        [
            bexos_userspace::live_migration::Resource::Handle(self.ecam_handle),
            bexos_userspace::live_migration::Resource::Mapping {
                handle: self.ecam_handle,
                offset: 0,
                va: self.ecam_vaddr,
                size: QEMU_VIRT_ECAM_SIZE,
                rights: 6,
            },
        ]
    }

    pub fn activate(&mut self) {
        self.owns_ecam = true;
        self.nic.activate_migration_owner();
        self.rx.activate();
        self.tx.activate();
    }
}

impl Drop for Hardware {
    fn drop(&mut self) {
        if !self.owns_ecam {
            return;
        }
        let _ = bexos_userspace::Memory::unmap(self.ecam_vaddr, QEMU_VIRT_ECAM_SIZE);
        let _ = bexos_userspace::Memory::close(self.ecam_handle);
    }
}
