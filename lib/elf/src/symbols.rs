use crate::*;

pub(crate) fn dynamic_symbol_count(
    bytes: &[u8],
    loads: &[ProgramLoad],
    hash_vaddr: u64,
    gnu_hash_vaddr: u64,
) -> Result<usize, ElfError> {
    if hash_vaddr == 0 {
        return gnu_dynamic_symbol_count(bytes, loads, gnu_hash_vaddr);
    }
    let hash_offset =
        vaddr_to_file_offset(loads, hash_vaddr).ok_or(ElfError::DynamicEntryOutOfBounds)?;
    let raw = bytes
        .get(hash_offset as usize..hash_offset as usize + 8)
        .ok_or(ElfError::DynamicEntryOutOfBounds)?;
    let _bucket_count = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    Ok(u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize)
}

pub(crate) fn gnu_dynamic_symbol_count(
    bytes: &[u8],
    loads: &[ProgramLoad],
    hash_vaddr: u64,
) -> Result<usize, ElfError> {
    let offset =
        vaddr_to_file_offset(loads, hash_vaddr).ok_or(ElfError::DynamicEntryOutOfBounds)? as usize;
    let nbuckets = read_u32(bytes, offset)? as usize;
    let symoffset = read_u32(bytes, offset + 4)? as usize;
    let bloom_size = read_u32(bytes, offset + 8)? as usize;
    let header_and_bloom = 16usize
        .checked_add(bloom_size.checked_mul(8).ok_or(ElfError::Overflow)?)
        .ok_or(ElfError::Overflow)?;
    let buckets_offset = offset
        .checked_add(header_and_bloom)
        .ok_or(ElfError::Overflow)?;
    let buckets_size = nbuckets.checked_mul(4).ok_or(ElfError::Overflow)?;
    if buckets_offset
        .checked_add(buckets_size)
        .is_none_or(|end| end > bytes.len())
    {
        return Err(ElfError::DynamicEntryOutOfBounds);
    }
    let mut max_bucket = 0usize;
    for index in 0..nbuckets {
        let bucket = read_u32(bytes, buckets_offset + index * 4)? as usize;
        max_bucket = max_bucket.max(bucket);
    }
    if max_bucket == 0 {
        return Ok(symoffset);
    }
    let chains_offset = buckets_offset
        .checked_add(buckets_size)
        .ok_or(ElfError::Overflow)?;
    let mut symbol = max_bucket;
    loop {
        let chain_index = symbol.checked_sub(symoffset).ok_or(ElfError::Overflow)?;
        let chain_offset = chains_offset
            .checked_add(chain_index.checked_mul(4).ok_or(ElfError::Overflow)?)
            .ok_or(ElfError::Overflow)?;
        let chain = read_u32(bytes, chain_offset)?;
        symbol = symbol.checked_add(1).ok_or(ElfError::Overflow)?;
        if chain & 1 != 0 {
            return Ok(symbol);
        }
    }
}

pub(crate) fn parse_dynamic_symbols(
    bytes: &[u8],
    loads: &[ProgramLoad],
    dynamic: &DynamicInfo,
    count: usize,
) -> Result<Vec<DynamicSymbol>, ElfError> {
    let offset = vaddr_to_file_offset(loads, dynamic.symtab)
        .ok_or(ElfError::DynamicSymbolOutOfBounds)? as usize;
    let total = count.checked_mul(24).ok_or(ElfError::Overflow)?;
    if offset
        .checked_add(total)
        .is_none_or(|end| end > bytes.len())
    {
        return Err(ElfError::DynamicSymbolOutOfBounds);
    }
    let mut symbols = Vec::new();
    for index in 0..count {
        let cursor = offset + index * 24;
        if bytes[cursor + 4] & 15 == 10 {
            return Err(ElfError::UnsupportedRelocation);
        }
        symbols.push(DynamicSymbol {
            name_offset: read_u32(bytes, cursor)?,
            info: *bytes
                .get(cursor + 4)
                .ok_or(ElfError::DynamicSymbolOutOfBounds)?,
            visibility: bytes[cursor + 5],
            section_index: read_u16(bytes, cursor + 6)?,
            value: read_u64(bytes, cursor + 8)?,
        });
    }
    Ok(symbols)
}

