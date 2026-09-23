mod aarch64;
pub use aarch64::{
    Aarch64Firmware, RAW_QEMU_AARCH64_MACHINE, SECURE_QEMU_AARCH64_MACHINE,
    qemu_aarch64_machine_configuration,
};
/// Compatibility name for the ARM TF-A firmware bundle.
pub type SecureFirmware = Aarch64Firmware;

mod architecture;
pub use architecture::Architecture;

mod arm_rpmb_proxy;
mod entropy;
mod firmware_disk;
mod host_activity;
mod launch;
mod shutdown;
pub use launch::{LaunchConfig, Product};
mod qmp;
mod rpmb_relay;
mod x86_secure;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
pub use x86_secure::X86SecureFirmware;

use bexos_avb::verify_vbmeta;
use bexos_boot::{
    BOOT_EVIDENCE_FLAG_IOMMU_STRICT, BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK,
    BOOT_EVIDENCE_FLAG_SECURE_BOOT, BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR, BootEvidenceV1,
};
use bexos_crypto::verify_ed25519;
use bexos_debug_client::{DebugClient, DebugTransport, UnixSocketTransport};
use bexos_e2e::{DebugSession, DebugSessionGuard, E2eDevice, assert_absent, assert_markers};
use bexos_kernel_core::bootfs::Bootfs;
use sha2::{Digest, Sha256};

const DEBUGD_READY: &[u8] = b"debugd: QEMU socket transport ready";
const DEBUGD_SHELL_DEPENDENCIES: &[&[u8]] = &[
    b"debugd: app lifecycle proxy connected",
    b"debugd: user proxy connected",
    b"appd: lifecycle dispatch ready",
];
const SECURE_BOOT_REJECTIONS: &[&[u8]] = &[
    b"monitor-runtime: boot payload snapshot rejected",
    b"monitor-runtime: boot payload authentication rejected",
    b"monitor-runtime: stale generation rejected by Trusty RPMB floor",
    b"monitor-runtime: unlocked Trusty boot state rejected",
    b"monitor-runtime: invalid boot generation rejected",
    b"refusing execution",
    b": Access Denied\r\n",
];
fn secure_boot_rejected(output: &[u8]) -> bool {
    SECURE_BOOT_REJECTIONS
        .iter()
        .any(|marker| contains(output, marker))
}

fn guest_firmware_started(output: &[u8]) -> bool {
    contains(output, b"Booting Trusted Firmware")
        || contains(output, b"UEFI")
        || contains(output, b"monitor-runtime:")
        || contains(output, b"SeaBIOS")
}
static INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
pub const DEFAULT_DEVELOPER_DEBUG_SOCKET: &str = "/tmp/bexos-qemu-nongui-aarch64-debugd.sock";
#[derive(Clone, Debug)]
pub struct QemuArtifacts {
    pub architecture: Architecture,
    pub development: bool,
    pub boot_image: Option<PathBuf>,
    pub kernel: PathBuf,
    pub disk: PathBuf,
    pub bootfs: PathBuf,
    pub handoff: PathBuf,
    pub evidence: PathBuf,
    pub vbmeta: PathBuf,
    pub avb_public_key: PathBuf,
    pub layout: PathBuf,
    pub inspector: PathBuf,
    pub key: PathBuf,
    pub rpmbd: PathBuf,
    pub rpmb_template: PathBuf,
    pub extra_args: Vec<String>,
    pub secure_firmware: Option<SecureFirmware>,
    pub x86_secure_firmware: Option<X86SecureFirmware>,
}

impl QemuArtifacts {
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        Self::from_env_or_args(args).map(|(artifacts, _extra)| artifacts)
    }

    pub fn from_env_or_args(args: &[String]) -> Result<(Self, Vec<String>), String> {
        // Explicit launch artifacts take precedence over an E2E environment.
        if args.len() >= 12 {
            return Self::parse_args(args);
        }
        if let Some(artifacts) = Self::from_env()? {
            return Ok((artifacts, split_env_args("BEXOS_QEMU_TEST_ARGS")));
        }
        Self::parse_args(args)
    }

    fn validate_architecture(&self) -> Result<(), String> {
        if self.development
            && (self.secure_firmware.is_some() || self.x86_secure_firmware.is_some())
        {
            return Err("development profile cannot use integrated secure firmware".into());
        }
        match self.architecture {
            Architecture::Aarch64 if self.x86_secure_firmware.is_some() => {
                Err("x86 EFI firmware cannot boot an ARM guest".into())
            }
            Architecture::Aarch64 if self.boot_image.is_some() => {
                Err("x86 boot image cannot boot an ARM guest".into())
            }
            Architecture::X86_64 if self.secure_firmware.is_some() => {
                Err("ARM TF-A firmware cannot boot an x86 guest".into())
            }
            Architecture::X86_64
                if self.boot_image.is_some() && self.x86_secure_firmware.is_some() =>
            {
                Err("x86 secure firmware and development Multiboot are mutually exclusive".into())
            }
            Architecture::X86_64
                if self.boot_image.is_none() && self.x86_secure_firmware.is_none() =>
            {
                Err("x86 guest requires a Multiboot2 boot image".into())
            }
            Architecture::X86_64 if !self.development && self.x86_secure_firmware.is_none() => {
                Err("integrated x86 requires authenticated EFI monitor firmware".into())
            }
            _ => Ok(()),
        }
    }

    pub fn machine_configuration(&self) -> &'static str {
        if self.x86_secure_firmware.is_some() {
            return "q35,accel=tcg,smm=on";
        }
        self.architecture.machine(self.secure_firmware.is_some())
    }

    fn from_env() -> Result<Option<Self>, String> {
        let Some(kernel) = env_path("BEXOS_QEMU_KERNEL")? else {
            return Ok(None);
        };
        let extra_args = split_env_args("BEXOS_QEMU_DEVICE_ARGS");
        Ok(Some(Self {
            architecture: Architecture::from_env()?,
            development: std::env::var("BEXOS_QEMU_DEVELOPMENT").as_deref() == Ok("1"),
            boot_image: env_path("BEXOS_QEMU_BOOT_IMAGE")?,
            kernel,
            disk: required_env_path("BEXOS_QEMU_DISK")?,
            bootfs: required_env_path("BEXOS_QEMU_BOOTFS")?,
            handoff: required_env_path("BEXOS_QEMU_HANDOFF")?,
            evidence: required_env_path("BEXOS_QEMU_EVIDENCE")?,
            vbmeta: required_env_path("BEXOS_QEMU_VBMETA")?,
            avb_public_key: required_env_path("BEXOS_QEMU_AVB_PUBLIC_KEY")?,
            layout: required_env_path("BEXOS_QEMU_LAYOUT")?,
            inspector: required_env_path("BEXOS_QEMU_INSPECTOR")?,
            key: required_env_path("BEXOS_QEMU_KEY")?,
            rpmbd: required_env_path("BEXOS_QEMU_RPMBD")?,
            rpmb_template: required_env_path("BEXOS_QEMU_RPMB_TEMPLATE")?,
            extra_args,
            secure_firmware: secure_firmware_from_env()?,
            x86_secure_firmware: x86_secure::from_env()?,
        }))
    }

    fn parse_args(args: &[String]) -> Result<(Self, Vec<String>), String> {
        if args.len() < 12 {
            return Err(
                "usage: TEST kernel disk bootfs handoff evidence vbmeta avb-public-key layout inspector key rpmbd rpmb-template [extra...]"
                    .into(),
            );
        }
        let mut secure = None;
        let mut development = false;
        let mut architecture = Architecture::from_env()?;
        let mut boot_image = None;
        let mut x86_secure_firmware = None;
        let mut extra = &args[12..];
        while let Some((flag, rest)) = extra.split_first() {
            match flag.as_str() {
                "--secure-x86" => {
                    if rest.len() < 3 {
                        return Err(
                            "--secure-x86 requires OVMF code, enrolled variables and EFI loader"
                                .into(),
                        );
                    }
                    x86_secure_firmware = Some(X86SecureFirmware {
                        code: rest[0].clone().into(),
                        variables: rest[1].clone().into(),
                        loader: rest[2].clone().into(),
                    });
                    extra = &rest[3..];
                }
                "--development" => {
                    development = true;
                    extra = rest;
                }
                "--arch" | "--boot-image" => {
                    let (value, tail) = rest
                        .split_first()
                        .ok_or_else(|| format!("{flag} requires a value"))?;
                    if flag == "--arch" {
                        architecture = Architecture::parse(value)?;
                    } else {
                        boot_image = Some(value.into());
                    }
                    extra = tail;
                }
                "--secure-bl1" => {
                    let Some((path, tail)) = rest.split_first() else {
                        return Err("--secure-bl1 requires a path".into());
                    };
                    let Some(bl2) = tail.first() else {
                        return Err("--secure-bl1 requires BL2 BL31 BL32 paths".into());
                    };
                    let Some(bl31) = tail.get(1) else {
                        return Err("--secure-bl1 requires BL2 BL31 BL32 paths".into());
                    };
                    let Some(bl32) = tail.get(2) else {
                        return Err("--secure-bl1 requires BL2 BL31 BL32 paths".into());
                    };
                    let Some(bl33) = tail.get(3) else {
                        return Err("--secure-bl1 requires BL2 BL31 BL32 BL33 paths".into());
                    };
                    secure = Some(SecureFirmware {
                        bl1: path.clone().into(),
                        bl2: bl2.clone().into(),
                        bl31: bl31.clone().into(),
                        bl32: bl32.clone().into(),
                        bl33: bl33.clone().into(),
                        bl33_certificate: None,
                    });
                    extra = &tail[4..];
                }
                _ => break,
            }
        }
        let extra_args = extra.to_vec();
        Ok((
            Self {
                architecture,
                development,
                boot_image,
                kernel: args[0].clone().into(),
                disk: args[1].clone().into(),
                bootfs: args[2].clone().into(),
                handoff: args[3].clone().into(),
                evidence: args[4].clone().into(),
                vbmeta: args[5].clone().into(),
                avb_public_key: args[6].clone().into(),
                layout: args[7].clone().into(),
                inspector: args[8].clone().into(),
                key: args[9].clone().into(),
                rpmbd: args[10].clone().into(),
                rpmb_template: args[11].clone().into(),
                extra_args: extra_args.clone(),
                secure_firmware: secure,
                x86_secure_firmware,
            },
            extra_args,
        ))
    }
}

