use bexos_e2e::{BEXFS_MARKERS, NVME_MARKERS, boot_markers};
use bexos_qemu_test::{QemuAarch64Device, QemuArtifacts};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (artifacts, _extra) = QemuArtifacts::from_env_or_args(&args)?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    // On an arm64 macOS host the integrated x86 guest runs entirely under
    // TCG. Loading the WASM runner and compiling its first component are two
    // independently meaningful startup transitions, so require both and let
    // the boot watchdog observe the latter before waiting for service entry.
    markers.push(b"wasm_runner: compiling service component bytes=");
    markers.push(b"wasm_runtime: component compilation complete");
    markers.push(b"wasm_runner: service component instantiated");
    markers.push(b"sdk-fixture-wasm: out-of-tree service started");
    markers.push(b"sdk-fixture-native: out-of-tree service started");
    let mut device = QemuAarch64Device::new(artifacts)?;
    device.boot_until_markers(&markers)?;
    Ok(())
}