pub(crate) fn defined_scope_symbols(
    bytes: &[u8],
    loads: &[ProgramLoad],
    dynamic: &DynamicInfo,
    symbols: &[DynamicSymbol],
    load_bias: u64,
    tls: crate::tls::Module,
) -> Result<Vec<ScopeSymbol>, ElfError> {
    let mut out = Vec::new();
    for symbol in symbols {
        if symbol.section_index == 0 {
            continue;
        }
        let binding = symbol.info >> 4;
        let visibility = symbol.visibility & 0x3;
        if (binding == 1 || binding == 2) && (visibility == 0 || visibility == 3) {
            let name = read_dyn_string(
                bytes,
                loads,
                dynamic.strtab,
                dynamic.string_size,
                u64::from(symbol.name_offset),
            )?;
            out.push(ScopeSymbol {
                name: name.to_string(),
                address: (if symbol.section_index == 0xfff1 {
                    0u64
                } else {
                    load_bias
                })
                .checked_add(symbol.value)
                .ok_or(ElfError::Overflow)?,
                weak: binding == 2,
                tls: if symbol.info & 15 == 6 {
                    Some(TlsSymbol {
                        module: tls.id,
                        offset: symbol.value,
                        thread_offset: tls
                            .thread_offset
                            .checked_add(
                                i64::try_from(symbol.value).map_err(|_| ElfError::Overflow)?,
                            )
                            .ok_or(ElfError::Overflow)?,
                    })
                } else {
                    None
                },
            });
        }
    }
    Ok(out)
}

pub(crate) fn library_constructors(
    image: &[u8],
    loads: &[ProgramLoad],
    dynamic: &DynamicInfo,
    load_bias: u64,
) -> Result<Vec<u64>, ElfError> {
    let mut out = Vec::new();
    if dynamic.init != 0 {
        out.push(
            load_bias
                .checked_add(dynamic.init)
                .ok_or(ElfError::Overflow)?,
        );
    }
    if dynamic.init_array_size != 0 {
        let offset = vaddr_to_file_offset(loads, dynamic.init_array)
            .ok_or(ElfError::DynamicEntryOutOfBounds)? as usize;
        if offset
            .checked_add(dynamic.init_array_size as usize)
            .is_none_or(|end| end > image.len())
        {
            return Err(ElfError::DynamicEntryOutOfBounds);
        }
        for index in 0..(dynamic.init_array_size / 8) as usize {
            let address = read_u64(image, offset + index * 8)?;
            if address != 0 {
                out.push(address);
            }
        }
    }
    Ok(out)
}

pub(crate) fn relocation_symbol_value(
    bytes: &[u8],
    loads: &[ProgramLoad],
    dynamic: &DynamicInfo,
    symbols: &[DynamicSymbol],
    symbol_index: usize,
    scope: &BTreeMap<String, ScopeSymbol>,
    load_bias: u64,
) -> Result<u64, ElfError> {
    let symbol = symbols
        .get(symbol_index)
        .ok_or(ElfError::DynamicSymbolOutOfBounds)?;
    if symbol_index == 0 {
        return Ok(0);
    }
    let binding = symbol.info >> 4;
    if symbol.section_index != 0 {
        if (binding == 1 || binding == 2) && symbol.visibility & 3 == 0 {
            let name = read_dyn_string(
                bytes,
                loads,
                dynamic.strtab,
                dynamic.string_size,
                symbol.name_offset as u64,
            )?;
            if let Some(resolved) = scope.get(name) {
                return Ok(resolved.address);
            }
        }
        return (if symbol.section_index == 0xfff1 {
            0u64
        } else {
            load_bias
        })
        .checked_add(symbol.value)
        .ok_or(ElfError::Overflow);
    }
    let name = read_dyn_string(
        bytes,
        loads,
        dynamic.strtab,
        dynamic.string_size,
        u64::from(symbol.name_offset),
    )?;
    if let Some(address) = scope.get(name) {
        return Ok(address.address);
    }
    if binding == 2 {
        Ok(0)
    } else {
        Err(ElfError::UnresolvedStrongSymbol)
    }
}

pub(crate) fn relocation_targets_text(loads: &[ProgramLoad], vaddr: u64) -> bool {
    loads.iter().any(|load| {
        let end = load.vaddr.checked_add(load.mem_size).unwrap_or(load.vaddr);
        vaddr >= load.vaddr
            && vaddr.checked_add(8).is_some_and(|next| next <= end)
            && load.rights & RIGHTS_EXECUTE != 0
    })
}

pub(crate) fn is_bexos_tls_anchor(name: &str) -> bool {
    matches!(
        name,
        "__bexos_tls_alignment"
            | "__bexos_tls_start"
            | "__bexos_tls_file_end"
            | "__bexos_pthread_local"
            | "__bexos_tls_end"
    )
}