fn secure_firmware_from_env() -> Result<Option<SecureFirmware>, String> {
    let Some(bl1) = env_path("BEXOS_QEMU_SECURE_BL1")? else {
        return Ok(None);
    };
    Ok(Some(SecureFirmware {
        bl1,
        bl2: required_env_path("BEXOS_QEMU_SECURE_BL2")?,
        bl31: required_env_path("BEXOS_QEMU_SECURE_BL31")?,
        bl32: required_env_path("BEXOS_QEMU_SECURE_BL32")?,
        bl33: required_env_path("BEXOS_QEMU_SECURE_BL33")?,
        bl33_certificate: None,
    }))
}

fn env_path(name: &str) -> Result<Option<PathBuf>, String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(Some(resolve_runfile_path(&value))),
        Ok(_) => Ok(None),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("read {name}: {error}")),
    }
}

fn required_env_path(name: &str) -> Result<PathBuf, String> {
    env_path(name)?.ok_or_else(|| format!("missing {name}"))
}

fn split_env_args(name: &str) -> Vec<String> {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .split_whitespace()
                .map(|token| {
                    let path = resolve_runfile_path(token);
                    if path.exists() {
                        path.display().to_string()
                    } else {
                        token.to_string()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_runfile_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("${pwd}/") {
        if let Ok(cwd) = std::env::current_dir() {
            let candidate = cwd.join(rest);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    let path = PathBuf::from(value);
    if path.exists() || path.is_absolute() {
        return path;
    }
    for root in ["RUNFILES_DIR", "TEST_SRCDIR"] {
        if let Ok(dir) = std::env::var(root) {
            let candidate = PathBuf::from(&dir).join(&path);
            if candidate.exists() {
                return candidate;
            }
            if let Ok(workspace) = std::env::var("TEST_WORKSPACE") {
                let candidate = PathBuf::from(&dir).join(workspace).join(&path);
                if candidate.exists() {
                    return candidate;
                }
            }
        }
    }
    path
}

#[derive(Debug)]
pub struct QemuDevice {
    qemu: PathBuf,
    artifacts: QemuArtifacts,
    workdir: PathBuf,
    disk: PathBuf,
    rpmb: PathBuf,
    secure_firmware_dir: Option<PathBuf>,
    layout: Layout,
}

#[derive(Clone, Copy, Debug)]
struct Layout {
    ram_mib: u64,
    bootfs_addr: u64,
    handoff_addr: u64,
    evidence_addr: u64,
    vbmeta_addr: u64,
    max_cpus: u32,
}

impl QemuDevice {
    pub fn new(artifacts: QemuArtifacts) -> Result<Self, String> {
        Self::new_inner(artifacts, true)
    }

    /// Test-only negative-boot entry point. Production/developer launches use
    /// `new`, which catches corrupt artifacts before spawning QEMU; AVB E2E
    /// tests bypass that convenience check to prove BL33 itself fails closed.
    pub fn new_without_host_preflight(artifacts: QemuArtifacts) -> Result<Self, String> {
        Self::new_inner(artifacts, false)
    }

    fn new_inner(mut artifacts: QemuArtifacts, preflight: bool) -> Result<Self, String> {
        artifacts.validate_architecture()?;
        if preflight {
            verify_boot_artifacts(&artifacts)?;
        }
        canonicalize_artifacts(&mut artifacts)?;
        let qemu = find_qemu(artifacts.architecture)?;
        let instance = INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        let workdir = PathBuf::from(format!("/tmp/bexos-qemu-{}-{instance}", std::process::id()));
        fs::create_dir_all(&workdir).map_err(|e| format!("create workdir: {e}"))?;
        let seeded_handoff = workdir.join("boot_handoff.bin");
        entropy::stage(&artifacts.handoff, &seeded_handoff)?;
        artifacts.handoff = seeded_handoff;
        let disk = workdir.join("nvme.img");
        fs::copy(&artifacts.disk, &disk)
            .map_err(|e| format!("copy disk image {}: {e}", artifacts.disk.display()))?;
        let mut permissions = fs::metadata(&disk)
            .map_err(|e| format!("stat copied disk image: {e}"))?
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&disk, permissions)
            .map_err(|e| format!("make copied disk image writable: {e}"))?;
        let rpmb = match std::env::var("BEXOS_QEMU_RPMB_STATE") {
            Ok(path) if !path.is_empty() => PathBuf::from(path),
            _ => workdir.join("RPMB_DATA"),
        };
        if !artifacts.development {
            prepare_rpmb_image(&artifacts.rpmb_template, &rpmb)?;
        }
        if artifacts.architecture == Architecture::Aarch64 && artifacts.secure_firmware.is_some() {
            firmware_disk::prepare(&workdir.join("firmware.raw"))?;
        }
        let layout = parse_layout(&artifacts.layout)?;
        let secure_firmware_dir = if let Some(firmware) = &artifacts.secure_firmware {
            for (source, name) in [
                (&firmware.bl1, "bl1.bin"),
                (&firmware.bl2, "bl2.bin"),
                (&firmware.bl31, "bl31.bin"),
                (&firmware.bl32, "bl32.bin"),
                (&firmware.bl33, "bl33.bin"),
            ] {
                fs::copy(source, workdir.join(name))
                    .map_err(|e| format!("stage secure firmware {}: {e}", source.display()))?;
            }
            let firmware_root = firmware.bl1.parent().ok_or_else(|| {
                format!(
                    "secure firmware path has no parent: {}",
                    firmware.bl1.display()
                )
            })?;
            for certificate in [
                "tb_fw.crt",
                "trusted_key.crt",
                "soc_fw_key.crt",
                "tos_fw_key.crt",
                "nt_fw_key.crt",
                "soc_fw_content.crt",
                "tos_fw_content.crt",
                "nt_fw_content.crt",
            ] {
                let source = if certificate == "nt_fw_content.crt" {
                    firmware
                        .bl33_certificate
                        .clone()
                        .unwrap_or_else(|| firmware_root.join(certificate))
                } else {
                    firmware_root.join(certificate)
                };
                fs::copy(&source, workdir.join(certificate)).map_err(|e| {
                    format!("stage trusted-boot certificate {}: {e}", source.display())
                })?;
            }
            Some(workdir.clone())
        } else {
            None
        };
        Ok(Self {
            qemu,
            artifacts,
            workdir,
            disk,
            rpmb,
            secure_firmware_dir,
            layout,
        })
    }

    pub fn disk_path(&self) -> &Path {
        &self.disk
    }

    pub fn rpmb_path(&self) -> &Path {
        &self.rpmb
    }
    pub fn firmware_disk_path(&self) -> PathBuf {
        self.workdir.join("firmware.raw")
    }

    /// Observe an actual guest rejection. A timeout or failed host launch is
    /// never evidence that verified boot rejected an image.
    pub fn assert_boot_rejected(
        &mut self,
        rejection: &[&[u8]],
        forbidden: &[&[u8]],
    ) -> Result<(), String> {
        for attempt in 0..2 {
            let mut child = self.spawn(None)?;
            let mut output = Vec::new();
            read_until_done(&mut child.qemu, None, &mut output, rejection)?;
            let result = validate_rejection(&output, rejection, forbidden);
            if result.is_ok() || guest_firmware_started(&output) || attempt == 1 {
                return result;
            }
            // A macOS host can occasionally reap a freshly spawned QEMU before
            // its first firmware instruction while rapidly cycling negative
            // boots. This output is not rejection evidence. Drop both managed
            // children, then retry the host launch once from the same immutable
            // inputs; a started guest or second failure remains a hard error.
            drop(child);
            thread::sleep(Duration::from_millis(250));
        }
        unreachable!()
    }

    /// The pinned storage TA deliberately terminates the critical application
    /// after a bad RPMB MAC. Require the requested explicit rejection (monitor
    /// refusal on x86 or the secure critical halt on ARM), while continuing to
    /// reject every unrelated panic.
    pub fn assert_rpmb_authentication_rejected(
        &mut self,
        rejection: &[&[u8]],
        forbidden: &[&[u8]],
    ) -> Result<(), String> {
        let mut child = self.spawn(None)?;
        let mut output = Vec::new();
        read_until_done_mode(&mut child.qemu, None, &mut output, rejection, true)?;
        assert_markers(
            &output,
            &[b": Bad MAC", b"block_device_tipc_init_rpmb_key failed"],
        )?;
        assert_markers(&output, rejection)?;
        validate_rejection(
            &without_rpmb_critical_exit(&output),
            &[b": Bad MAC", b"block_device_tipc_init_rpmb_key failed"],
            forbidden,
        )
    }

    pub fn run_developer_instance(&mut self, debug_socket: &Path) -> Result<(), String> {
        self.run_developer_instance_with_config(
            debug_socket,
            Product::Nongui,
            &LaunchConfig::default(),
        )
    }

    pub fn run_developer_instance_with_config(
        &mut self,
        debug_socket: &Path,
        product: Product,
        launch: &LaunchConfig,
    ) -> Result<(), String> {
        let _signals = shutdown::Signals::install()?;
        remove_stale_socket(debug_socket)?;
        let mut child = self.spawn_with_launch(Some(debug_socket), launch)?;
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        if let Some(stdout) = child.qemu.stdout.take() {
            child
                .readers
                .push(spawn_passthrough_reader(stdout, diagnostics.clone()));
        }
        if let Some(stderr) = child.qemu.stderr.take() {
            child
                .readers
                .push(spawn_passthrough_reader(stderr, diagnostics.clone()));
        }

        let mut serial = match connect_socket(debug_socket) {
            Ok(serial) => serial,
            Err(error) => {
                kill_child(&mut child.qemu);
                let _ = fs::remove_file(debug_socket);
                return Err(error);
            }
        };
        if let Err(error) = wait_for_debugd_ready(&mut child.qemu, &mut serial, &diagnostics) {
            kill_child(&mut child.qemu);
            let _ = fs::remove_file(debug_socket);
            return Err(error);
        }
        drop(serial);

        eprintln!(
            "BexOS QEMU developer instance is running; debugd socket: {}",
            debug_socket.display()
        );
        let config = if self.artifacts.architecture == Architecture::X86_64 {
            " --config=x86_64"
        } else {
            ""
        };
        eprintln!(
            "Run `bazel run{config} //device/virtual/qemu/{}:debugd -- health` from another terminal.",
            product.name()
        );
        eprintln!("Press Ctrl-C here to stop QEMU.");

        let status = loop {
            if shutdown::requested() {
                kill_child(&mut child.qemu);
            }
            if let Some(status) = child
                .qemu
                .try_wait()
                .map_err(|e| format!("wait for QEMU: {e}"))?
            {
                break status;
            }
            thread::sleep(Duration::from_millis(20));
        };
        for reader in child.readers.drain(..) {
            let _ = reader.join();
        }
        let _ = fs::remove_file(debug_socket);
        if status.success() || shutdown::requested() {
            Ok(())
        } else {
            Err(format!("QEMU exited with {status}"))
        }
    }

    fn spawn(&self, debug_socket: Option<&Path>) -> Result<ManagedQemuChild, String> {
        self.spawn_with_launch(debug_socket, &LaunchConfig::default())
    }

    fn spawn_with_launch(
        &self,
        debug_socket: Option<&Path>,
        launch: &LaunchConfig,
    ) -> Result<ManagedQemuChild, String> {
        launch.check_host_display(&self.qemu)?;
        if self.artifacts.x86_secure_firmware.is_some() {
            return x86_secure::spawn(self, debug_socket, launch);
        }
        let mut command = Command::new(&self.qemu);
        host_activity::configure(&mut command)?;
        let (mut rpmb, rpmb_socket) = if self.artifacts.development {
            (None, self.workdir.join("unused-rpmb.sock"))
        } else {
            let (child, socket) = self.spawn_rpmb()?;
            (Some(child), socket)
        };
        let x86 = self.artifacts.architecture == Architecture::X86_64;
        let arm_rpmb_proxy = if self.artifacts.architecture == Architecture::Aarch64
            && self.secure_firmware_dir.is_some()
        {
            Some(arm_rpmb_proxy::Proxy::start(
                &self.workdir,
                rpmb_socket.clone(),
            )?)
        } else {
            None
        };
        let idle_socket = self.workdir.join("debug-idle.sock");
        let serial_socket = debug_socket.unwrap_or(&idle_socket);
        if debug_socket.is_some() || x86 {
            remove_stale_socket(serial_socket)?;
        }
        command
            .arg("-machine")
            .arg(self.artifacts.machine_configuration())
            .arg("-cpu")
            .arg(self.artifacts.architecture.cpu())
            .arg("-smp")
            .arg(self.layout.max_cpus.to_string())
            .arg("-m")
            .arg(
                if self.artifacts.architecture == Architecture::X86_64 {
                    1024
                } else {
                    self.layout.ram_mib
                }
                .to_string(),
            );
        launch.configure_display(&mut command);
        if x86 {
            command.arg("-serial").arg("stdio");
            command.args([
                "-device",
                "virtio-serial-pci,disable-legacy=on,disable-modern=off,romfile=",
                "-device",
                "virtserialport,chardev=debug0,name=debug0,nr=1",
                "-chardev",
            ]);
            command.arg(format!(
                "socket,id=debug0,path={},server=on,wait=off",
                serial_socket.display()
            ));
            command.args(["-monitor", "none"]);
        } else {
            // Keep architectural diagnostics on PL011 and carry framed debug
            // RPC over its own heart-transplantable virtio-console instance.
            // Large firmware uploads must not be serialized one UART byte at
            // a time.
            command.arg("-serial").arg("stdio");
            command.args([
                "-device",
                "virtio-serial-pci,id=debugbus,disable-legacy=on,disable-modern=off,romfile=",
                "-device",
                "virtserialport,bus=debugbus.0,chardev=debug0,name=debug0,nr=1",
                "-chardev",
            ]);
            command.arg(format!(
                "socket,id=debug0,path={},server=on,wait=off",
                serial_socket.display()
            ));
            command.args(["-monitor", "none"]);
        }
        if self.artifacts.architecture == Architecture::Aarch64 {
            if self.secure_firmware_dir.is_some() {
                command.env("BEXOS_SECURE_FIRMWARE", self.firmware_disk_path());
            }
            aarch64::configure_boot(
                &mut command,
                self.secure_firmware_dir.as_deref(),
                &self.artifacts.kernel,
                arm_rpmb_proxy
                    .as_ref()
                    .map(|proxy| (proxy.boot_socket.as_path(), proxy.runtime_socket.as_path())),
            );
        }
        command
            .arg("-d")
            .arg("unimp")
            .arg("-drive")
            .arg(format!(
                "if=none,id=nvme0,file={},format=raw",
                self.disk.display()
            ))
            .arg("-device")
            .arg("nvme,drive=nvme0,serial=bexos-nvme0")
            .arg("-netdev")
            .arg("user,id=net0")
            .arg("-device")
            .arg("virtio-net-pci,disable-legacy=on,disable-modern=off,romfile=,netdev=net0,mac=52:54:00:12:34:56")
            .arg("-device")
            .arg(format!(
                "loader,file={},addr={},force-raw=on",
                self.artifacts.bootfs.display(),
                self.layout.bootfs_addr
            ))
            .arg("-device")
            .arg(format!(
                "loader,file={},addr={},force-raw=on",
                self.artifacts.handoff.display(),
                self.layout.handoff_addr
            ))
            .arg("-device")
            .arg(format!(
                "loader,file={},addr={},force-raw=on",
                self.artifacts.evidence.display(),
                self.layout.evidence_addr
            ))
            .arg("-device")
            .arg(format!(
                "loader,file={},addr={},force-raw=on",
                self.artifacts.vbmeta.display(),
                self.layout.vbmeta_addr
            ));
        if self.artifacts.architecture == Architecture::X86_64 {
            self.artifacts.architecture.configure_x86_boot(
                &mut command,
                self.artifacts.boot_image.as_ref().unwrap(),
                &[
                    &self.artifacts.bootfs,
                    &self.artifacts.handoff,
                    &self.artifacts.evidence,
                    &self.artifacts.vbmeta,
                ],
            );
        }
        launch.append_devices(&mut command);
        command.args(&self.artifacts.extra_args);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        eprintln!("qemu: command {command:?}");
        match command.spawn() {
            Ok(qemu) => {
                let mut child = ManagedQemuChild {
                    qemu,
                    rpmb,
                    _idle_serial: None,
                    readers: Vec::new(),
                    secure_relay: None,
                    arm_rpmb_proxy,
                };
                if debug_socket.is_none() {
                    child._idle_serial = Some(connect_socket(serial_socket)?);
                }
                Ok(child)
            }
            Err(error) => {
                if let Some(child) = &mut rpmb {
                    kill_child(child);
                }
                Err(format!("spawn QEMU: {error}"))
            }
        }
    }

    fn spawn_rpmb(&self) -> Result<(Child, PathBuf), String> {
        let socket = self.workdir.join("rpmb.sock");
        let _ = fs::remove_file(&socket);
        let mut child = Command::new(&self.artifacts.rpmbd)
            .arg("--dev")
            .arg(&self.rpmb)
            .arg("--sock")
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn Trusty RPMB proxy: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if socket.exists() {
                return Ok((child, socket));
            }
            let status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    kill_child(&mut child);
                    return Err(format!("poll Trusty RPMB proxy: {error}"));
                }
            };
            if let Some(status) = status {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                return Err(format!(
                    "Trusty RPMB proxy exited before ready: {status}: {}",
                    stderr.trim()
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
        kill_child(&mut child);
        Err("Trusty RPMB proxy did not create its socket".into())
    }
}

fn canonicalize_artifacts(artifacts: &mut QemuArtifacts) -> Result<(), String> {
    if let Some(firmware) = &mut artifacts.x86_secure_firmware {
        firmware.canonicalize()?;
    }
    if let Some(image) = &mut artifacts.boot_image {
        *image = image
            .canonicalize()
            .map_err(|e| format!("resolve x86 boot image: {e}"))?;
    }
    for path in [
        &mut artifacts.kernel,
        &mut artifacts.disk,
        &mut artifacts.bootfs,
        &mut artifacts.handoff,
        &mut artifacts.evidence,
        &mut artifacts.vbmeta,
        &mut artifacts.avb_public_key,
        &mut artifacts.layout,
        &mut artifacts.inspector,
        &mut artifacts.key,
        &mut artifacts.rpmbd,
        &mut artifacts.rpmb_template,
    ] {
        let original = path.clone();
        *path = fs::canonicalize(&original)
            .map_err(|e| format!("resolve QEMU artifact {}: {e}", original.display()))?;
    }
    if let Some(firmware) = &mut artifacts.secure_firmware {
        for path in [
            &mut firmware.bl1,
            &mut firmware.bl2,
            &mut firmware.bl31,
            &mut firmware.bl32,
            &mut firmware.bl33,
        ] {
            let original = path.clone();
            *path = fs::canonicalize(&original).map_err(|e| {
                format!(
                    "resolve secure-firmware artifact {}: {e}",
                    original.display()
                )
            })?;
        }
    }
    Ok(())
}

fn verify_boot_artifacts(artifacts: &QemuArtifacts) -> Result<(), String> {
    if artifacts.x86_secure_firmware.is_some() {
        // The monitor constructs evidence from its verified execution chain.
        // Host preflight is only a convenience check of the supplied images.
        let read =
            |path: &Path| fs::read(path).map_err(|e| format!("read {}: {e}", path.display()));
        return bexos_secure_monitor::boot_verify::verify(
            &read(&artifacts.vbmeta)?,
            &read(&artifacts.avb_public_key)?,
            &read(&artifacts.kernel)?,
            &read(&artifacts.bootfs)?,
        )
        .map(|_| ())
        .map_err(|e| format!("x86 boot payload preflight: {e:?}"));
    }
    let evidence_bytes =
        fs::read(&artifacts.evidence).map_err(|e| format!("read verified-boot evidence: {e}"))?;
    let evidence = BootEvidenceV1::decode(&evidence_bytes)
        .ok_or_else(|| "invalid verified-boot evidence".to_string())?;
    let mut signed = [0; BootEvidenceV1::SIGNED_BYTES];
    evidence.encode_unsigned(&mut signed);
    verify_ed25519(&evidence.public_key, &signed, &evidence.signature)
        .map_err(|_| "verified-boot evidence signature rejected".to_string())?;
    let kernel = fs::read(&artifacts.kernel).map_err(|e| format!("read kernel: {e}"))?;
    if <[u8; 32]>::from(Sha256::digest(&kernel)) != evidence.kernel_sha256 {
        return Err("verified-boot kernel digest mismatch".into());
    }
    let bootfs_bytes = fs::read(&artifacts.bootfs).map_err(|e| format!("read BootFS: {e}"))?;
    if <[u8; 32]>::from(Sha256::digest(&bootfs_bytes)) != evidence.bootfs_sha256 {
        return Err("verified-boot BootFS digest mismatch".into());
    }
    let bootfs = Bootfs::parse(&bootfs_bytes).map_err(|_| "parse verified BootFS".to_string())?;
    let policy = bootfs
        .find("/boot/platform.pcfg")
        .map_err(|_| "parse BootFS policy entry".to_string())?
        .ok_or_else(|| "verified BootFS has no platform policy".to_string())?;
    if <[u8; 32]>::from(Sha256::digest(policy.bytes)) != evidence.policy_sha256 {
        return Err("verified-boot platform-policy digest mismatch".into());
    }
    let x86_monitor_evidence = artifacts.architecture == Architecture::X86_64
        && evidence.flags
            & (BOOT_EVIDENCE_FLAG_SECURE_BOOT
                | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK
                | BOOT_EVIDENCE_FLAG_IOMMU_STRICT
                | BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR)
            == (BOOT_EVIDENCE_FLAG_SECURE_BOOT
                | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK
                | BOOT_EVIDENCE_FLAG_IOMMU_STRICT
                | BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR);
    if evidence.flags & (BOOT_EVIDENCE_FLAG_SECURE_BOOT | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK)
        != 0
        && !x86_monitor_evidence
    {
        return Err("host-provided evidence must not assert secure boot or RPMB".into());
    }
    let vbmeta = fs::read(&artifacts.vbmeta).map_err(|e| format!("read vbmeta: {e}"))?;
    let public_key =
        fs::read(&artifacts.avb_public_key).map_err(|e| format!("read AVB public key: {e}"))?;
    let modulus = public_key
        .get(8..264)
        .ok_or_else(|| "invalid AVB public key".to_string())?;
    let verified = verify_vbmeta(&vbmeta, modulus).map_err(|e| format!("verify vbmeta: {e:?}"))?;
    bexos_avb::verify_partition(
        verified
            .hash_descriptor(b"kernel")
            .map_err(|e| format!("kernel descriptor: {e:?}"))?,
        &kernel,
    )
    .map_err(|e| format!("kernel AVB digest: {e:?}"))?;
    bexos_avb::verify_partition(
        verified
            .hash_descriptor(b"bootfs")
            .map_err(|e| format!("BootFS descriptor: {e:?}"))?,
        &bootfs_bytes,
    )
    .map_err(|e| format!("BootFS AVB digest: {e:?}"))?;
    bexos_avb::verify_partition(
        verified
            .hash_descriptor(b"platform-policy")
            .map_err(|e| format!("policy descriptor: {e:?}"))?,
        policy.bytes,
    )
    .map_err(|e| format!("policy AVB digest: {e:?}"))?;
    Ok(())
}

struct ManagedQemuChild {
    qemu: Child,
    rpmb: Option<Child>,
    _idle_serial: Option<UnixStream>,
    readers: Vec<JoinHandle<()>>,
    secure_relay: Option<rpmb_relay::Relay>,
    arm_rpmb_proxy: Option<arm_rpmb_proxy::Proxy>,
}

fn prepare_rpmb_image(template: &Path, state: &Path) -> Result<(), String> {
    if !state.exists() {
        if let Some(parent) = state.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create RPMB state directory: {e}"))?;
        }
        fs::copy(template, state).map_err(|e| {
            format!(
                "copy RPMB image {} to {}: {e}",
                template.display(),
                state.display()
            )
        })?;
    }
    // Bazel outputs are deliberately read-only and `fs::copy` preserves their
    // mode. The proxy must update the per-instance authenticated image.
    let mut permissions = fs::metadata(state)
        .map_err(|e| format!("stat RPMB state {}: {e}", state.display()))?
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(state, permissions)
        .map_err(|e| format!("make RPMB state writable {}: {e}", state.display()))?;
    Ok(())
}

impl Drop for ManagedQemuChild {
    fn drop(&mut self) {
        kill_child(&mut self.qemu);
        self.secure_relay.take();
        self.arm_rpmb_proxy.take();
        if let Some(child) = &mut self.rpmb {
            kill_child(child);
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        QemuArtifacts, RAW_QEMU_AARCH64_MACHINE, SECURE_QEMU_AARCH64_MACHINE, prepare_rpmb_image,
        qemu_aarch64_machine_configuration,
    };
    use std::fs;

    #[test]
    fn developer_readiness_waits_for_shell_dependencies() {
        let mut output = super::DEBUGD_READY.to_vec();
        assert!(!super::debugd_ready(
            &output,
            super::DEBUGD_SHELL_DEPENDENCIES
        ));
        output.extend_from_slice(super::DEBUGD_SHELL_DEPENDENCIES[0]);
        assert!(!super::debugd_ready(
            &output,
            super::DEBUGD_SHELL_DEPENDENCIES
        ));
        output.extend_from_slice(super::DEBUGD_SHELL_DEPENDENCIES[1]);
        assert!(!super::debugd_ready(
            &output,
            super::DEBUGD_SHELL_DEPENDENCIES
        ));
        output.extend_from_slice(super::DEBUGD_SHELL_DEPENDENCIES[2]);
        assert!(super::debugd_ready(
            &output,
            super::DEBUGD_SHELL_DEPENDENCIES
        ));
        let dependencies_only = super::DEBUGD_SHELL_DEPENDENCIES.concat();
        assert!(!super::debugd_ready(
            &dependencies_only,
            super::DEBUGD_SHELL_DEPENDENCIES
        ));
    }

    fn artifacts_with_secure_firmware(secure_firmware: bool) -> QemuArtifacts {
        QemuArtifacts {
            architecture: super::Architecture::Aarch64,
            development: false,
            boot_image: None,
            kernel: "kernel".into(),
            disk: "disk".into(),
            bootfs: "bootfs".into(),
            handoff: "handoff".into(),
            evidence: "evidence".into(),
            vbmeta: "vbmeta".into(),
            avb_public_key: "avb-key".into(),
            layout: "layout".into(),
            inspector: "inspector".into(),
            key: "key".into(),
            rpmbd: "rpmb_dev".into(),
            rpmb_template: "RPMB_DATA".into(),
            extra_args: Vec::new(),
            x86_secure_firmware: None,
            secure_firmware: secure_firmware.then(|| super::SecureFirmware {
                bl1: "bl1.bin".into(),
                bl2: "bl2.bin".into(),
                bl31: "bl31.bin".into(),
                bl32: "lk.bin".into(),
                bl33: "bl33.bin".into(),
                bl33_certificate: None,
            }),
        }
    }

    #[test]
    fn rejection_requires_real_evidence_and_no_success_marker() {
        let evidence: &[&[u8]] = &[b"bl33: FATAL: kernel digest failed"];
        let forbidden: &[&[u8]] = &[b"userspace: entering el0 appd"];
        assert!(super::validate_rejection(b"", evidence, forbidden).is_err());
        assert!(super::validate_rejection(b"qemu: missing file", evidence, forbidden).is_err());
        assert!(
            super::validate_rejection(
                b"bl33: FATAL: kernel digest failed\nkernel: panic",
                evidence,
                forbidden,
            )
            .is_err()
        );
        assert!(
            super::validate_rejection(b"bl33: FATAL: kernel digest failed", evidence, forbidden)
                .is_ok()
        );
        assert!(
            super::validate_rejection(
                b"bl33: FATAL: kernel digest failed\nuserspace: entering el0 appd",
                evidence,
                forbidden
            )
            .is_err()
        );
    }

    #[test]
    fn rpmb_critical_exit_requires_mac_failure_and_cannot_hide_other_panics() {
        let mac =
            b"[trusty] read counter: Bad MAC\n[trusty] block_device_tipc_init_rpmb_key failed\n";
        let fatal = b"[trusty] panic (caller 0xffff1234): Unclean exit from critical app\n";
        assert_eq!(super::without_rpmb_critical_exit(fatal), fatal);
        let mut output = mac.to_vec();
        output.extend_from_slice(fatal);
        assert_eq!(super::without_rpmb_critical_exit(&output), mac);
        let mut arm = mac.to_vec();
        arm.extend_from_slice(
            b"secure os: panic (caller 0xffffabcd): Unclean exit from critical app\r\n",
        );
        assert_eq!(super::without_rpmb_critical_exit(&arm), mac);
        for unexpected in [
            b"kernel: panic\n".as_slice(),
            b"[trusty] panic (caller 0xffff): unexpected fault\n",
            b"[trusty] panic (caller 0xpanic): Unclean exit from critical app\n",
            b"secure os: panic (caller 0x): Unclean exit from critical app\n",
            b"secure os: panic (caller 0xffff): unrelated panic\n",
        ] {
            let mut mixed = output.clone();
            mixed.extend_from_slice(unexpected);
            assert!(super::contains(
                &super::without_rpmb_critical_exit(&mixed),
                b"panic"
            ));
        }
    }

    #[test]
    fn firmware_types_cannot_cross_guest_architectures() {
        let mut artifacts = artifacts_with_secure_firmware(true);
        assert!(artifacts.validate_architecture().is_ok());
        artifacts.architecture = super::Architecture::X86_64;
        assert!(artifacts.validate_architecture().is_err());
        artifacts.secure_firmware = None;
        assert!(artifacts.validate_architecture().is_err());
        artifacts.boot_image = Some("boot.elf".into());
        assert!(artifacts.validate_architecture().is_err());
        artifacts.development = true;
        assert!(artifacts.validate_architecture().is_ok());
        artifacts.architecture = super::Architecture::Aarch64;
        assert!(artifacts.validate_architecture().is_err());
        artifacts.boot_image = None;
        artifacts.development = false;
        artifacts.x86_secure_firmware = Some(super::X86SecureFirmware {
            code: "code.fd".into(),
            variables: "vars.fd".into(),
            loader: "loader.efi".into(),
        });
        assert!(artifacts.validate_architecture().is_err());
        artifacts.architecture = super::Architecture::X86_64;
        assert!(artifacts.validate_architecture().is_ok());
        assert_eq!(artifacts.machine_configuration(), "q35,accel=tcg,smm=on");
        artifacts.boot_image = Some("boot.elf".into());
        assert!(artifacts.validate_architecture().is_err());
    }

    #[test]
    fn raw_qemu_boot_uses_fake_psci_smc_machine() {
        assert_eq!(
            qemu_aarch64_machine_configuration(false),
            "virt,secure=off,virtualization=on,iommu=smmuv3,highmem-ecam=off,highmem-mmio=off"
        );
        assert_eq!(
            artifacts_with_secure_firmware(false).machine_configuration(),
            RAW_QEMU_AARCH64_MACHINE
        );
    }

    #[test]
    fn trusty_boot_uses_secure_firmware_machine() {
        assert_eq!(
            qemu_aarch64_machine_configuration(true),
            "virt,secure=on,virtualization=on,iommu=smmuv3,highmem-ecam=off,highmem-mmio=off"
        );
        assert_eq!(
            artifacts_with_secure_firmware(true).machine_configuration(),
            SECURE_QEMU_AARCH64_MACHINE
        );
    }

    #[test]
    fn rpmb_state_is_created_once_and_reused_across_boots() {
        let root = std::env::temp_dir().join(format!("bexos-rpmb-test-{}", std::process::id()));
        let template = root.join("template");
        let state = root.join("instance/RPMB_DATA");
        fs::create_dir_all(&root).unwrap();
        fs::write(&template, b"initial").unwrap();
        let mut template_permissions = fs::metadata(&template).unwrap().permissions();
        template_permissions.set_readonly(true);
        fs::set_permissions(&template, template_permissions).unwrap();
        prepare_rpmb_image(&template, &state).unwrap();
        fs::write(&state, b"persisted").unwrap();
        prepare_rpmb_image(&template, &state).unwrap();
        assert_eq!(fs::read(&state).unwrap(), b"persisted");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fragmented_boot_failure_preserves_error_detail() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        let mut owned = super::ManagedQemuChild {
            qemu: std::process::Command::new("/bin/sleep")
                .arg("60")
                .spawn()
                .unwrap(),
            rpmb: None,
            _idle_serial: None,
            readers: Vec::new(),
            secure_relay: None,
            arm_rpmb_proxy: None,
        };
        let (mut serial, mut producer) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            producer.write_all(b"appd: boot failed:").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            producer
                .write_all(b" users storage initialization response ErrTimedOut\n")
                .unwrap();
            producer
        });
        let mut output = Vec::new();
        super::read_debug_boot(
            &mut owned.qemu,
            &mut serial,
            &mut output,
            &[b"boot complete"],
            &Arc::new(Mutex::new(Vec::new())),
            60,
        )
        .unwrap();
        writer.join().unwrap();
        assert_eq!(
            output,
            b"appd: boot failed: users storage initialization response ErrTimedOut\n"
        );
    }

    #[test]
    fn managed_children_are_reaped_on_success_error_and_unwind() {
        use std::process::{Command, Stdio};
        for outcome in 0..3 {
            let mut qemu = Command::new("/bin/sleep").arg("60").spawn().unwrap();
            let rpmb = match Command::new("/bin/sleep").arg("60").spawn() {
                Ok(child) => child,
                Err(error) => {
                    super::kill_child(&mut qemu);
                    panic!("spawn cleanup test child: {error}");
                }
            };
            let ids = [qemu.id(), rpmb.id()];
            let owned = super::ManagedQemuChild {
                qemu,
                rpmb: Some(rpmb),
                _idle_serial: None,
                readers: Vec::new(),
                secure_relay: None,
                arm_rpmb_proxy: None,
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                move || -> Result<(), &str> {
                    let _owned = owned;
                    match outcome {
                        0 => Ok(()),
                        1 => Err("simulated boot timeout"),
                        _ => panic!("simulated test failure"),
                    }
                },
            ));
            assert_eq!(result.is_err(), outcome == 2);
            for pid in ids {
                assert!(
                    !Command::new("/bin/kill")
                        .args(["-0", &pid.to_string()])
                        .stderr(Stdio::null())
                        .status()
                        .unwrap()
                        .success()
                );
            }
        }
    }
}

impl Drop for QemuDevice {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.workdir);
    }
}

