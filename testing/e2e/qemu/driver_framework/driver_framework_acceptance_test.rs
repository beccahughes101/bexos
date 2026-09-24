use bexos_e2e::{E2eDevice, NVME_MARKERS, VIRTIO_NET_MARKERS, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let artifacts = QemuArtifacts::from_args(&args)?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(VIRTIO_NET_MARKERS);
    markers.extend_from_slice(&[
        b"pci: root segment=".as_slice(),
        b"e1000e: ready mac=".as_slice(),
        b"igb: ready mac=".as_slice(),
    ]);

    let mut device = QemuDevice::new(artifacts)?;
    let output = device.boot(&markers)?;
    let registrations = output
        .windows(b"pci: device node=".len())
        .filter(|window| *window == b"pci: device node=")
        .count();
    if registrations < 4 {
        return Err(format!(
            "expected independent NVMe, VirtIO-Net, e1000e and igb PCI nodes; observed {registrations}"
        ));
    }
    Ok(())
}