pub(crate) fn read_dyn_string<'a>(
    bytes: &'a [u8],
    loads: &[ProgramLoad],
    strtab: u64,
    string_size: u64,
    name_offset: u64,
) -> Result<&'a str, ElfError> {
    if name_offset >= string_size {
        return Err(ElfError::DynamicSymbolOutOfBounds);
    }
    let offset = vaddr_to_file_offset(loads, strtab + name_offset)
        .ok_or(ElfError::DynamicSymbolOutOfBounds)? as usize;
    let max = offset
        .checked_add((string_size - name_offset) as usize)
        .ok_or(ElfError::Overflow)?;
    if max > bytes.len() {
        return Err(ElfError::DynamicSymbolOutOfBounds);
    }
    let mut end = offset;
    while end < max && bytes[end] != 0 {
        end += 1;
    }
    if end == max {
        return Err(ElfError::DynamicSymbolOutOfBounds);
    }
    core::str::from_utf8(&bytes[offset..end]).map_err(|_| ElfError::DynamicSymbolOutOfBounds)
}

pub(crate) fn vaddr_to_file_offset(loads: &[ProgramLoad], vaddr: u64) -> Option<u64> {
    loads.iter().find_map(|load| {
        let end = load.vaddr.checked_add(load.file_size)?;
        if vaddr >= load.vaddr && vaddr.checked_add(1).is_some_and(|next| next <= end) {
            load.file_offset.checked_add(vaddr - load.vaddr)
        } else {
            None
        }
    })
}

/// Resolve executable-owned runtime anchors from its ELF symbol table.
/// These are ABI symbols, not exports of a package library.
pub fn executable_runtime_anchors(
    bytes: &[u8],
    machine: Machine,
) -> Result<Vec<ScopeSymbol>, ElfError> {
    crate::load::LoadPlan::parse(bytes, machine).map_err(|_| ElfError::InvalidLoadSegment)?;
    let offset = read_u64(bytes, 40)? as usize;
    let size = read_u16(bytes, 58)? as usize;
    let count = read_u16(bytes, 60)? as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    if size != 64
        || offset
            .checked_add(size.checked_mul(count).ok_or(ElfError::Overflow)?)
            .is_none_or(|end| end > bytes.len())
    {
        return Err(ElfError::DynamicSymbolOutOfBounds);
    }
    let mut out = Vec::new();
    for index in 0..count {
        let header = offset + index * size;
        if read_u32(bytes, header + 4)? != 2 {
            continue;
        }
        let strings = read_u32(bytes, header + 40)? as usize;
        if strings >= count || read_u64(bytes, header + 56)? != 24 {
            return Err(ElfError::DynamicSymbolOutOfBounds);
        }
        let sh = offset + strings * size;
        let string_start = read_u64(bytes, sh + 24)? as usize;
        let string_size = read_u64(bytes, sh + 32)? as usize;
        let strings = bytes
            .get(
                string_start
                    ..string_start
                        .checked_add(string_size)
                        .ok_or(ElfError::Overflow)?,
            )
            .ok_or(ElfError::DynamicSymbolOutOfBounds)?;
        let start = read_u64(bytes, header + 24)? as usize;
        let len = read_u64(bytes, header + 32)? as usize;
        let symbols = bytes
            .get(start..start.checked_add(len).ok_or(ElfError::Overflow)?)
            .ok_or(ElfError::DynamicSymbolOutOfBounds)?;
        if len % 24 != 0 {
            return Err(ElfError::DynamicSymbolOutOfBounds);
        }
        for symbol in symbols.chunks_exact(24) {
            if read_u16(symbol, 6)? == 0 {
                continue;
            }
            let name = strings
                .get(read_u32(symbol, 0)? as usize..)
                .ok_or(ElfError::DynamicSymbolOutOfBounds)?;
            let end = name
                .iter()
                .position(|b| *b == 0)
                .ok_or(ElfError::DynamicSymbolOutOfBounds)?;
            let name = core::str::from_utf8(&name[..end])
                .map_err(|_| ElfError::DynamicSymbolOutOfBounds)?;
            if is_bexos_tls_anchor(name) || name == "__tls_get_addr" {
                let bias = if read_u16(bytes, 16)? == 3 && read_u16(symbol, 6)? != 0xfff1 {
                    crate::load::PIE_LOAD_BIAS
                } else {
                    0
                };
                out.push(ScopeSymbol {
                    name: name.to_string(),
                    address: read_u64(symbol, 8)?
                        .checked_add(bias)
                        .ok_or(ElfError::Overflow)?,
                    weak: false,
                    tls: None,
                });
            }
        }
    }
    Ok(out)
}

/// Require a whole range in one load segment, including writes at its final word.
pub(crate) fn vaddr_range_to_file_offset(
    loads: &[ProgramLoad],
    vaddr: u64,
    size: u64,
) -> Option<u64> {
    loads.iter().find_map(|load| {
        let relative = vaddr.checked_sub(load.vaddr)?;
        (relative.checked_add(size)? <= load.file_size)
            .then(|| load.file_offset.checked_add(relative))
            .flatten()
    })
}