impl E2eDevice for QemuDevice {
    type DebugTransport = QemuDebugTransport;

    fn boot(&mut self, markers: &[&[u8]]) -> Result<Vec<u8>, String> {
        let mut child = self.spawn(None)?;
        let mut output = Vec::new();
        read_until_done(&mut child.qemu, None, &mut output, &[])?;
        validate_boot(&output, markers)?;
        Ok(output)
    }

    fn boot_with_debugd(
        &mut self,
        markers: &[&[u8]],
    ) -> Result<DebugSession<Self::DebugTransport>, String> {
        let socket = self.workdir.join("debugd.sock");
        eprintln!("e2e: qemu spawn");
        let mut child = self.spawn(Some(&socket))?;
        eprintln!("e2e: qemu connect debug socket");
        let mut serial = match connect_socket(&socket) {
            Ok(serial) => serial,
            Err(error) => {
                kill_child(&mut child.qemu);
                let diagnostics = collect_child_output(&mut child.qemu);
                return Err(format!(
                    "{error}; QEMU output: {}",
                    String::from_utf8_lossy(&diagnostics).trim()
                ));
            }
        };
        eprintln!("e2e: qemu read boot markers");
        let mut output = Vec::new();
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        if let Some(stdout) = child.qemu.stdout.take() {
            child
                .readers
                .push(spawn_reader(stdout, diagnostics.clone()));
        }
        if let Some(stderr) = child.qemu.stderr.take() {
            child
                .readers
                .push(spawn_reader(stderr, diagnostics.clone()));
        }
        read_debug_boot(
            &mut child.qemu,
            &mut serial,
            &mut output,
            markers,
            &diagnostics,
            60,
        )?;
        eprintln!("e2e: qemu boot markers read");
        validate_boot(&output, markers)?;
        if !contains(&output, DEBUGD_READY) {
            return Err("debugd did not report QEMU socket readiness".into());
        }
        let cursor = diagnostics
            .lock()
            .map_err(|_| "QEMU output lock poisoned")?
            .len();
        let transport = QemuDebugTransport {
            socket: UnixSocketTransport::from_stream(serial),
            diagnostics,
            cursor,
        };
        Ok(DebugSession::new(
            output,
            DebugClient::new(transport),
            Some(Box::new(QemuChildGuard { child: Some(child) })),
        ))
    }

