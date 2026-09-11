//! Read the VirtIO host-visible shared-memory capability before queue setup.
use bexos_virtio_gpu_protocol::aperture::Aperture;
use kernel_fidl::Status;
use virtio_drivers::transport::pci::bus::{
    BarInfo, Cam, ConfigurationAccess, DeviceFunction, MmioCam, PciRoot,
};

pub fn discover(node: u64, ecam: u64) -> Result<Aperture, Status> {
    let cam = unsafe { MmioCam::new(ecam as *mut u8, Cam::Ecam) };
    // The cloned accessor reads only immutable capability descriptors.
    let descriptors = unsafe { cam.unsafe_clone() };
    let mut root = PciRoot::new(cam);
    let device = DeviceFunction {
        bus: (node >> 16) as u8,
        device: (node >> 8) as u8,
        function: node as u8,
    };
    let mut shared = None;
    for cap in root.capabilities(device).take(48) {
        if cap.id != 9 || cap.private_header >> 8 != 8 {
            continue;
        }
        if (cap.private_header as u8) < 24 || cap.offset > 232 {
            return Err(Status::ErrInvalidArgs);
        }
        let read = |offset| descriptors.read_word(device, cap.offset + offset);
        let bar_id = read(4);
        if (bar_id >> 8) as u8 != 1 {
            continue;
        } // VIRTIO_GPU_SHM_ID_HOST_VISIBLE
        if shared.is_some() || bar_id as u8 >= 6 {
            return Err(Status::ErrInvalidArgs);
        }
        shared = Some((
            bar_id as u8,
            (read(8) as u64) | ((read(16) as u64) << 32),
            (read(12) as u64) | ((read(20) as u64) << 32),
        ));
    }
    let Some((bar, offset, size)) = shared else {
        return Ok(Aperture::default());
    };
    // bar_info sizes the BAR by temporarily disabling memory decode. This must
    // never run during adoption or after a queue has been made device-visible.
    let Some(BarInfo::Memory {
        address,
        size: bar_size,
        prefetchable: true,
        ..
    }) = root
        .bar_info(device, bar)
        .map_err(|_| Status::ErrInvalidArgs)?
    else {
        return Err(Status::ErrInvalidArgs);
    };
    Aperture::new(address, bar_size, offset, size).map_err(|_| Status::ErrInvalidArgs)
}
