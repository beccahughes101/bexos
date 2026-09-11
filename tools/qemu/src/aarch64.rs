use bexos_boot::KERNEL_START;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const RAW_QEMU_AARCH64_MACHINE: &str =
    "virt,secure=off,virtualization=on,iommu=smmuv3,highmem-ecam=off,highmem-mmio=off";
pub const SECURE_QEMU_AARCH64_MACHINE: &str =
    "virt,secure=on,virtualization=on,iommu=smmuv3,highmem-ecam=off,highmem-mmio=off";

#[derive(Clone, Debug)]
pub struct Aarch64Firmware {
    pub bl1: PathBuf,
    pub bl2: PathBuf,
    pub bl31: PathBuf,
    pub bl32: PathBuf,
    pub bl33: PathBuf,
    /// Optional leaf certificate for a separately authenticated test verifier.
    /// All intermediate certificates still come from the selected BL1 bundle.
    pub bl33_certificate: Option<PathBuf>,
}

pub fn qemu_aarch64_machine_configuration(secure_firmware: bool) -> &'static str {
    if secure_firmware {
        SECURE_QEMU_AARCH64_MACHINE
    } else {
        RAW_QEMU_AARCH64_MACHINE
    }
}

pub(crate) fn configure_boot(
    command: &mut Command,
    firmware: Option<&Path>,
    kernel: &Path,
    rpmb_socket: &Path,
) {
    if let Some(directory) = firmware {
        command
            .current_dir(directory)
            .arg("-bios")
            .arg(directory.join("bl1.bin"))
            .arg("-semihosting-config")
            .arg("enable=on,target=native")
            // QEMU connects chardevs in declaration order. The RPMB helper
            // serves this boot connection first, then the queued virtio one
            // after BL33 closes its QL handles and releases the boot owner.
            .arg("-chardev")
            .arg(format!("socket,id=bootrpmb,path={}", rpmb_socket.display()))
            .args(["-device", "pci-serial,addr=7,chardev=bootrpmb"])
            .args(super::rpmb_qemu_arguments(rpmb_socket));
        // TF-A authenticates BL33, which verifies the separately loaded kernel.
        command.arg("-device").arg(format!(
            "loader,file={},addr=0x{KERNEL_START:x},force-raw=on",
            kernel.display()
        ));
    } else {
        command.arg("-kernel").arg(kernel);
    }
}
