//! Explicit split-ring state; migration never serializes Rust queue objects.
use bexos_flatland_input::virtio::RawEvent;
use bexos_graphics_runtime::Mapping;
use bexos_virtio_hal::{BexHal, buffer::DmaBuffer};
use core::sync::atomic::{Ordering, fence};
use kernel_fidl::Status;
use virtio_drivers::transport::{
    DeviceStatus, Transport,
    pci::{
        PciTransport,
        bus::{Cam, DeviceFunction, MmioCam, PciRoot},
    },
};
pub const QUEUE_SIZE: u16 = 64;
pub const ECAM_SIZE: u64 = 0x0100_0000;
pub struct Hardware {
    pub transport: PciTransport,
    pub ecam: Mapping,
    pub node: u64,
    pub queues: [DmaBuffer; 2],
    pub reports: DmaBuffer,
    pub available: u16,
    pub used: u16,
    pub healthy: bool,
    pub axes: [i32; 8],
}
pub fn transport(node: u64, address: u64) -> Result<PciTransport, Status> {
    let mut root = PciRoot::new(unsafe { MmioCam::new(address as *mut u8, Cam::Ecam) });
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
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
impl Hardware {
    /// Stop DMA before appd may revoke the device's mappings. Migration does
    /// not call this: the replacement adopts the live queues and DMA pins.
    pub fn stop(&mut self) -> bool {
        self.healthy = false;
        self.transport.set_status(DeviceStatus::empty());
        for _ in 0..1024 {
            if self.transport.get_status().is_empty() {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
    pub fn new(node: u64) -> Result<Self, Status> {
        let ecam = Mapping::map(
            bexos_userspace::Memory::physical(
                bexos_userspace::syscall::pci_ecam_base(),
                ECAM_SIZE,
            )?,
            ECAM_SIZE,
            6,
        )?;
        let mut transport = transport(node, ecam.address)?;
        transport.set_status(DeviceStatus::empty());
        transport.set_status(DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER);
        let offered = transport.read_device_features();
        if offered & (1 << 32) == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        transport.write_driver_features(offered & ((1 << 32) | (1 << 33)));
        transport.set_status(
            DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER | DeviceStatus::FEATURES_OK,
        );
        if !transport.get_status().contains(DeviceStatus::FEATURES_OK)
            || transport.max_queue_size(0) < QUEUE_SIZE as u32
            || transport.max_queue_size(1) < 2
        {
            return Err(Status::ErrInvalidArgs);
        }
        // All allocations precede publication of DMA addresses.
        let mut queues = [
            DmaBuffer::new(3).ok_or(Status::ErrNoMemory)?,
            DmaBuffer::new(3).ok_or(Status::ErrNoMemory)?,
        ];
        let reports = DmaBuffer::new(1).ok_or(Status::ErrNoMemory)?;
        let report_pa = reports.snapshot()[0];
        let q = unsafe { queues[0].bytes() };
        q.fill(0);
        for id in 0..QUEUE_SIZE {
            let d = id as usize * 16;
            put64(q, d, report_pa + id as u64 * 8);
            put32(q, d + 8, 8);
            put16(q, d + 12, 2);
            put16(q, 4100 + id as usize * 2, id);
        }
        put16(q, 4096, 1);
        put16(q, 4098, QUEUE_SIZE);
        unsafe { queues[1].bytes() }.fill(0);
        let mut axes = [0; 8];
        for (i, axis) in [0u8, 1, 0x35, 0x36].into_iter().enumerate() {
            if transport.write_config_space(0, 0x12u8).is_ok()
                && transport.write_config_space(1, axis).is_ok()
                && transport.read_config_space::<u8>(2).unwrap_or(0) >= 20
            {
                axes[i * 2] = transport.read_config_space::<u32>(8).unwrap_or(0) as i32;
                axes[i * 2 + 1] = transport.read_config_space::<u32>(12).unwrap_or(0) as i32;
            }
        }
        fence(Ordering::SeqCst);
        for (i, q) in queues.iter().enumerate() {
            let a = q.snapshot()[0];
            transport.queue_set(
                i as u16,
                if i == 0 { QUEUE_SIZE as u32 } else { 2 },
                a,
                a + 4096,
                a + 8192,
            );
        }
        transport.finish_init();
        transport.notify(0);
        Ok(Self {
            transport,
            ecam,
            node,
            queues,
            reports,
            available: QUEUE_SIZE,
            used: 0,
            healthy: true,
            axes,
        })
    }
    pub fn drain(&mut self, out: &mut [RawEvent; 64]) -> Result<usize, Status> {
        if !self.healthy {
            return Err(Status::ErrInvalidArgs);
        }
        let q = unsafe { self.queues[0].bytes() };
        let reports = unsafe { self.reports.bytes() };
        let batch = match crate::report_queue::read(q, reports, self.used, out) {
            Ok(batch) => batch,
            Err(_) => {
                self.healthy = false;
                return Err(Status::ErrInvalidArgs);
            }
        };
        let count = batch.count;
        for id in batch.ids.into_iter().take(count) {
            put16(q, 4100 + (self.available as usize % 64) * 2, id);
            self.available = self.available.wrapping_add(1);
            self.used = self.used.wrapping_add(1);
        }
        if count != 0 {
            fence(Ordering::SeqCst);
            unsafe {
                core::ptr::write_volatile(
                    q.as_mut_ptr().add(4098).cast::<u16>(),
                    self.available.to_le(),
                )
            };
            fence(Ordering::SeqCst);
            self.transport.notify(0);
        }
        self.transport.ack_interrupt();
        Ok(count)
    }
}