    fn inspect_path(
        &self,
        partition: &str,
        label: &str,
        path: &str,
        extra: &[&str],
    ) -> Result<(), String> {
        let status = Command::new(&self.artifacts.inspector)
            .arg("--inspect")
            .arg("--image")
            .arg(&self.disk)
            .arg("--key-file")
            .arg(&self.artifacts.key)
            .arg("--partition")
            .arg(partition)
            .arg("--label")
            .arg(label)
            .arg("--path")
            .arg(path)
            .args(extra)
            .status()
            .map_err(|e| format!("run image inspector: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("image inspector failed for {partition}:{path}"))
        }
    }
}

fn validate_rejection(
    output: &[u8],
    rejection: &[&[u8]],
    forbidden: &[&[u8]],
) -> Result<(), String> {
    if rejection.is_empty() {
        return Err("negative boot test requires rejection evidence".into());
    }
    assert_markers(output, rejection)?;
    for marker in forbidden {
        assert_absent(output, marker, "rejected boot crossed its security gate")?;
    }
    assert_absent(output, b"panic", "unrelated panic during negative boot")?;
    assert_absent(
        output,
        b"unhandled synchronous exception",
        "unrelated Trusty exception during negative boot",
    )?;
    assert_absent(
        output,
        b"guest fault",
        "unrelated guest fault during negative boot",
    )
}

fn validate_boot(output: &[u8], markers: &[&[u8]]) -> Result<(), String> {
    assert_markers(output, markers).map_err(|error| {
        // COM1 is collected separately from the debug socket. Retain its tail
        // when boot stops before debugd, so persistent-reboot failures expose
        // the guest error instead of only listing the missing success markers.
        let tail = &output[output.len().saturating_sub(8192)..];
        format!(
            "{error}\nQEMU boot output tail:\n{}",
            String::from_utf8_lossy(tail)
        )
    })?;
    assert_absent(output, b"panic", "guest panic during QEMU boot")?;
    assert_absent(output, b"guest fault", "guest fault during QEMU boot")
}

fn read_until_done(
    child: &mut Child,
    serial: Option<&mut UnixStream>,
    output: &mut Vec<u8>,
    stop_markers: &[&[u8]],
) -> Result<(), String> {
    read_until_done_mode(child, serial, output, stop_markers, false)
}

fn without_rpmb_critical_exit(output: &[u8]) -> Vec<u8> {
    let authenticated_failure = contains(output, b": Bad MAC")
        && contains(output, b"block_device_tipc_init_rpmb_key failed");
    output
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| {
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let caller = [
                b"[trusty] panic (caller 0x".as_slice(),
                b"secure os: panic (caller 0x",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(*prefix))
            .and_then(|rest| rest.strip_suffix(b"): Unclean exit from critical app"));
            !(authenticated_failure
                && caller.is_some_and(|address| {
                    !address.is_empty() && address.iter().all(u8::is_ascii_hexdigit)
                }))
        })
        .flatten()
        .copied()
        .collect()
}

