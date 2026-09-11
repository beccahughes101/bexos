use crate::*;

pub(crate) fn parse_dynamic_library_headers(
    bytes: &[u8],
    machine: Machine,
) -> Result<
    (
        Vec<ProgramLoad>,
        u64,
        u64,
        Option<TlsSegment>,
        Vec<(u64, u64)>,
    ),
    ElfError,
> {
    validate_elf_header(bytes, machine)?;
    if !matches!(read_u16(bytes, 16)?, 2 | 3) {
        return Err(ElfError::UnsupportedType);
    }
    let phoff = read_u64(bytes, 32)?;
    let phentsize = read_u16(bytes, 54)?;
    let phnum = read_u16(bytes, 56)?;
    if phnum > 32 {
        return Err(ElfError::TooManyProgramHeaders);
    }
    if phentsize != 56 {
        return Err(ElfError::BadProgramHeaderSize);
    }
    let end = phoff
        .checked_add(u64::from(phentsize) * u64::from(phnum))
        .ok_or(ElfError::Overflow)?;
    if end as usize > bytes.len() {
        return Err(ElfError::ProgramHeadersOutOfBounds);
    }

    let mut loads = Vec::new();
    let mut dynamic = None;
    let mut tls = None;
    let mut relro = Vec::new();
    for index in 0..phnum {
        let offset = phoff
            .checked_add(u64::from(index) * u64::from(phentsize))
            .ok_or(ElfError::Overflow)? as usize;
        let kind = read_u32(bytes, offset)?;
        let flags = read_u32(bytes, offset + 4)?;
        if flags & 0x2 != 0 && flags & 0x1 != 0 {
            return Err(ElfError::WriteExecuteSegment);
        }
        let p_offset = read_u64(bytes, offset + 8)?;
        let p_vaddr = read_u64(bytes, offset + 16)?;
        let p_filesz = read_u64(bytes, offset + 32)?;
        let p_memsz = read_u64(bytes, offset + 40)?;
        let p_align = read_u64(bytes, offset + 48)?;
        if p_offset
            .checked_add(p_filesz)
            .is_none_or(|end| end as usize > bytes.len())
        {
            return Err(ElfError::SegmentOutOfBounds);
        }
        match kind {
            1 => {
                if p_memsz == 0 || p_filesz > p_memsz {
                    return Err(ElfError::InvalidLoadSegment);
                }
                if (p_align > 1
                    && (!p_align.is_power_of_two() || p_vaddr % p_align != p_offset % p_align))
                    || p_vaddr % PAGE_SIZE != p_offset % PAGE_SIZE
                {
                    return Err(ElfError::InvalidLoadSegment);
                }
                let map_vaddr = align_down(p_vaddr);
                let file_offset = align_down(p_offset);
                let leading = p_vaddr.checked_sub(map_vaddr).ok_or(ElfError::Overflow)?;
                let file_size = leading.checked_add(p_filesz).ok_or(ElfError::Overflow)?;
                let mem_size = leading.checked_add(p_memsz).ok_or(ElfError::Overflow)?;
                let end = map_vaddr
                    .checked_add(page_round(mem_size).ok_or(ElfError::Overflow)?)
                    .ok_or(ElfError::Overflow)?;
                for prior in &loads {
                    let prior: &ProgramLoad = prior;
                    let prior_end = prior
                        .vaddr
                        .checked_add(page_round(prior.mem_size).ok_or(ElfError::Overflow)?)
                        .ok_or(ElfError::Overflow)?;
                    if map_vaddr < prior_end && prior.vaddr < end {
                        return Err(ElfError::InvalidLoadSegment);
                    }
                }
                loads.push(ProgramLoad {
                    file_offset,
                    file_size,
                    mem_size,
                    vaddr: map_vaddr,
                    rights: dynamic_segment_rights(flags),
                });
            }
            2 => {
                if dynamic.replace((p_offset, p_filesz)).is_some() {
                    return Err(ElfError::UnsupportedDynamicEntry);
                }
            }
            7 => {
                if tls.is_some()
                    || p_filesz > p_memsz
                    || (p_align > 1 && !p_align.is_power_of_two())
                {
                    return Err(ElfError::InvalidTlsSegment);
                }
                tls = Some(TlsSegment {
                    file_offset: p_offset,
                    file_size: p_filesz,
                    mem_size: p_memsz,
                    align: p_align.max(1),
                });
            }
            0x6474_e552 => {
                let start = align_down(p_vaddr);
                let end = page_round(p_vaddr.checked_add(p_memsz).ok_or(ElfError::Overflow)?)
                    .ok_or(ElfError::Overflow)?;
                relro.push((start, end));
            }
            _ => {}
        }
    }
    let Some((dynamic_offset, dynamic_size)) = dynamic else {
        return Err(ElfError::MissingDynamicSection);
    };
    Ok((loads, dynamic_offset, dynamic_size, tls, relro))
}

