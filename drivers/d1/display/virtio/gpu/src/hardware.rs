use bexos_graphics::{Damage, Format, Surface};
use bexos_graphics_runtime::Mapping;
use bexos_virtio_hal::{BexHal, buffer::DmaBuffer};
use kernel_fidl::Status;
use virtio_drivers::transport::{
    DeviceStatus, Transport,
    pci::{
        PciTransport,
        bus::{Cam, DeviceFunction, MmioCam, PciRoot},
    },
};
const ECAM_SIZE: u64 = 0x0100_0000;
pub struct Hardware {
    pub aperture: bexos_virtio_gpu_protocol::aperture::Aperture,
    pub transport: PciTransport,
    pub ecam: Mapping,
    pub node: u64,
    pub queue: DmaBuffer,
    pub command: DmaBuffer,
    pub frames: [DmaBuffer; 2],
    pub surface: Surface,
    pub index: u16,
    pub front: u32,
    pub scanout_resource: u32,
    pub fence: u64,
    pub healthy: bool,
    pub stale: [Option<Damage>; 2],
    pub capabilities: bexos_virtio_gpu_protocol::Capabilities,
    pub(crate) inflight: Option<crate::command::PendingCommand>,
    pub(crate) presenting: Option<crate::present::PresentFlight>,
}
pub(crate) fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
pub fn transport(node: u64, ecam: u64) -> Result<PciTransport, Status> {
    let mut root = PciRoot::new(unsafe { MmioCam::new(ecam as *mut u8, Cam::Ecam) });
    PciTransport::new::<BexHal, _>(
        &mut root,
        DeviceFunction {
            bus: ((node >> 16) & 255) as u8,
            device: ((node >> 8) & 255) as u8,
            function: (node & 255) as u8,
        },
    )
    .map_err(|_| Status::ErrInvalidArgs)
}
impl Hardware {
    pub fn new(node: u64) -> Result<Self, Status> {
        let h = bexos_userspace::Memory::physical(
            bexos_userspace::syscall::pci_ecam_base(),
            ECAM_SIZE,
        )?;
        let ecam = Mapping::map(h, ECAM_SIZE, 6)?;
        bexos_userspace::log("virtio-gpu: ECAM mapped\n");
        let aperture = crate::aperture::discover(node, ecam.address)?;
        let mut transport = transport(node, ecam.address)?;
        bexos_userspace::log("virtio-gpu: PCI transport ready\n");
        transport.set_status(DeviceStatus::empty());
        transport.set_status(DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER);
        if transport.read_device_features() & (1 << 32) == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let features = bexos_virtio_gpu_protocol::transport::negotiated_features(
            transport.read_device_features(),
        );
        transport.write_driver_features(features);
        transport.set_status(
            DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER | DeviceStatus::FEATURES_OK,
        );
        if !transport.get_status().contains(DeviceStatus::FEATURES_OK)
            || transport.max_queue_size(0) < 2
        {
            return Err(Status::ErrInvalidArgs);
        }
        let queue = DmaBuffer::new(3).ok_or(Status::ErrNoMemory)?;
        // Allocate every backing before publishing a queue address to the device.
        let command = DmaBuffer::new(2).ok_or(Status::ErrNoMemory)?;
        let pages = (800 * 600 * 4 + 4095) / 4096;
        let frames = [
            DmaBuffer::new(pages).ok_or(Status::ErrNoMemory)?,
            DmaBuffer::new(pages).ok_or(Status::ErrNoMemory)?,
        ];
        let q = queue.snapshot()[0];
        transport.queue_set(0, 2, q, q + 4096, q + 8192);
        bexos_userspace::log("virtio-gpu: queue ready\n");
        transport.finish_init();
        // Safe boot fallback until the primary display's extent is discovered.
        let surface = Surface {
            width: 800,
            height: 600,
            stride: 3200,
            format: Format::Bgra,
        };
        let mut this = Self {
            aperture,
            transport,
            ecam,
            node,
            queue,
            command,
            frames,
            surface,
            index: 0,
            front: 0,
            scanout_resource: 1,
            fence: 0,
            healthy: true,
            stale: [Some(surface.full()); 2],
            capabilities: Default::default(),
            inflight: None,
            presenting: None,
        };
        if let Err(error) = this.discover() {
            this.healthy = false;
            bexos_userspace::log(&format!(
                "virtio-gpu: discovery failed {error:?}; retaining DMA\n"
            ));
            return Ok(this);
        }
        if let Some((width, height)) = this.capabilities.primary_extent() {
            if (width, height) != (surface.width, surface.height) {
                let pages = (width as usize * height as usize * 4).div_ceil(4096);
                // These frame addresses have not been published to the device.
                // Allocation failure keeps the bounded boot buffers operational.
                if let (Some(a), Some(b)) = (DmaBuffer::new(pages), DmaBuffer::new(pages)) {
                    this.frames = [a, b];
                    this.surface = Surface {
                        width,
                        height,
                        stride: width * 4,
                        format: Format::Bgra,
                    };
                    this.stale = [Some(this.surface.full()); 2];
                } else {
                    bexos_userspace::log(
                        "virtio-gpu: mode allocation failed; retaining 800x600 fallback\n",
                    );
                }
            }
        }
        bexos_userspace::log("virtio-gpu: buffers allocated\n");
        for id in 1..=2 {
            let mut args = [0u8; 16];
            put32(&mut args, 0, id);
            put32(&mut args, 4, 1);
            put32(&mut args, 8, this.surface.width);
            put32(&mut args, 12, this.surface.height);
            if let Err(error) = this.request(0x101, &args, 0x1100) {
                this.healthy = false;
                bexos_userspace::log(&format!(
                    "virtio-gpu: resource setup failed {error:?}; retaining DMA\n"
                ));
                return Ok(this);
            }
            let mut args = [0u8; 24];
            put32(&mut args, 0, id);
            put32(&mut args, 4, 1);
            put64(&mut args, 8, this.frames[id as usize - 1].snapshot()[0]);
            put32(&mut args, 16, this.surface.stride * this.surface.height);
            if let Err(error) = this.request(0x106, &args, 0x1100) {
                this.healthy = false;
                bexos_userspace::log(&format!(
                    "virtio-gpu: backing setup failed {error:?}; retaining DMA\n"
                ));
                return Ok(this);
            }
        }
        Ok(this)
    }
    fn request(&mut self, command: u32, payload: &[u8], expected: u32) -> Result<(), Status> {
        self.request_response(command, payload, expected)
            .map(|_| ())
    }
    pub fn matches_front(&mut self, bytes: &[u8], surface: Surface) -> bool {
        surface == self.surface
            && bytes
                == unsafe {
                    &self.frames[self.front as usize].bytes()
                        [..(surface.stride * surface.height) as usize]
                }
    }
    pub fn snapshot(&mut self) -> Result<Mapping, Status> {
        let len = (self.surface.stride * self.surface.height) as usize;
        let mut m = Mapping::new(len as u64)?;
        m.bytes_mut()
            .copy_from_slice(unsafe { &self.frames[self.front as usize].bytes()[..len] });
        Ok(m)
    }
}
