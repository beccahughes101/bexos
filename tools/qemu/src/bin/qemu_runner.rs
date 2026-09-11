use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use bexos_e2e::{BEXFS_MARKERS, E2eDevice, NVME_MARKERS, VIRTIO_NET_MARKERS};
use bexos_qemu_test::{Architecture, LaunchConfig, Product, QemuArtifacts, QemuDevice};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let e2e = take_flag(&mut args, "--e2e");
    let debugd = take_flag(&mut args, "--debugd");
    let developer = take_flag(&mut args, "--developer");
    let arch = take_option(&mut args, "--arch")?.unwrap_or_else(|| "aarch64".into());
    let architecture = Architecture::parse(&arch)?;

    let product =
        Product::parse(&take_option(&mut args, "--product")?.unwrap_or_else(|| "nongui".into()))?;
    let launch = take_option(&mut args, "--launch-config")?
        .map(|path| LaunchConfig::read(std::path::Path::new(&path)))
        .transpose()?
        .unwrap_or_default();
    let debug_socket = take_option(&mut args, "--debug-socket")?
        .map(PathBuf::from)
        .unwrap_or_else(|| product.debug_socket(architecture));
    if args.len() >= 12 {
        args.splice(12..12, ["--arch".into(), arch]);
    }
    let artifacts = QemuArtifacts::from_args(&args)?;
    let mut device = QemuDevice::new(artifacts)?;
    if developer {
        return device.run_developer_instance_with_config(&debug_socket, product, &launch);
    }
    let mut markers = bexos_e2e::boot_markers_for_arch(architecture == Architecture::X86_64);
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(VIRTIO_NET_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    if debugd {
        let mut session = device.boot_with_debugd(&markers)?;
        session.assert_debugd_ready()?;
    } else {
        let first = device.boot(&markers)?;
        if !String::from_utf8_lossy(&first).contains("generation=2 prior=1") {
            return Err("first boot did not advance SYS_STATE to generation 2".into());
        }
        if e2e {
            let second = device.boot(&markers)?;
            if !String::from_utf8_lossy(&second).contains("generation=3 prior=2") {
                return Err("second boot did not advance SYS_STATE to generation 3".into());
            }
        }
    }
    let generation = if e2e { "3" } else { "2" };
    device.inspect_path(
        "SYS_STATE",
        "SYS_STATE",
        "boot_state.bin",
        &["--sys-state", "--expect-generation", generation],
    )?;
    device.inspect_path(
        "STORAGE",
        "STORAGE",
        "data/persistence.txt",
        &["--expect-text", "BexOS guest persistent data v1"],
    )?;
    Ok(())
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_option(args: &mut Vec<String>, option: &str) -> Result<Option<String>, String> {
    if let Some(index) = args.iter().position(|arg| arg == option) {
        args.remove(index);
        if index >= args.len() {
            return Err(format!("{option} requires VALUE"));
        }
        Ok(Some(args.remove(index)))
    } else {
        Ok(None)
    }
}
