//! Integrated EFI/SVM launch and boot-to-normal-world RPMB ownership handoff.
use super::{ManagedQemuChild, QemuDevice};
use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::thread::{self, JoinHandle};
#[path = "firmware_disk.rs"]
mod firmware_disk;

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
    firmware_disk::prepare(&firmware_disk)?;
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
    };
    let original = child.qemu.stdout.take().ok_or("missing QEMU stdout")?;
    let (output, relay) = Relay::start(original, control, socket)?;
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

/// Tee the existing child output through a socket, retaining the ordinary E2E
/// reader and cleanup paths. The relay controls transport only on root-owned
/// boot markers, before normal-world execution can emit arbitrary output.
pub(super) struct Relay {
    shutdown: UnixStream,
    thread: Option<JoinHandle<()>>,
}
impl Relay {
    fn start(
        mut input: impl Read + Send + 'static,
        control: PathBuf,
        rpmb: PathBuf,
    ) -> Result<(ChildStdout, Self), String> {
        let (output, mut writer) = UnixStream::pair().map_err(|e| e.to_string())?;
        let shutdown = writer.try_clone().map_err(|e| e.to_string())?;
        let task = thread::spawn(move || {
            let mut attached = false;
            let mut transferred = false;
            let mut recent = Vec::new();
            let mut buffer = [0; 4096];
            while let Ok(count) = input.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                recent.extend_from_slice(&buffer[..count]);
                let result = (|| {
                    if !attached
                        && super::contains(
                            &recent,
                            b"monitor-runtime: entering assigned Trusty domain",
                        )
                    {
                        super::qmp::change_serial(&control, "rpmb", Some(&rpmb))?;
                        attached = true;
                    }
                    if attached
                        && !transferred
                        && super::contains(
                            &recent,
                            b"monitor-runtime: boot RPMB owner release verified",
                        )
                    {
                        super::qmp::change_serial(&control, "rpmb", None)?;
                        super::qmp::change_serial(&control, "normalrpmb", Some(&rpmb))?;
                        transferred = true;
                    }
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    let _ = writeln!(writer, "panic: secure QEMU transport setup failed: {error}");
                    break;
                }
                if writer.write_all(&buffer[..count]).is_err() {
                    break;
                }
                if recent.len() > 512 {
                    recent.drain(..recent.len() - 512);
                }
            }
            // The owner retains a cloned endpoint to interrupt a blocked
            // writer during cancellation. Explicitly half-close here so that
            // normal EOF reaches readers before the owner itself is dropped.
            let _ = writer.shutdown(Shutdown::Write);
        });
        Ok((
            ChildStdout::from(OwnedFd::from(output)),
            Self {
                shutdown,
                thread: Some(task),
            },
        ))
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.shutdown.shutdown(Shutdown::Both);
        if let Some(task) = self.thread.take() {
            let _ = task.join();
        }
    }
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
    #[test]
    fn relay_eof_does_not_wait_for_owner_drop() {
        let expected = b"guest completed\n";
        let (output, relay) = Relay::start(
            std::io::Cursor::new(expected),
            PathBuf::new(),
            PathBuf::new(),
        )
        .unwrap();
        let mut socket = UnixStream::from(OwnedFd::from(output));
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
        drop(relay);
    }
    #[test]
    fn cancellation_releases_a_backpressured_relay() {
        let (_output, relay) = Relay::start(
            std::io::Cursor::new(vec![b'x'; 1024 * 1024]),
            PathBuf::new(),
            PathBuf::new(),
        )
        .unwrap();
        // Keep the consumer alive without reading while canceling its writer.
        drop(relay);
    }
}
