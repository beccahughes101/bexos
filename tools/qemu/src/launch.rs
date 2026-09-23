//! Host-only launch policy. The wire format is tools/qemu/launch.proto;
//! Bazel encodes the checked-in prototxt before the runner reads it.
use crate::Architecture;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Product {
    #[default]
    Nongui,
    Workstation,
}

impl Product {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "nongui" => Ok(Self::Nongui),
            "workstation" => Ok(Self::Workstation),
            _ => Err(format!("unknown QEMU product: {value}")),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Nongui => "nongui",
            Self::Workstation => "workstation",
        }
    }
    pub fn debug_socket(self, architecture: Architecture) -> PathBuf {
        let arch = match architecture {
            Architecture::Aarch64 => "aarch64",
            Architecture::X86_64 => "x86_64",
        };
        format!("/tmp/bexos-qemu-{}-{arch}-debugd.sock", self.name()).into()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LaunchConfig {
    windowed: bool,
    devices: Vec<String>,
}

impl LaunchConfig {
    pub fn read(path: &Path) -> Result<Self, String> {
        let data = fs::read(path)
            .map_err(|e| format!("read QEMU launch config {}: {e}", path.display()))?;
        Self::decode(&data).map_err(|e| format!("QEMU launch config {}: {e}", path.display()))
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, String> {
        let mut config = Self::default();
        while !data.is_empty() {
            match varint(&mut data)? {
                8 => {
                    config.windowed = match varint(&mut data)? {
                        0 => false,
                        1 => true,
                        value => return Err(format!("unsupported display mode {value}")),
                    }
                }
                18 => {
                    let length =
                        usize::try_from(varint(&mut data)?).map_err(|_| "device name too long")?;
                    let bytes = data.get(..length).ok_or("truncated device name")?;
                    let device =
                        std::str::from_utf8(bytes).map_err(|_| "device name is not UTF-8")?;
                    if device.is_empty()
                        || !device
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    {
                        return Err("expected a QEMU device model name".into());
                    }
                    config.devices.push(device.to_owned());
                    data = &data[length..];
                }
                tag => return Err(format!("unsupported launch config field tag {tag}")),
            }
        }
        Ok(config)
    }

    pub(crate) fn check_host_display(&self, qemu: &Path) -> Result<(), String> {
        if !self.windowed {
            return Ok(());
        }
        let output = Command::new(qemu)
            .args(["-display", "help"])
            .output()
            .map_err(|e| format!("query QEMU UI backends: {e}"))?;
        let help = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || !has_native_ui(&help) {
            return Err(
                "workstation requires a QEMU build with a native UI backend (Cocoa, GTK, or SDL)"
                    .into(),
            );
        }
        Ok(())
    }

    pub(crate) fn configure_display(&self, command: &mut Command) {
        if self.windowed {
            // QEMU selects its native UI (Cocoa on macOS, GTK/SDL on Linux).
            // Suppress Q35's implicit VGA so the virtio GPU owns the display.
            command.args(["-vga", "none"]);
        } else {
            command.arg("-nographic");
        }
    }

    pub(crate) fn append_devices(&self, command: &mut Command) {
        for device in &self.devices {
            command.arg("-device").arg(device);
        }
    }
}

fn has_native_ui(help: &str) -> bool {
    help.lines()
        .any(|line| matches!(line.trim(), "cocoa" | "gtk" | "sdl"))
}

fn varint(data: &mut &[u8]) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = data.split_first().ok_or("truncated varint")?;
        *data = rest;
        if shift == 63 && byte > 1 {
            return Err("varint overflow".into());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("varint overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|s| s.to_str().unwrap().to_owned())
            .collect()
    }
    #[test]
    fn compiled_profiles_append_graphics_after_boot_devices() {
        for (profile, windowed) in [
            (
                include_bytes!(env!("NONGUI_LAUNCH_CONFIG")).as_slice(),
                false,
            ),
            (
                include_bytes!(env!("WORKSTATION_LAUNCH_CONFIG")).as_slice(),
                true,
            ),
        ] {
            let config = LaunchConfig::decode(profile).unwrap();
            assert_eq!(config.windowed, windowed);
            let mut cmd = Command::new("qemu");
            config.configure_display(&mut cmd);
            cmd.args(["-device", "nvme,drive=nvme0", "-device", "virtio-net-pci"]);
            let before = args(&cmd);
            config.append_devices(&mut cmd);
            let arguments = args(&cmd);
            assert_eq!(&arguments[..before.len()], before);
            assert_eq!(arguments.contains(&"-nographic".into()), !windowed);
            if windowed {
                assert_eq!(
                    &arguments[arguments.len() - 6..],
                    [
                        "-device",
                        "virtio-gpu-pci",
                        "-device",
                        "virtio-keyboard-pci",
                        "-device",
                        "virtio-mouse-pci"
                    ]
                );
                assert_eq!(&arguments[..2], ["-vga", "none"]);
            } else {
                assert_eq!(arguments, before);
            }
        }
    }
    #[test]
    fn headless_only_qemu_cannot_silently_launch_a_workstation() {
        assert!(!has_native_ui(
            "Available display backend types:\nnone\ncurses\ndbus\n"
        ));
        assert!(has_native_ui(
            "Available display backend types:\nnone\ncocoa\n"
        ));
        assert!(has_native_ui("none\ngtk\nsdl\n"));
    }
    #[test]
    fn malformed_profiles_fail_before_launch() {
        for data in [
            &[8, 2][..],
            &[18, 9, 1],
            &[18, 1, 255],
            &[0],
            &[8],
            &[18, 0],
            &[18, 1, b','],
            &[128; 11],
        ] {
            assert!(LaunchConfig::decode(data).is_err(), "{data:?}");
        }
        assert_eq!(LaunchConfig::decode(&[]).unwrap(), LaunchConfig::default());
    }
    #[test]
    fn product_and_architecture_sockets_are_distinct() {
        let mut sockets = std::collections::HashSet::new();
        for product in [Product::Nongui, Product::Workstation] {
            for arch in [Architecture::Aarch64, Architecture::X86_64] {
                assert!(sockets.insert(product.debug_socket(arch)));
            }
        }
        assert_eq!(
            Product::Workstation.debug_socket(Architecture::Aarch64),
            Path::new("/tmp/bexos-qemu-workstation-aarch64-debugd.sock")
        );
        assert!(Product::parse("invalid").is_err());
    }
}
