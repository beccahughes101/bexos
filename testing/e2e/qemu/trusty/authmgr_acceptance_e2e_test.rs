use bexos_e2e::{E2eDevice, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let mut device = QemuDevice::new(QemuArtifacts::from_args(&[])?)?;
    let mut markers = boot_markers();
    markers.extend_from_slice(&[
        b"bexos-authmgr-acceptance: authenticated connection accepted",
        b"bexos-authmgr-acceptance: malformed DICE rejected",
        b"bexos-authmgr-acceptance: unauthenticated completion rejected",
        b"bexos-authmgr-acceptance: complete",
    ]);
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()
}
