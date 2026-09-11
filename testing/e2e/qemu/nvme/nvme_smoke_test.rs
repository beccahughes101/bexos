use bexos_e2e::{E2eDevice, NVME_MARKERS, boot_markers};
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
    let mut device = QemuDevice::new(artifacts)?;
    device.boot(&markers)?;
    Ok(())
}
