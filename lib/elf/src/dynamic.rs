use crate::*;

pub struct LibraryPolicy<'a> {
    pub soname: &'a str,
    pub symbol_prefix: &'a str,
    pub tls: crate::tls::Module,
    pub direct_dependencies: &'a [&'a str],
}

pub struct DynamicLibrary;

impl DynamicLibrary {
    pub fn parse_and_relocate<'a>(
        bytes: &'a [u8],
        load_bias: u64,
        library: LibraryPolicy<'_>,
        machine: Machine,
        global_scope: &[ScopeSymbol],
    ) -> Result<LoadedLibrary<'a>, ElfError> {
        if read_u16(bytes, 16)? != 3 {
            return Err(ElfError::UnsupportedType);
        }
        link_image(bytes, load_bias, library, machine, global_scope, true)
    }
}

/// Link an executable using the same dependency, relocation, RELRO and TLS rules
/// as a DSO. Static executables have no dynamic image to relocate.
pub fn link_executable<'a>(
    bytes: &'a [u8],
    machine: Machine,
    tls: crate::tls::Module,
    dependencies: &[&str],
    scope: &[ScopeSymbol],
) -> Result<Option<LoadedLibrary<'a>>, ElfError> {
    LoadPlan::parse(bytes, machine).map_err(ElfError::from)?;
    match parse_dynamic_library_headers(bytes, machine) {
        Err(ElfError::MissingDynamicSection) => return Ok(None),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let bias = if read_u16(bytes, 16)? == 3 {
        load::PIE_LOAD_BIAS
    } else {
        0
    };
    link_image(
        bytes,
        bias,
        LibraryPolicy {
            soname: "",
            symbol_prefix: "",
            tls,
            direct_dependencies: dependencies,
        },
        machine,
        scope,
        false,
    )
    .map(Some)
}

fn link_image<'a>(
    bytes: &'a [u8],
    load_bias: u64,
    library: LibraryPolicy<'_>,
    machine: Machine,
    global_scope: &[ScopeSymbol],
    require_soname: bool,
) -> Result<LoadedLibrary<'a>, ElfError> {
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic link headers begin\n");
    let (loads, dynamic_offset, dynamic_size, tls, relro) =
        parse_dynamic_library_headers(bytes, machine)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic link headers complete\n");
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic info begin\n");
    let dynamic = parse_dynamic_info(bytes, dynamic_offset, dynamic_size)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic info complete\n");
    if require_soname {
        if dynamic.soname == 0 {
            return Err(ElfError::MissingSoname);
        }
        let soname = read_dyn_string(
            bytes,
            &loads,
            dynamic.strtab,
            dynamic.string_size,
            dynamic.soname,
        )?;
        if soname != library.soname {
            return Err(ElfError::SonameMismatch);
        }
    }
    let direct_needed = library.direct_dependencies;
    for needed in &dynamic.needed {
        let name = read_dyn_string(bytes, &loads, dynamic.strtab, dynamic.string_size, *needed)?;
        if !direct_needed.contains(&name) {
            return Err(ElfError::UndeclaredNeededLibrary);
        }
    }
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic symbol count begin\n");
    let symbol_count = dynamic_symbol_count(bytes, &loads, dynamic.hash, dynamic.gnu_hash)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic symbol parse begin\n");
    let symbols = parse_dynamic_symbols(bytes, &loads, &dynamic, symbol_count)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic scope begin\n");
    let local_scope =
        defined_scope_symbols(bytes, &loads, &dynamic, &symbols, load_bias, library.tls)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic memory image begin\n");
    let mut relocation_scope = BTreeMap::new();
    for symbol in global_scope.iter().chain(local_scope.iter()) {
        relocation_scope
            .entry(symbol.name.clone())
            .or_insert_with(|| symbol.clone());
    }
    // Relocate a zero-initialized memory image. File offsets cannot describe
    // relocation targets in BSS, and zero padding must never come from the archive.
    let (mut image, memory_loads) = memory_image(bytes, &loads)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic relocations begin\n");
    apply_relocations(
        &mut image,
        &memory_loads,
        &dynamic,
        &symbols,
        load_bias,
        &relocation_scope,
        machine,
        library.tls,
    )?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic relocations complete\n");
    let mut segments = Vec::new();
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic segments begin\n");
    for load in &memory_loads {
        let size_bytes = page_round(load.mem_size).ok_or(ElfError::Overflow)?;
        let mut segment = alloc::vec![0; size_bytes as usize];
        let file_start = load.file_offset as usize;
        let file_end = file_start
            .checked_add(load.file_size as usize)
            .ok_or(ElfError::Overflow)?;
        if file_end > image.len() {
            return Err(ElfError::SegmentOutOfBounds);
        }
        segment[..load.file_size as usize].copy_from_slice(&image[file_start..file_end]);
        push_segment_pieces(
            &mut segments,
            segment,
            load.vaddr,
            load_bias,
            load.rights,
            &relro,
        )?;
    }
    let mut exported = Vec::new();
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic exports begin\n");
    for symbol in &symbols {
        if symbol.section_index == 0 {
            continue;
        }
        let binding = symbol.info >> 4;
        if (binding != 1 && binding != 2) || matches!(symbol.visibility & 3, 1 | 2) {
            continue;
        }
        let name = read_dyn_string(
            bytes,
            &loads,
            dynamic.strtab,
            dynamic.string_size,
            u64::from(symbol.name_offset),
        )?;
        if name.starts_with(library.symbol_prefix) && symbol.info & 15 != 6 {
            exported.push(RuntimeSymbol {
                name: name.to_string(),
                address: (if symbol.section_index == 0xfff1 {
                    0u64
                } else {
                    load_bias
                })
                .checked_add(symbol.value)
                .ok_or(ElfError::Overflow)?,
            });
        }
    }
    let tls_template = if let Some(tls) = tls {
        #[cfg(bexos_guest)]
        bexos_userspace::log("elf: dynamic tls template begin\n");
        let headers = read_u64(bytes, 32)? as usize;
        let count = read_u16(bytes, 56)? as usize;
        let header = (0..count)
            .map(|index| headers + index * 56)
            .find(|offset| read_u32(bytes, *offset).ok() == Some(7))
            .ok_or(ElfError::InvalidTlsSegment)?;
        let address = read_u64(bytes, header + 16)?;
        let start = usize::try_from(
            crate::symbols::vaddr_range_to_file_offset(&memory_loads, address, tls.file_size)
                .ok_or(ElfError::TlsSegmentOutOfBounds)?,
        )
        .map_err(|_| ElfError::Overflow)?;
        let initialized = image
            .get(
                start
                    ..start
                        .checked_add(tls.file_size as usize)
                        .ok_or(ElfError::Overflow)?,
            )
            .ok_or(ElfError::TlsSegmentOutOfBounds)?;
        // PT_TLS's zero-fill region can overlap initialized non-TLS sections
        // in PT_LOAD. Only the relocated .tdata bytes belong in the template.
        let mut template =
            alloc::vec![0; usize::try_from(tls.mem_size).map_err(|_| ElfError::Overflow)?];
        template[..initialized.len()].copy_from_slice(initialized);
        template
    } else {
        Vec::new()
    };
    let constructors = library_constructors(&image, &memory_loads, &dynamic, load_bias)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("elf: dynamic constructors complete\n");
    Ok(LoadedLibrary {
        source_bytes: bytes,
        image: Rc::new(LoadedLibraryImage {
            tls_template,
            segments,
            symbols: exported,
            scope_symbols: local_scope,
            constructors,
            tls,
        }),
    })
}

