//! Architecture policy for compiled, signed package manifests.
#![no_std]
extern crate alloc;
mod wire;
use wire::{fields, varint};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Architecture {
    #[default]
    Multi = 0,
    Aarch64 = 1,
    X86_64 = 2,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Malformed,
    UnknownArchitecture,
    ConflictingArchitecture,
    NativeArchitectureRequired,
    IncompatibleArchitecture,
    InvalidElf,
}
impl Architecture {
    pub const fn from_proto(value: u64) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Multi),
            1 => Ok(Self::Aarch64),
            2 => Ok(Self::X86_64),
            _ => Err(Error::UnknownArchitecture),
        }
    }
    pub const fn current_guest() -> Self {
        if cfg!(bexos_arch_x86_64) {
            Self::X86_64
        } else {
            Self::Aarch64
        }
    }
    pub const fn compatible_with(self, target: Self) -> bool {
        matches!(self, Self::Multi) || self as u8 == target as u8
    }
    pub const fn elf_machine(self) -> Option<u16> {
        match self {
            Self::Multi => None,
            Self::Aarch64 => Some(183),
            Self::X86_64 => Some(62),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestArchitecture {
    pub declared: Option<Architecture>,
    pub native: bool,
}
impl ManifestArchitecture {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut result = Self {
            declared: None,
            native: false,
        };
        for field in fields(bytes) {
            let (number, kind, value) = field?;
            match number {
                20 => {
                    if kind != 0 || result.declared.is_some() {
                        return Err(Error::Malformed);
                    }
                    result.declared = Some(Architecture::from_proto(varint(value)?.0)?);
                }
                16 | 19 => {
                    if kind != 2 {
                        return Err(Error::Malformed);
                    }
                    result.native = true;
                }
                3 => {
                    if kind != 2 {
                        return Err(Error::Malformed);
                    }
                    for process in fields(value) {
                        let (number, kind, value) = process?;
                        if number == 2 {
                            if kind != 2 {
                                return Err(Error::Malformed);
                            }
                            result.native |= value.eq_ignore_ascii_case(b"elf");
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(result)
    }
    pub fn architecture(self) -> Architecture {
        self.declared.unwrap_or_default()
    }
    pub fn validate(self, target: Option<Architecture>) -> Result<Architecture, Error> {
        let arch = self.architecture();
        if self.native && arch == Architecture::Multi {
            return Err(Error::NativeArchitectureRequired);
        }
        if target.is_some_and(|target| !arch.compatible_with(target)) {
            return Err(Error::IncompatibleArchitecture);
        }
        Ok(arch)
    }
}

/// Operates only on unsigned compiler output, before archive construction.
pub fn stamp(bytes: &[u8], architecture: Architecture) -> Result<alloc::vec::Vec<u8>, Error> {
    let parsed = ManifestArchitecture::decode(bytes)?;
    if parsed.declared.is_some_and(|value| value != architecture) {
        return Err(Error::ConflictingArchitecture);
    }
    let mut output = bytes.to_vec();
    if parsed.declared.is_none() {
        output.extend_from_slice(&[0xa0, 0x01, architecture as u8]);
    }
    ManifestArchitecture::decode(&output)?.validate(None)?;
    Ok(output)
}

pub fn validate_payload(architecture: Architecture, bytes: &[u8]) -> Result<(), Error> {
    if !bytes.starts_with(b"\x7fELF") {
        return Ok(());
    }
    let machine = architecture
        .elf_machine()
        .ok_or(Error::NativeArchitectureRequired)?;
    if bytes.len() < 64 || bytes[4] != 2 || bytes[5] != 1 || bytes[6] != 1 {
        return Err(Error::InvalidElf);
    }
    if u16::from_le_bytes([bytes[18], bytes[19]]) != machine {
        return Err(Error::IncompatibleArchitecture);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
