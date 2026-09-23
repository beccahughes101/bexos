//! Integrated EFI/SVM launch and boot-to-normal-world RPMB ownership handoff.
use super::{ManagedQemuChild, QemuDevice};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct X86SecureFirmware {
    pub code: PathBuf,
    pub variables: PathBuf,
    pub loader: PathBuf,
}
impl X86SecureFirmware {
    pub fn canonicalize(&mut self) -> Result<(), String> {
        for path in [&mut self.code, &mut self.variables, &mut self.loader] {
            *path = path
                .canonicalize()
                .map_err(|e| format!("resolve x86 firmware {}: {e}", path.display()))?;
        }
        Ok(())
    }
}
pub fn from_env() -> Result<Option<X86SecureFirmware>, String> {
    let Some(code) = super::env_path("BEXOS_QEMU_X86_OVMF_CODE")? else {
        return Ok(None);
    };
    Ok(Some(X86SecureFirmware {
        code,
        variables: super::required_env_path("BEXOS_QEMU_X86_OVMF_VARS")?,
        loader: super::required_env_path("BEXOS_QEMU_X86_EFI_LOADER")?,
    }))
}

pub fn spawn(
    device: &QemuDevice,
    debug: Option<&Path>,
    launch: &super::LaunchConfig,
) -> Result<ManagedQemuChild, String> {
    let firmware = device
        .artifacts
        .x86_secure_firmware
        .as_ref()
        .ok_or("missing x86 firmware")?;
    let work = &device.workdir;
    let firmware_disk = work.join("firmware.raw");
    super::firmware_disk::prepare(&firmware_disk)?;
    let esp = work.join("esp");
    let efi = esp.join("EFI/BOOT");
    fs::create_dir_all(&efi).map_err(|e| format!("create EFI system partition: {e}"))?;
    stage_loader(&firmware.loader, &efi.join("BOOTX64.EFI"))
        .map_err(|e| format!("stage EFI loader: {e}"))?;
    let variables = work.join("vars.fd");
    if !variables.exists() {
        fs::copy(&firmware.variables, &variables)
            .map_err(|e| format!("stage EFI variables: {e}"))?;
        let mut permissions = fs::metadata(&variables)
            .map_err(|e| e.to_string())?
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&variables, permissions).map_err(|e| e.to_string())?;
    }
    let entropy = work.join("monitor.entropy");
    let mut seed = [0; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut seed))
        .map_err(|e| format!("fresh monitor entropy: {e}"))?;
    if seed == [0; 32] {
        return Err("zero monitor entropy".into());
    }
    fs::write(&entropy, seed).map_err(|e| e.to_string())?;
    seed.fill(0);
    let idle = work.join("debug-idle.sock");
    let debug = debug.unwrap_or(&idle);
    let control = work.join("monitor-qmp.sock");
    for path in [debug, control.as_path()] {
        super::remove_stale_socket(path)?;
    }
    let mut command = Command::new(&device.qemu);
    command.args([
        "-machine",
        "q35,accel=tcg,smm=on",
        "-cpu",
        "max",
        "-smp",
        "1",
        "-m",
        "3072",
        "-monitor",
        "none",
        "-serial",
        "stdio",
        "-no-reboot",
        "-net",
        "none",
        "-global",
        "driver=cfi.pflash01,property=secure,value=on",
    ]);
    launch.configure_display(&mut command);
    for drive in [
        format!(
            "if=pflash,format=raw,unit=0,readonly=on,file={}",
            firmware.code.display()
        ),
        format!("if=pflash,format=raw,unit=1,file={}", variables.display()),
        format!("format=raw,file=fat:rw:{}", esp.display()),
        format!(
            "if=none,id=firmwaredisk,format=raw,cache=writeback,file={}",
            firmware_disk.display()
        ),
        format!(
            "if=none,id=normaldisk,file={},format=raw",
            device.disk.display()
        ),
    ] {
        command.arg("-drive").arg(drive);
    }
    command
        .arg("-qmp")
        .arg(format!("unix:{},server=on,wait=off", control.display()));
    // OVMF must never send terminal probes into the opaque RPMB protocol.
    for name in ["rpmb", "normalrpmb"] {
        let detached = work.join(format!("{name}-detached.sock"));
        super::remove_stale_socket(&detached)?;
        command.arg("-chardev").arg(format!(
            "socket,id={name},path={},server=on,wait=off",
            detached.display()
        ));
    }
    command.args(["-serial", "chardev:rpmb"]);
    command.arg("-chardev").arg(format!(
        "socket,id=debug0,path={},server=on,wait=off",
        debug.display()
    ));
    for value in [
        "isa-ide,id=firmwarebus,iobase=0x1f0,iobase2=0x3f6,irq=14",
        "ide-hd,bus=firmwarebus.0,unit=0,drive=firmwaredisk",
        "intel-iommu,intremap=on,eim=off",
        "nvme,addr=3,drive=normaldisk,serial=bexos-nvme0",
        "virtio-serial-pci,id=debugbus,addr=4,disable-legacy=on,disable-modern=off,iommu_platform=on",
        "virtserialport,bus=debugbus.0,chardev=debug0,name=debug0,nr=1",
        "virtio-net-pci,addr=5,netdev=normalnet,disable-legacy=on,disable-modern=off,iommu_platform=on,mac=52:54:00:12:34:56",
        "virtio-serial-pci,id=rpmbbus,addr=6,disable-legacy=on,disable-modern=off,iommu_platform=on",
        "virtserialport,bus=rpmbbus.0,chardev=normalrpmb,name=rpmb0,nr=1",
    ] {
        command.arg("-device").arg(value);
    }
    launch.append_devices(&mut command);
    command.args(&device.artifacts.extra_args);
    command.args(["-netdev", "user,id=normalnet"]);
    for (name, path) in [
        ("normal-entropy", &entropy),
        ("kernel", &device.artifacts.kernel),
        ("bootfs", &device.artifacts.bootfs),
        ("vbmeta", &device.artifacts.vbmeta),
    ] {
        command
            .arg("-fw_cfg")
            .arg(format!("name=opt/bexos/{name},file={}", path.display()));
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::host_activity::configure(&mut command)?;
    let (mut rpmb, socket) = device.spawn_rpmb()?;
    eprintln!("e2e: integrated secure qemu command {command:?}");
    let qemu = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            super::kill_child(&mut rpmb);
            return Err(format!("spawn secure QEMU: {error}"));
        }
    };
    let mut child = ManagedQemuChild {
        qemu,
        rpmb: Some(rpmb),
        _idle_serial: None,
        readers: Vec::new(),
        secure_relay: None,
        arm_rpmb_proxy: None,
    };
    let original = child.qemu.stdout.take().ok_or("missing QEMU stdout")?;
    let (output, relay) = super::rpmb_relay::Relay::start(
        original,
        control,
        socket,
        super::rpmb_relay::Transfer {
            boot_device: "rpmb",
            runtime_device: "normalrpmb",
            attach_marker: b"monitor-runtime: entering assigned Trusty domain",
            release_marker: b"monitor-runtime: boot RPMB owner release verified",
            initially_attached: false,
        },
    )?;
    child.qemu.stdout = Some(output);
    child.secure_relay = Some(relay);
    if debug == idle {
        child._idle_serial = Some(super::connect_socket(debug)?);
    }
    Ok(child)
}

/// Cached inputs and an existing ESP loader can both be read-only. Replace the
/// directory entry only after a complete copy, retaining the previous image if
/// staging fails. The guest has exited before another spawn reaches this path.
fn stage_loader(source: &Path, destination: &Path) -> std::io::Result<()> {
    let temporary = destination.with_extension("EFI.staged");
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let result = (|| {
        fs::copy(source, &temporary)?;
        fs::File::open(&temporary)?.sync_all()?;
        fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reboot_replaces_readonly_loader_and_failed_copy_retains_previous() {
        let directory =
            std::env::temp_dir().join(format!("bexos-efi-stage-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("cached.efi");
        let destination = directory.join("BOOTX64.EFI");
        fs::write(&source, b"signed loader").unwrap();
        let mut permissions = fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&source, permissions).unwrap();
        stage_loader(&source, &destination).unwrap();
        stage_loader(&source, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"signed loader");
        assert!(stage_loader(&directory.join("missing"), &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"signed loader");
        assert!(!destination.with_extension("EFI.staged").exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
