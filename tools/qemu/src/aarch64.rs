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
    rpmb_sockets: Option<(&Path, &Path)>,
) {
    if let Some(directory) = firmware {
        let (boot_socket, runtime_socket) =
            rpmb_sockets.expect("secure ARM QEMU requires RPMB relay sockets");
        let boot_socket = boot_socket.file_name().unwrap_or(boot_socket.as_os_str());
        let runtime_socket = runtime_socket
            .file_name()
            .unwrap_or(runtime_socket.as_os_str());
        command
            .current_dir(directory)
            .arg("-bios")
            .arg(directory.join("bl1.bin"))
            .arg("-semihosting-config")
            .arg("enable=on,target=native")
            // Distinct relay sockets keep both guest frontends connected while
            // the host owner serializes their authenticated backend access.
            .arg("-chardev")
            .arg(format!(
                "socket,id=bootrpmb,path={}",
                Path::new(boot_socket).display()
            ))
            .args(["-device", "pci-serial,addr=7,chardev=bootrpmb"])
            .arg("-chardev")
            .arg(format!(
                "socket,id=rpmb0,path={}",
                Path::new(runtime_socket).display()
            ))
            .args([
                "-device",
                "virtio-serial-pci,id=rpmbbus,disable-legacy=on,disable-modern=off,romfile=",
                "-device",
                "virtserialport,bus=rpmbbus.0,chardev=rpmb0,name=rpmb0,nr=1",
            ]);
        // TF-A authenticates BL33, which verifies the separately loaded kernel.
        command.arg("-device").arg(format!(
            "loader,file={},addr=0x{KERNEL_START:x},force-raw=on",
            kernel.display()
        ));
    } else {
        command.arg("-kernel").arg(kernel);
    }
}