fn push_segment_pieces(
    out: &mut Vec<LoadedLibrarySegment>,
    segment: Vec<u8>,
    segment_vaddr: u64,
    load_bias: u64,
    rights: u32,
    relro: &[(u64, u64)],
) -> Result<(), ElfError> {
    let segment_end = segment_vaddr
        .checked_add(segment.len() as u64)
        .ok_or(ElfError::Overflow)?;
    let mut cursor = 0u64;
    while cursor < segment.len() as u64 {
        let local_vaddr = segment_vaddr
            .checked_add(cursor)
            .ok_or(ElfError::Overflow)?;
        let relro_end = relro
            .iter()
            .find_map(|(start, end)| (local_vaddr >= *start && local_vaddr < *end).then_some(*end));
        let next_boundary = if let Some(end) = relro_end {
            end.min(segment_end)
        } else {
            relro
                .iter()
                .filter_map(|(start, _)| (*start > local_vaddr).then_some(*start))
                .min()
                .unwrap_or(segment_end)
        };
        let piece_end = next_boundary
            .checked_sub(segment_vaddr)
            .ok_or(ElfError::Overflow)?
            .min(segment.len() as u64);
        let piece = segment[cursor as usize..piece_end as usize].to_vec();
        out.push(LoadedLibrarySegment {
            size_bytes: piece.len() as u64,
            bytes: piece,
            vaddr: local_vaddr
                .checked_add(load_bias)
                .ok_or(ElfError::Overflow)?,
            rights: if relro_end.is_some() {
                rights & !RIGHTS_WRITE
            } else {
                rights
            },
        });
        cursor = piece_end;
    }
    Ok(())
}

fn memory_image(
    bytes: &[u8],
    loads: &[ProgramLoad],
) -> Result<(Vec<u8>, Vec<ProgramLoad>), ElfError> {
    let mut image = Vec::new();
    let mut memory_loads = Vec::new();
    for load in loads {
        let start = usize::try_from(load.file_offset).map_err(|_| ElfError::Overflow)?;
        let file_size = usize::try_from(load.file_size).map_err(|_| ElfError::Overflow)?;
        let mem_size = usize::try_from(load.mem_size).map_err(|_| ElfError::Overflow)?;
        let source = bytes
            .get(start..start.checked_add(file_size).ok_or(ElfError::Overflow)?)
            .ok_or(ElfError::SegmentOutOfBounds)?;
        let destination = image.len();
        let end = destination
            .checked_add(mem_size)
            .ok_or(ElfError::Overflow)?;
        image
            .try_reserve(mem_size)
            .map_err(|_| ElfError::Overflow)?;
        image.resize(end, 0);
        image[destination..destination + file_size].copy_from_slice(source);
        memory_loads.push(ProgramLoad {
            file_offset: destination as u64,
            file_size: load.mem_size,
            ..*load
        });
    }
    Ok((image, memory_loads))
}

/// Publish executable definitions before linking dependencies so ELF interposition
/// gives the main program precedence over shared-library definitions.
pub fn executable_scope(
    bytes: &[u8],
    machine: Machine,
    tls: crate::tls::Module,
) -> Result<Vec<ScopeSymbol>, ElfError> {
    let (loads, offset, size, _, _) = match parse_dynamic_library_headers(bytes, machine) {
        Err(ElfError::MissingDynamicSection) => return Ok(Vec::new()),
        result => result?,
    };
    let info = parse_dynamic_info(bytes, offset, size)?;
    let count = dynamic_symbol_count(bytes, &loads, info.hash, info.gnu_hash)?;
    let symbols = parse_dynamic_symbols(bytes, &loads, &info, count)?;
    let bias = if read_u16(bytes, 16)? == 3 {
        load::PIE_LOAD_BIAS
    } else {
        0
    };
    defined_scope_symbols(bytes, &loads, &info, &symbols, bias, tls)
}