fn read_until_done_mode(
    child: &mut Child,
    mut serial: Option<&mut UnixStream>,
    output: &mut Vec<u8>,
    stop_markers: &[&[u8]],
    rpmb_critical_exit: bool,
) -> Result<(), String> {
    let timeout = std::env::var("BEXOS_QEMU_TIMEOUT_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let shared = Arc::new(Mutex::new(Vec::new()));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, shared.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, shared.clone()));
    }
    if let Some(serial) = serial.as_mut() {
        readers.push(spawn_reader(
            serial
                .try_clone()
                .map_err(|e| format!("clone debug serial: {e}"))?,
            shared.clone(),
        ));
    }
    while Instant::now() < deadline {
        {
            let current = shared.lock().map_err(|_| "QEMU output lock poisoned")?;
            output.clear();
            output.extend_from_slice(&current);
            if (!stop_markers.is_empty() && stop_markers.iter().all(|m| contains(output, m)))
                || contains(
                    output,
                    b"guest persistence and disk-only application verified",
                )
                || (!rpmb_critical_exit && contains(output, b"panic"))
                || contains(output, b"guest fault")
                || (stop_markers.is_empty()
                    && (contains(output, b"boot failed:")
                        || contains(output, b"bl33: FATAL:")
                        || secure_boot_rejected(output)))
            {
                break;
            }
        }
        if child
            .try_wait()
            .map_err(|e| format!("poll QEMU: {e}"))?
            .is_some()
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if child
        .try_wait()
        .map_err(|e| format!("poll QEMU: {e}"))?
        .is_none()
    {
        let _ = child.kill();
    }
    let _ = child.wait();
    for reader in readers {
        let _ = reader.join();
    }
    let current = shared.lock().map_err(|_| "QEMU output lock poisoned")?;
    output.clear();
    output.extend_from_slice(&current);
    eprint!("{}", String::from_utf8_lossy(output));
    Ok(())
}

fn read_debug_boot(
    child: &mut Child,
    serial: &mut UnixStream,
    output: &mut Vec<u8>,
    markers: &[&[u8]],
    diagnostics: &Arc<Mutex<Vec<u8>>>,
    default_timeout_seconds: u64,
) -> Result<(), String> {
    let timeout = std::env::var("BEXOS_QEMU_DEBUG_BOOT_TIMEOUT_SECONDS")
        .or_else(|_| std::env::var("BEXOS_QEMU_TIMEOUT_SECONDS"))
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default_timeout_seconds);
    serial
        .set_nonblocking(true)
        .map_err(|e| format!("set debug serial nonblocking: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let mut buf = [0u8; 8192];
    let mut ready_since = None;
    let mut failure_since = None;
    let mut diagnostic_cursor = 0;
    let mut timed_out = true;
    let mut terminated = false;
    while Instant::now() < deadline {
        if shutdown::requested() {
            return Err(format!(
                "QEMU launch interrupted\n{}",
                diagnostic_tail(output)
            ));
        }
        {
            let log = diagnostics
                .lock()
                .map_err(|_| "QEMU output lock poisoned")?;
            output.extend_from_slice(&log[diagnostic_cursor..]);
            diagnostic_cursor = log.len();
        }
        if ready_since.is_none() && debugd_ready(output, markers) {
            ready_since = Some(Instant::now());
        }
        if let Some(start) = boot_failure_start(output) {
            let since = failure_since.get_or_insert_with(Instant::now);
            // COM1 often delivers a diagnostic one byte at a time. Preserve
            // the error detail after the marker before the owner reaps QEMU.
            if output[start..].contains(&b'\n') || since.elapsed() >= Duration::from_secs(2) {
                timed_out = false;
                break;
            }
        }
        match serial.read(&mut buf) {
            Ok(0) => {
                timed_out = false;
                terminated = true;
                break;
            }
            Ok(n) => {
                eprint!("{}", String::from_utf8_lossy(&buf[..n]));
                output.extend_from_slice(&buf[..n]);
                if debugd_ready(output, markers) {
                    ready_since = Some(Instant::now());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if ready_since.is_some_and(|ready| ready.elapsed() >= Duration::from_secs(2)) {
                    timed_out = false;
                    break;
                }
            }
            Err(e) => return Err(format!("read debug serial: {e}")),
        }
        if child
            .try_wait()
            .map_err(|e| format!("poll QEMU: {e}"))?
            .is_some()
        {
            timed_out = false;
            terminated = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if timed_out {
        return Err(format!(
            "QEMU debug boot marker timeout after {timeout} s\n{}",
            diagnostic_tail(output)
        ));
    }
    if terminated && ready_since.is_none() {
        thread::sleep(Duration::from_millis(50));
        let log = diagnostics
            .lock()
            .map_err(|_| "QEMU output lock poisoned")?;
        output.extend_from_slice(&log[diagnostic_cursor..]);
        let status = child
            .try_wait()
            .map_err(|e| format!("read QEMU exit status: {e}"))?;
        return Err(format!(
            "QEMU exited before debug boot markers status={status:?}\n{}",
            diagnostic_tail(output)
        ));
    }
    serial
        .set_nonblocking(false)
        .map_err(|e| format!("restore debug serial blocking mode: {e}"))?;
    serial
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("set debug serial read timeout: {e}"))?;
    serial
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("set debug serial write timeout: {e}"))?;
    Ok(())
}

fn diagnostic_tail(output: &[u8]) -> String {
    const MAX_TAIL: usize = 16 * 1024;
    let start = output.len().saturating_sub(MAX_TAIL);
    String::from_utf8_lossy(&output[start..]).into_owned()
}

fn boot_failure_start(output: &[u8]) -> Option<usize> {
    [
        b"panic".as_slice(),
        b"guest fault",
        b"boot failed:",
        b"bl33: FATAL:",
    ]
    .into_iter()
    .chain(SECURE_BOOT_REJECTIONS.iter().copied())
    .filter_map(|marker| {
        output
            .windows(marker.len())
            .position(|bytes| bytes == marker)
    })
    .min()
}

fn wait_for_debugd_ready(
    child: &mut Child,
    serial: &mut UnixStream,
    diagnostics: &Arc<Mutex<Vec<u8>>>,
) -> Result<(), String> {
    let mut output = Vec::new();
    // The socket is published early in boot. Shell commands also need appd's
    // installed-provider registry and usersd, which arrive after storage startup.
    read_debug_boot(
        child,
        serial,
        &mut output,
        DEBUGD_SHELL_DEPENDENCIES,
        diagnostics,
        600,
    )?;
    validate_partial_boot(&output)?;
    if debugd_ready(&output, DEBUGD_SHELL_DEPENDENCIES) {
        Ok(())
    } else {
        Err("timed out waiting for debugd and shell dependencies".into())
    }
}

fn debugd_ready(output: &[u8], markers: &[&[u8]]) -> bool {
    contains(output, DEBUGD_READY) && markers.iter().all(|m| contains(output, m))
}

fn validate_partial_boot(output: &[u8]) -> Result<(), String> {
    for marker in SECURE_BOOT_REJECTIONS {
        assert_absent(output, marker, "authenticated x86 boot rejected")?;
    }
    assert_absent(output, b"panic", "guest panic during QEMU boot")?;
    assert_absent(output, b"guest fault", "guest fault during QEMU boot")?;
    assert_absent(output, b"boot failed:", "guest boot failed")?;
    assert_absent(output, b"bl33: FATAL:", "authenticated BL33 rejected boot")
}

/// COM1 diagnostics and virtio-console RPC are separate Q35 channels.
pub struct QemuDebugTransport {
    socket: UnixSocketTransport,
    diagnostics: Arc<Mutex<Vec<u8>>>,
    cursor: usize,
}
impl DebugTransport for QemuDebugTransport {
    fn read_diagnostics(&mut self) -> Vec<u8> {
        let bytes = self
            .diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let out = bytes[self.cursor..].to_vec();
        self.cursor = bytes.len();
        out
    }
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.socket.write_all(bytes)
    }
    fn read_chunk(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket.read_chunk(bytes)
    }
}

struct QemuChildGuard {
    child: Option<ManagedQemuChild>,
}

impl DebugSessionGuard for QemuChildGuard {}

impl Drop for QemuChildGuard {
    fn drop(&mut self) {
        let _ = self.child.take();
    }
}

fn kill_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn collect_child_output(child: &mut Child) -> Vec<u8> {
    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut output);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_end(&mut output);
    }
    output
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    output: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<()> {
    let live_log = std::env::var_os("BEXOS_QEMU_LIVE_LOG").is_some();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if live_log {
                        eprint!("{}", String::from_utf8_lossy(&buf[..n]));
                    }
                    if let Ok(mut output) = output.lock() {
                        output.extend_from_slice(&buf[..n]);
                    } else {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn spawn_passthrough_reader<R: Read + Send + 'static>(
    mut reader: R,
    output: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    eprint!("{}", String::from_utf8_lossy(&buf[..n]));
                    let _ = std::io::stderr().flush();
                    if let Ok(mut output) = output.lock() {
                        output.extend_from_slice(&buf[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path).map_err(|e| format!("remove stale debugd socket: {e}"))
        }
        Ok(_) => Err(format!(
            "debugd socket path exists and is not a UNIX socket: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("stat debugd socket path: {error}")),
    }
}

fn connect_socket(path: &Path) -> Result<UnixStream, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = None;
    while Instant::now() < deadline {
        if shutdown::requested() {
            return Err("QEMU launch interrupted".into());
        }
        match UnixStream::connect(path) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(20)))
                    .map_err(|e| format!("set debugd read timeout: {e}"))?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(20)))
                    .map_err(|e| format!("set debugd write timeout: {e}"))?;
                return Ok(stream);
            }
            Err(e) => {
                last = Some(e);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(format!(
        "connect debugd socket {}: {last:?}",
        path.display()
    ))
}