pub fn dynamic_library_span(bytes: &[u8], machine: Machine) -> Result<(u64, u64, u64), ElfError> {
    let (loads, _, _, _, _) = parse_dynamic_library_headers(bytes, machine)?;
    let mut start = u64::MAX;
    let mut end = 0u64;
    let mut align = PAGE_SIZE;
    for load in loads {
        start = start.min(load.vaddr);
        end = end.max(
            load.vaddr
                .checked_add(load.mem_size)
                .ok_or(ElfError::Overflow)?,
        );
        align = align.max(PAGE_SIZE);
    }
    if start == u64::MAX || start >= end {
        return Err(ElfError::InvalidLoadSegment);
    }
    Ok((start, end, align))
}

fn validate_elf_header(bytes: &[u8], machine: Machine) -> Result<(), ElfError> {
    if bytes.len() < 64 {
        return Err(ElfError::TooSmall);
    }
    if &bytes[0..4] != b"\x7fELF" {
        return Err(ElfError::BadMagic);
    }
    if bytes[4] != 2 {
        return Err(ElfError::UnsupportedClass);
    }
    if bytes[5] != 1 {
        return Err(ElfError::UnsupportedEndian);
    }
    if bytes[6] != 1 {
        return Err(ElfError::UnsupportedVersion);
    }
    if read_u16(bytes, 18)? != machine.elf_machine() {
        return Err(ElfError::UnsupportedMachine);
    }
    Ok(())
}

pub(crate) fn parse_dynamic_info(
    bytes: &[u8],
    offset: u64,
    size: u64,
) -> Result<DynamicInfo, ElfError> {
    let mut info = DynamicInfo::default();
    let mut cursor = offset as usize;
    let end = offset.checked_add(size).ok_or(ElfError::Overflow)? as usize;
    if end > bytes.len() {
        return Err(ElfError::DynamicEntryOutOfBounds);
    }
    while cursor.checked_add(16).is_some_and(|next| next <= end) {
        let tag = read_u64(bytes, cursor)?;
        let value = read_u64(bytes, cursor + 8)?;
        cursor += 16;
        match tag {
            0 => break,
            1 => info.needed.push(value),
            2 => info.plt_rela_size = value,
            4 => info.hash = value,
            5 => info.strtab = value,
            6 => info.symtab = value,
            7 => info.rela = value,
            8 => info.rela_size = value,
            9 => info.rela_entry_size = value,
            10 => info.string_size = value,
            11 => info.symbol_entry_size = value,
            12 => info.init = value,
            14 => info.soname = value,
            20 => info.plt_rela_kind = value,
            23 => info.plt_rela = value,
            25 => info.init_array = value,
            27 => info.init_array_size = value,
            35 => info.relr_size = value,
            36 => info.relr = value,
            37 => info.relr_entry_size = value,
            0x16 => return Err(ElfError::TextRelocation),
            0x1e if value & 0x4 != 0 => return Err(ElfError::TextRelocation),
            0x3 | 0x1e | 0x1f | 0x6ffffff9 | 0x6ffffffb => {}
            0x70000001 | 0x70000003 => {}
            0x6ffffef5 => info.gnu_hash = value,
            0x6ffffff0 | 0x6ffffffe | 0x6fffffff => {
                return Err(ElfError::UnsupportedSymbolVersion);
            }
            _ => return Err(ElfError::UnsupportedDynamicEntry),
        }
    }
    if info.symtab == 0 || info.symbol_entry_size != 24 {
        return Err(ElfError::MissingDynamicSymbolTable);
    }
    if info.strtab == 0 || info.string_size == 0 {
        return Err(ElfError::MissingDynamicStringTable);
    }
    if info.hash == 0 && info.gnu_hash == 0 {
        return Err(ElfError::MissingDynamicHash);
    }
    if info.rela_size != 0 && info.rela_entry_size != 24 {
        return Err(ElfError::UnsupportedDynamicEntry);
    }
    if info.plt_rela_size != 0 && info.plt_rela_kind != 7 {
        return Err(ElfError::UnsupportedDynamicEntry);
    }
    if info.relr_size != 0 && info.relr_entry_size != 8 {
        return Err(ElfError::UnsupportedDynamicEntry);
    }
    if info.init_array_size % 8 != 0 {
        return Err(ElfError::UnsupportedDynamicEntry);
    }
    Ok(info)
}

pub fn library_tls(bytes: &[u8], machine: Machine) -> Result<Option<TlsSegment>, ElfError> {
    Ok(parse_dynamic_library_headers(bytes, machine)?.3)
}
