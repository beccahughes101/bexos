use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Architecture {
    #[default]
    Aarch64,
    X86_64,
}
impl Architecture {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "aarch64" => Ok(Self::Aarch64),
            "x86_64" => Ok(Self::X86_64),
            _ => Err(format!("unsupported QEMU guest architecture: {value}")),
        }
    }
    pub fn from_env() -> Result<Self, String> {
        Self::parse(&std::env::var("BEXOS_QEMU_ARCH").unwrap_or_else(|_| "aarch64".into()))
    }
    pub fn emulator(self) -> &'static str {
        match self {
            Self::Aarch64 => "qemu-system-aarch64",
            Self::X86_64 => "qemu-system-x86_64",
        }
    }
    pub fn debug_socket(self) -> &'static str {
        match self {
            Self::Aarch64 => "/tmp/bexos-qemu-nongui-aarch64-debugd.sock",
            Self::X86_64 => "/tmp/bexos-qemu-nongui-x86_64-debugd.sock",
        }
    }
    pub fn cpu(self) -> &'static str {
        match self {
            // The maintained secure profile runs its permanent execution
            // owner at S-EL2 and therefore requires secure virtualization.
            Self::Aarch64 => "max",
            Self::X86_64 => "max",
        }
    }
    pub fn machine(self, secure: bool) -> &'static str {
        match self {
            Self::Aarch64 => super::qemu_aarch64_machine_configuration(secure),
            Self::X86_64 => "q35,accel=tcg",
        }
    }
    pub fn configure_x86_boot(self, command: &mut Command, image: &Path, modules: &[&Path]) {
        command.arg("-kernel").arg(image);
        if !modules.is_empty() {
            command.arg("-initrd").arg(
                modules
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn architectures_have_separate_emulators_and_sockets() {
        assert_ne!(
            Architecture::Aarch64.emulator(),
            Architecture::X86_64.emulator()
        );
        assert_ne!(
            Architecture::Aarch64.debug_socket(),
            Architecture::X86_64.debug_socket()
        );
        assert!(!Architecture::X86_64.machine(false).contains("secure="));
        assert!(Architecture::parse("amd64").is_err());
    }
}
