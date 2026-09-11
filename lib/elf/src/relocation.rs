use crate::*;

pub(crate) fn apply_relocations(
    image: &mut [u8],
    loads: &[ProgramLoad],
    dynamic: &DynamicInfo,
    symbols: &[DynamicSymbol],
    load_bias: u64,
    scope: &BTreeMap<String, ScopeSymbol>,
    machine: Machine,
    tls: crate::tls::Module,
) -> Result<(), ElfError> {
    apply_relocation_table(
        image,
        loads,
        dynamic.rela,
        dynamic.rela_size,
        symbols,
        load_bias,
        dynamic,
        scope,
        machine,
        tls,
    )?;
    apply_relocation_table(
        image,
        loads,
        dynamic.plt_rela,
        dynamic.plt_rela_size,
        symbols,
        load_bias,
        dynamic,
        scope,
        machine,
        tls,
    )?;
    apply_relr_table(image, loads, dynamic.relr, dynamic.relr_size, load_bias)
}

pub(crate) fn apply_relocation_table(
    image: &mut [u8],
    loads: &[ProgramLoad],
    vaddr: u64,
    size: u64,
    symbols: &[DynamicSymbol],
    load_bias: u64,
    dynamic: &DynamicInfo,
    scope: &BTreeMap<String, ScopeSymbol>,
    machine: Machine,
    tls: crate::tls::Module,
) -> Result<(), ElfError> {
    if size == 0 {
        return Ok(());
    }
    let table_offset = vaddr_range_to_file_offset(loads, vaddr, size)
        .ok_or(ElfError::RelocationOutOfBounds)? as usize;
    if table_offset
        .checked_add(size as usize)
        .is_none_or(|end| end > image.len())
    {
        return Err(ElfError::RelocationOutOfBounds);
    }
    if size % 24 != 0 {
        return Err(ElfError::RelocationOutOfBounds);
    }
    let count = size / 24;
    for index in 0..count {
        let cursor = table_offset + index as usize * 24;
        let target_vaddr = read_u64(image, cursor)?;
        let info = read_u64(image, cursor + 8)?;
        let addend = read_i64(image, cursor + 16)? as i128;
        let reloc_type = (info & 0xffff_ffff) as u32;
        let symbol_index = (info >> 32) as usize;
        if relocation_targets_text(loads, target_vaddr) {
            return Err(ElfError::TextRelocation);
        }
        let kind = machine
            .relocation(reloc_type)
            .ok_or(ElfError::UnsupportedRelocation)?;
        let value = match kind {
            Relocation::Relative => (load_bias as i128)
                .checked_add(addend)
                .ok_or(ElfError::Overflow)?,
            Relocation::Absolute | Relocation::Symbol => {
                let address = relocation_symbol_value(
                    image,
                    loads,
                    dynamic,
                    symbols,
                    symbol_index,
                    scope,
                    load_bias,
                )?;
                // x86 GLOB_DAT/JUMP_SLOT use S; the absolute relocation uses S+A.
                (address as i128)
                    .checked_add(if kind == Relocation::Symbol {
                        0
                    } else {
                        addend
                    })
                    .ok_or(ElfError::Overflow)?
            }
            Relocation::TlsModule | Relocation::TlsOffset | Relocation::ThreadOffset => {
                let symbol = symbols
                    .get(symbol_index)
                    .ok_or(ElfError::DynamicSymbolOutOfBounds)?;
                let resolved = if symbol.section_index != 0 || symbol_index == 0 {
                    TlsSymbol {
                        module: tls.id,
                        offset: symbol.value,
                        thread_offset: tls
                            .thread_offset
                            .checked_add(
                                i64::try_from(symbol.value).map_err(|_| ElfError::Overflow)?,
                            )
                            .ok_or(ElfError::Overflow)?,
                    }
                } else {
                    let name = read_dyn_string(
                        image,
                        loads,
                        dynamic.strtab,
                        dynamic.string_size,
                        symbol.name_offset as u64,
                    )?;
                    if let Some(value) = scope.get(name).and_then(|s| s.tls) {
                        value
                    } else if is_bexos_tls_anchor(name) {
                        TlsSymbol {
                            module: tls.id,
                            offset: 0,
                            thread_offset: tls.thread_offset,
                        }
                    } else if symbol.info >> 4 == 2 {
                        TlsSymbol {
                            module: 0,
                            offset: 0,
                            thread_offset: 0,
                        }
                    } else {
                        return Err(ElfError::UnresolvedStrongSymbol);
                    }
                };
                match kind {
                    Relocation::TlsModule => resolved.module as i128,
                    Relocation::TlsOffset => resolved.offset as i128 + addend,
                    Relocation::ThreadOffset => resolved.thread_offset as i128 + addend,
                    _ => unreachable!(),
                }
            }
        };
        if (value < 0 && (kind != Relocation::ThreadOffset || value < i64::MIN as i128))
            || value > u64::MAX as i128
        {
            return Err(ElfError::Overflow);
        }
        let target_offset = vaddr_range_to_file_offset(loads, target_vaddr, 8)
            .ok_or(ElfError::RelocationTargetOutOfBounds)? as usize;
        let end = target_offset.checked_add(8).ok_or(ElfError::Overflow)?;
        if end > image.len() {
            return Err(ElfError::RelocationTargetOutOfBounds);
        }
        image[target_offset..end].copy_from_slice(&(value as u64).to_le_bytes());
    }
    Ok(())
}

fn apply_relr_table(
    image: &mut [u8],
    loads: &[ProgramLoad],
    vaddr: u64,
    size: u64,
    load_bias: u64,
) -> Result<(), ElfError> {
    if size == 0 {
        return Ok(());
    }
    let table_offset = vaddr_range_to_file_offset(loads, vaddr, size)
        .ok_or(ElfError::RelocationOutOfBounds)? as usize;
    if table_offset
        .checked_add(size as usize)
        .is_none_or(|end| end > image.len())
        || size % 8 != 0
    {
        return Err(ElfError::RelocationOutOfBounds);
    }
    let mut base = 0u64;
    for index in 0..(size / 8) as usize {
        let entry = read_u64(image, table_offset + index * 8)?;
        if entry & 1 == 0 {
            apply_relative_at(image, loads, entry, load_bias)?;
            base = entry.checked_add(8).ok_or(ElfError::Overflow)?;
        } else {
            let mut bitmap = entry >> 1;
            for bit in 0..63 {
                if bitmap & 1 != 0 {
                    let target = base.checked_add(bit * 8).ok_or(ElfError::Overflow)?;
                    apply_relative_at(image, loads, target, load_bias)?;
                }
                bitmap >>= 1;
            }
            base = base.checked_add(63 * 8).ok_or(ElfError::Overflow)?;
        }
    }
    Ok(())
}

fn apply_relative_at(
    image: &mut [u8],
    loads: &[ProgramLoad],
    target_vaddr: u64,
    load_bias: u64,
) -> Result<(), ElfError> {
    if relocation_targets_text(loads, target_vaddr) {
        return Err(ElfError::TextRelocation);
    }
    let target_offset = vaddr_range_to_file_offset(loads, target_vaddr, 8)
        .ok_or(ElfError::RelocationTargetOutOfBounds)? as usize;
    let current = read_u64(image, target_offset)?;
    let value = current.checked_add(load_bias).ok_or(ElfError::Overflow)?;
    image[target_offset..target_offset + 8].copy_from_slice(&value.to_le_bytes());
    Ok(())
}
