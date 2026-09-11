use crate::transport::Console;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{Cam, DeviceFunction, MmioCam, PciRoot};

use alloc::vec::Vec;
use bexos_migration::Error;
use bexos_migration::codec::{Decoder, Encoder};
use bexos_virtio_hal::BexHal;

const QEMU_VIRT_ECAM_SIZE: u64 = 0x0100_0000;

pub struct Hardware {
    pub console: Console,
    pub healthy: bool,
    ecam_handle: u64,
    ecam_vaddr: u64,
    owns_ecam: bool,
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
        let transport =
            PciTransport::new::<BexHal, _>(&mut root, device_function).map_err(|error| {
                bexos_userspace::log(&alloc::format!(
                    "virtio-console: PCI transport failed: {error:?}\n"
                ));
                kernel_fidl::Status::ErrInvalidArgs
            })?;
        let console = Console::new(transport)?;
        Ok(Self {
            console,
            healthy: true,
            ecam_handle,
            ecam_vaddr,
            owns_ecam: true,
        })
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.ecam_handle);
        w.word(self.ecam_vaddr);
        w.word(self.healthy as u64);
        w.bytes(&self.console.snapshot());
        w.finish()
    }

    pub fn adopt(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let ecam_handle = r.word()?;
        let ecam_vaddr = r.word()?;
        let healthy = r.flag()?;
        let console = unsafe { Console::adopt(r.bytes(65536)?) }.map_err(|_| Error::InvalidData)?;
        r.finish()?;
        Ok(Self {
            console,
            healthy,
            ecam_handle,
            ecam_vaddr,
            owns_ecam: false,
        })
    }

    pub fn ready_to_migrate(&self) -> bool {
        // Queue tokens, buffer cursors and DMA mappings are retained. A byte
        // stream can have pending TX or partially consumed RX at any boundary;
        // requiring an idle stream would reject migration under debug traffic.
        self.healthy && self.console.ready()
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
        self.console.activate();
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
