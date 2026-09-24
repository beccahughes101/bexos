use alloc::vec::Vec;

pub const PAGE_SIZE: u64 = 4096;
pub const PIE_LOAD_BIAS: u64 = 0x4000_0000;
pub const RIGHTS_READ: u32 = 0x0000_0002;
pub const RIGHTS_WRITE: u32 = 0x0000_0004;
pub const RIGHTS_EXECUTE: u32 = 0x0000_0008;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
use crate::arch::Machine;
const PT_LOAD: u32 = 1;
const PT_TLS: u32 = 7;
const PF_X: u32 = 0x1;
const PF_W: u32 = 0x2;
const PF_R: u32 = 0x4;
const MAX_PROGRAM_HEADERS: u16 = 32;
const ELF64_HEADER_SIZE: usize = 64;
const ELF64_PROGRAM_HEADER_SIZE: u16 = 56;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadPlan {
    pub entry_vaddr: u64,
    pub segments: Vec<LoadSegment>,
    pub tls: Option<TlsSegment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadSegment {
    pub file_offset: u64,
    pub file_size: u64,
    pub vaddr: u64,
    pub rights: u32,
    pub zero_fill: Option<ZeroFillSegment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroFillSegment {
    pub vaddr: u64,
    pub size_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlsSegment {
    pub file_offset: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfLoadError {
    TooSmall,
    BadMagic,
    UnsupportedClass,
    UnsupportedEndian,
    UnsupportedVersion,
    UnsupportedType,
    UnsupportedMachine,
    TooManyProgramHeaders,
    BadProgramHeaderSize,
    ProgramHeadersOutOfBounds,
    SegmentOutOfBounds,
    InvalidLoadSegment,
    WriteExecuteSegment,
    UnsupportedBssLayout,
    InvalidTlsSegment,
    TlsSegmentOutOfBounds,
    MissingExecutableSegment,
    EntryOutsideExecutableSegment,
    Overflow,
}

impl LoadPlan {
    pub fn parse_elf64_aarch64(bytes: &[u8]) -> Result<Self, ElfLoadError> {
        Self::parse(bytes, Machine::Aarch64)
    }

    pub fn parse(bytes: &[u8], machine: Machine) -> Result<Self, ElfLoadError> {
        Self::parse_with_bias(bytes, machine, PIE_LOAD_BIAS)
    }

    /// Parses an ELF image using the supplied load bias for `ET_DYN` images.
    /// `ET_EXEC` virtual addresses are never rebased.
    pub fn parse_with_bias(
        bytes: &[u8],
        machine: Machine,
        pie_load_bias: u64,
    ) -> Result<Self, ElfLoadError> {
        if bytes.len() < ELF64_HEADER_SIZE {
            return Err(ElfLoadError::TooSmall);
        }
        if &bytes[0..4] != b"\x7fELF" {
            return Err(ElfLoadError::BadMagic);
        }
        if bytes[4] != 2 {
            return Err(ElfLoadError::UnsupportedClass);
        }
        if bytes[5] != 1 {
            return Err(ElfLoadError::UnsupportedEndian);
        }
        if bytes[6] != 1 {
            return Err(ElfLoadError::UnsupportedVersion);
        }

        let elf_type = read_u16(bytes, 16)?;
        if !matches!(elf_type, ET_EXEC | ET_DYN) {
            return Err(ElfLoadError::UnsupportedType);
        }
        if read_u16(bytes, 18)? != machine.elf_machine() {
            return Err(ElfLoadError::UnsupportedMachine);
        }
        let entry = read_u64(bytes, 24)?;
        let program_header_offset = read_u64(bytes, 32)?;
        let program_header_entry_size = read_u16(bytes, 54)?;
        let program_header_count = read_u16(bytes, 56)?;
        if program_header_count > MAX_PROGRAM_HEADERS {
            return Err(ElfLoadError::TooManyProgramHeaders);
        }
        if program_header_entry_size != ELF64_PROGRAM_HEADER_SIZE {
            return Err(ElfLoadError::BadProgramHeaderSize);
        }

        let table_size = u64::from(program_header_entry_size)
            .checked_mul(u64::from(program_header_count))
            .ok_or(ElfLoadError::Overflow)?;
        let table_end = program_header_offset
            .checked_add(table_size)
            .ok_or(ElfLoadError::Overflow)?;
        if table_end as usize > bytes.len() {
            return Err(ElfLoadError::ProgramHeadersOutOfBounds);
        }

        let load_bias = if elf_type == ET_DYN { pie_load_bias } else { 0 };
        let mut segments = Vec::new();
        let mut tls = None;
        let mut has_executable_segment = false;
        let mut entry_is_executable = false;

        for index in 0..program_header_count {
            let offset = program_header_offset
                .checked_add(u64::from(index) * u64::from(program_header_entry_size))
                .ok_or(ElfLoadError::Overflow)? as usize;
            let segment_type = read_u32(bytes, offset)?;
            if segment_type == PT_TLS {
                let p_offset = read_u64(bytes, offset + 8)?;
                let p_filesz = read_u64(bytes, offset + 32)?;
                let p_memsz = read_u64(bytes, offset + 40)?;
                let p_align = read_u64(bytes, offset + 48)?;
                if tls.is_some()
                    || p_filesz > p_memsz
                    || (p_align > 1 && !p_align.is_power_of_two())
                {
                    return Err(ElfLoadError::InvalidTlsSegment);
                }
                if p_offset
                    .checked_add(p_filesz)
                    .is_none_or(|end| end as usize > bytes.len())
                {
                    return Err(ElfLoadError::TlsSegmentOutOfBounds);
                }
                if p_memsz != 0 {
                    tls = Some(TlsSegment {
                        file_offset: p_offset,
                        file_size: p_filesz,
                        mem_size: p_memsz,
                        align: p_align.max(1),
                    });
                }
                continue;
            }
            if segment_type != PT_LOAD {
                continue;
            }

            let flags = read_u32(bytes, offset + 4)?;
            if flags & PF_W != 0 && flags & PF_X != 0 {
                return Err(ElfLoadError::WriteExecuteSegment);
            }
            let p_offset = read_u64(bytes, offset + 8)?;
            let p_vaddr = read_u64(bytes, offset + 16)?;
            let p_filesz = read_u64(bytes, offset + 32)?;
            let p_memsz = read_u64(bytes, offset + 40)?;
            let p_align = read_u64(bytes, offset + 48)?;
            if p_memsz == 0 || p_filesz > p_memsz {
                return Err(ElfLoadError::InvalidLoadSegment);
            }
            if p_offset
                .checked_add(p_filesz)
                .is_none_or(|end| end as usize > bytes.len())
            {
                return Err(ElfLoadError::SegmentOutOfBounds);
            }

            if (p_align > 1
                && (!p_align.is_power_of_two() || p_vaddr % p_align != p_offset % p_align))
                || p_vaddr % PAGE_SIZE != p_offset % PAGE_SIZE
            {
                return Err(ElfLoadError::InvalidLoadSegment);
            }

            let biased_vaddr = p_vaddr
                .checked_add(load_bias)
                .ok_or(ElfLoadError::Overflow)?;
            let map_vaddr = align_down(biased_vaddr);
            let file_offset = align_down(p_offset);
            let leading = biased_vaddr
                .checked_sub(map_vaddr)
                .ok_or(ElfLoadError::Overflow)?;
            let file_size = leading
                .checked_add(p_filesz)
                .ok_or(ElfLoadError::Overflow)?;
            let file_map_size = align_up(file_size)?;
            let mem_size = align_up(leading.checked_add(p_memsz).ok_or(ElfLoadError::Overflow)?)?;
            let rights = segment_rights(flags);
            let map_end = map_vaddr
                .checked_add(mem_size)
                .ok_or(ElfLoadError::Overflow)?;
            for prior in &segments {
                let prior: &LoadSegment = prior;
                let prior_end = prior.zero_fill.map_or(
                    prior
                        .vaddr
                        .checked_add(align_up(prior.file_size)?)
                        .ok_or(ElfLoadError::Overflow)?,
                    |bss| bss.vaddr + bss.size_bytes,
                );
                if map_vaddr < prior_end && prior.vaddr < map_end {
                    return Err(ElfLoadError::InvalidLoadSegment);
                }
            }

            let zero_fill = if mem_size > file_map_size {
                Some(ZeroFillSegment {
                    vaddr: map_vaddr
                        .checked_add(file_map_size)
                        .ok_or(ElfLoadError::Overflow)?,
                    size_bytes: mem_size - file_map_size,
                })
            } else {
                None
            };

            if flags & PF_X != 0 {
                has_executable_segment = true;
                let biased_entry = entry.checked_add(load_bias).ok_or(ElfLoadError::Overflow)?;
                if biased_entry >= biased_vaddr
                    && biased_entry
                        < biased_vaddr
                            .checked_add(p_memsz)
                            .ok_or(ElfLoadError::Overflow)?
                {
                    entry_is_executable = true;
                }
            }

            if file_size != 0 || zero_fill.is_some() {
                segments.push(LoadSegment {
                    file_offset,
                    file_size,
                    vaddr: map_vaddr,
                    rights,
                    zero_fill,
                });
            }
        }

        if !has_executable_segment {
            return Err(ElfLoadError::MissingExecutableSegment);
        }
        if !entry_is_executable {
            return Err(ElfLoadError::EntryOutsideExecutableSegment);
        }

        Ok(Self {
            entry_vaddr: entry.checked_add(load_bias).ok_or(ElfLoadError::Overflow)?,
            segments,
            tls,
        })
    }
}

fn segment_rights(flags: u32) -> u32 {
    let mut rights = 0;
    if flags & PF_R != 0 {
        rights |= RIGHTS_READ;
    }
    if flags & PF_W != 0 {
        rights |= RIGHTS_WRITE;
    }
    if flags & PF_X != 0 {
        rights |= RIGHTS_EXECUTE;
    }
    rights
}

fn align_down(value: u64) -> u64 {
    value & !(PAGE_SIZE - 1)
}

fn align_up(value: u64) -> Result<u64, ElfLoadError> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
        .ok_or(ElfLoadError::Overflow)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfLoadError> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or(ElfLoadError::ProgramHeadersOutOfBounds)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfLoadError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or(ElfLoadError::ProgramHeadersOutOfBounds)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfLoadError> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or(ElfLoadError::ProgramHeadersOutOfBounds)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}