fn parse_layout(path: &Path) -> Result<Layout, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read layout: {e}"))?;
    let mut layout = Layout {
        ram_mib: 0,
        bootfs_addr: 0,
        handoff_addr: 0,
        evidence_addr: 0,
        vbmeta_addr: 0,
        max_cpus: 0,
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = parse_int(value)?;
        match key {
            "RAM_MIB" => layout.ram_mib = value,
            "BOOTFS_ADDR" => layout.bootfs_addr = value,
            "HANDOFF_ADDR" => layout.handoff_addr = value,
            "BOOT_EVIDENCE_ADDR" => layout.evidence_addr = value,
            "VBMETA_ADDR" => layout.vbmeta_addr = value,
            "MAX_CPUS" => {
                layout.max_cpus =
                    u32::try_from(value).map_err(|_| format!("MAX_CPUS too large: {value}"))?
            }
            _ => {}
        }
    }
    if layout.ram_mib == 0
        || layout.bootfs_addr == 0
        || layout.handoff_addr == 0
        || layout.evidence_addr == 0
        || layout.vbmeta_addr == 0
        || layout.max_cpus == 0
    {
        return Err(format!("layout missing required fields: {layout:?}"));
    }
    Ok(layout)
}

fn parse_int(value: &str) -> Result<u64, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|e| format!("parse layout integer {value}: {e}"))
    } else {
        value
            .parse::<u64>()
            .map_err(|e| format!("parse layout integer {value}: {e}"))
    }
}

fn find_qemu(architecture: Architecture) -> Result<PathBuf, String> {
    if architecture == Architecture::Aarch64 {
        if let Some(path) = std::env::var_os("BEXOS_QEMU_AARCH64_BINARY") {
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(format!(
                    "configured ARM QEMU binary is missing: {}",
                    path.display()
                ));
            }
            return path
                .canonicalize()
                .map_err(|e| format!("resolve configured ARM QEMU binary: {e}"));
        }
    }
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    for dir in std::env::split_paths(&path) {
        let qemu = dir.join(architecture.emulator());
        if qemu.is_file() {
            return Ok(qemu);
        }
    }
    Err(format!(
        "{} is required; install QEMU and rerun this Bazel target",
        architecture.emulator()
    ))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Compatibility name for existing ARM harness consumers.
pub type QemuAarch64Device = QemuDevice;
