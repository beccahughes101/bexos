use super::kernel::{CreatedProcess, CreatedVmar, KernelHandle, KernelOps};
use super::{
    LaunchError, LaunchRequest, LaunchResult, PackageImageResolver, PackageLibrary,
    PackageLibraryKind,
};
use crate::manifest::{ElfRunnerOptions, ProcessRunnerOptions};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use bexos_elf::{BssMapping, ElfError, ElfMapping, ParsedElf, TlsSegment};
use bexos_elf::{
    DynamicLibrary, LibraryPolicy, LoadedLibrary, LoadedLibraryImage, LoadedLibrarySegment,
    RuntimeSymbol, ScopeSymbol,
};
use bexos_kernel_core::loader::{RIGHTS_EXECUTE, RIGHTS_READ, RIGHTS_WRITE};
use core::cell::RefCell;
mod arch;
use arch::{ELF_MACHINE, LIBRARY_LOAD_BASE, LIBRARY_LOAD_LIMIT, TLS_LOAD_BASE};

pub const DEFAULT_STACK_SIZE: u64 = bexos_boot::USER_STACK_SIZE;
pub const DEFAULT_STACK_TOP: u64 = bexos_boot::USER_STACK_TOP;
pub const PAGE_SIZE: u64 = bexos_kernel_core::loader::PAGE_SIZE;
const VMO_FLAGS_NONE: u32 = 0;
const MAX_LIBRARY_COUNT: usize = 64;
const VMAR_CAN_MAP_READ: u32 = 0x0000_0001;
const VMAR_CAN_MAP_WRITE: u32 = 0x0000_0002;
const VMAR_CAN_MAP_EXECUTE: u32 = 0x0000_0004;
const VMAR_CAN_MAP_SPECIFIC: u32 = 0x0000_0008;

mod runtime_metadata;
use bexos_elf::tls;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ElfRunner {
    shared_library_segments: RefCell<Vec<CachedLibrarySegment>>,
    relocated_libraries: RefCell<Vec<CachedLoadedLibrary>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CachedLibrarySegment {
    content_hash: [u8; 32],
    package_name: String,
    export_name: String,
    abi_version: u32,
    load_bias: u64,
    vaddr: u64,
    size_bytes: u64,
    rights: u32,
    vmo: KernelHandle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CachedLoadedLibrary {
    source_hash: [u8; 32],
    package_name: String,
    export_name: String,
    abi_version: u32,
    load_bias: u64,
    global_scope: Vec<ScopeSymbol>,
    tls_module: tls::Module,
    image: Rc<LoadedLibraryImage>,
}

impl ElfRunner {
    pub fn launch<K: KernelOps, R: PackageImageResolver>(
        &self,
        request: &LaunchRequest<'_>,
        kernel: &mut K,
        resolver: &R,
    ) -> Result<LaunchResult, LaunchError> {
        self.launch_impl(request, kernel, resolver, false)
    }

    /// Only the WASM adapter can select the fixed, platform-authenticated image.
    pub(super) fn launch_trusted_runtime<K: KernelOps, R: PackageImageResolver>(
        &self,
        request: &LaunchRequest<'_>,
        kernel: &mut K,
        resolver: &R,
    ) -> Result<LaunchResult, LaunchError> {
        self.launch_impl(request, kernel, resolver, true)
    }

    fn launch_impl<K: KernelOps, R: PackageImageResolver>(
        &self,
        request: &LaunchRequest<'_>,
        kernel: &mut K,
        resolver: &R,
        trusted_runtime: bool,
    ) -> Result<LaunchResult, LaunchError> {
        if !trusted_runtime && request.runner_policy.is_none() && !request.trust_tier.allows_elf() {
            return Err(LaunchError::RunnerPolicyDenied);
        }

        let architecture = request.manifest.architecture;
        if architecture == crate::manifest::Architecture::Multi
            || !architecture.compatible_with(crate::manifest::Architecture::current_guest())
        {
            return Err(LaunchError::Elf(ElfError::UnsupportedMachine));
        }
        let options = elf_options(request)?;
        if options.path.is_empty() {
            return Err(LaunchError::MissingRunnerOptions);
        }

        let image = resolver
            .resolve_executable(&request.manifest.package_name, &options.path)
            .map_err(LaunchError::PackageImage)?;
        let parsed = ParsedElf::parse(image.bytes).map_err(LaunchError::Elf)?;

        let process_name =
            launch_process_name(&request.manifest.package_name, &request.process.name)?;
        let created = kernel
            .create_process(
                &process_name,
                request.resource_group_id,
                request.identity.package_id,
                request.hardware_access,
                request.realtime_scheduling,
            )
            .map_err(|source| kernel_error("create_process", source))?;
        bexos_userspace::log(&format!(
            "appd: elf launch process created package={} process={}\n",
            request.manifest.package_name, request.process.name
        ));

        let mut owned_vmos = Vec::new();
        let mut owned_handles = Vec::new();
        let mut constructed_vmars = Vec::new();
        let load_result = (|| -> Result<LaunchResult, LaunchError> {
            let image_span = image_span(&parsed.mappings).map_err(LaunchError::Elf)?;
            let image_arena = create_arena_for_span(
                kernel,
                created.root_vmar,
                bexos_boot::USER_START,
                image_span,
                VMAR_CAN_MAP_READ | VMAR_CAN_MAP_WRITE | VMAR_CAN_MAP_EXECUTE,
                "create_image_arena",
                &mut constructed_vmars,
            )?;
            bexos_userspace::log(&format!(
                "appd: elf image arena ready package={} base={:#x} size={:#x}\n",
                request.manifest.package_name, image_arena.base_address, image_span.1
            ));

            bexos_userspace::log(&format!(
                "appd: elf collect libraries begin package={}\n",
                request.manifest.package_name
            ));
            let libraries = collect_libraries(request, resolver).map_err(LaunchError::Elf)?;
            bexos_userspace::log(&format!(
                "appd: elf collect libraries complete package={} count={}\n",
                request.manifest.package_name,
                libraries.len()
            ));
            let executable_dependencies = libraries
                .iter()
                .filter(|library| {
                    request
                        .manifest
                        .library_dependencies
                        .iter()
                        .any(|dependency| {
                            dependency.package_name == library.package_name
                                && dependency.abi_version == library.abi_version
                        })
                })
                .map(|library| library.soname)
                .collect::<Vec<_>>();
            bexos_userspace::log(&format!(
                "appd: elf tls layout begin package={}\n",
                request.manifest.package_name
            ));
            let library_tls = libraries
                .iter()
                .map(|library| bexos_elf::library_tls(library.image.bytes, ELF_MACHINE))
                .collect::<Result<Vec<_>, _>>()
                .map_err(LaunchError::Elf)?;
            let mut tls_layout = tls::layout(
                ELF_MACHINE,
                core::iter::once((image.bytes, parsed.tls)).chain(
                    libraries
                        .iter()
                        .zip(&library_tls)
                        .map(|(library, tls)| (library.image.bytes, *tls)),
                ),
            )
            .map_err(LaunchError::Elf)?;
            bexos_userspace::log(&format!(
                "appd: elf tls layout complete package={}\n",
                request.manifest.package_name
            ));
            let mut runtime_symbols = Vec::new();
            bexos_userspace::log(&format!(
                "appd: elf runtime anchors begin package={}\n",
                request.manifest.package_name
            ));
            let mut global_scope = bexos_elf::executable_runtime_anchors(image.bytes, ELF_MACHINE)
                .map_err(LaunchError::Elf)?;
            bexos_userspace::log(&format!(
                "appd: elf runtime anchors complete package={} count={}\n",
                request.manifest.package_name,
                global_scope.len()
            ));
            bexos_userspace::log(&format!(
                "appd: elf executable scope begin package={}\n",
                request.manifest.package_name
            ));
            global_scope.extend(
                bexos_elf::executable_scope(image.bytes, ELF_MACHINE, tls_layout.modules[0])
                    .map_err(LaunchError::Elf)?,
            );
            bexos_userspace::log(&format!(
                "appd: elf executable scope complete package={} count={}\n",
                request.manifest.package_name,
                global_scope.len()
            ));
            let mut constructors = Vec::new();
            let mut loaded_libraries = Vec::new();
            let mut next_library_base = LIBRARY_LOAD_BASE;
            for (library_index, library) in libraries.into_iter().enumerate() {
                bexos_userspace::log(&format!(
                    "appd: elf library begin package={} library={} abi={} bytes={}\n",
                    request.manifest.package_name,
                    library.package_name,
                    library.abi_version,
                    library.image.bytes.len()
                ));
                let tls_module = tls_layout.modules[library_index + 1];
                bexos_userspace::log(&format!(
                    "appd: elf library span begin package={} library={}\n",
                    request.manifest.package_name, library.package_name
                ));
                let (span_start, span_end, span_align) =
                    bexos_elf::dynamic_library_span(library.image.bytes, ELF_MACHINE)
                        .map_err(LaunchError::Elf)?;
                bexos_userspace::log(&format!(
                    "appd: elf library span complete package={} library={} span={:#x}..{:#x} align={:#x}\n",
                    request.manifest.package_name,
                    library.package_name,
                    span_start,
                    span_end,
                    span_align
                ));
                let biased_start = next_library_base
                    .checked_sub(span_start)
                    .ok_or(LaunchError::Elf(ElfError::Overflow))?;
                let load_bias = align_up_to(biased_start, span_align.max(PAGE_SIZE))
                    .ok_or(LaunchError::Elf(ElfError::Overflow))?;
                let loaded_end = load_bias
                    .checked_add(span_end)
                    .ok_or(LaunchError::Elf(ElfError::Overflow))?;
                if loaded_end > LIBRARY_LOAD_LIMIT {
                    return Err(LaunchError::Elf(ElfError::LibraryAddressCollision));
                }
                next_library_base =
                    page_round(loaded_end).ok_or(LaunchError::Elf(ElfError::Overflow))?;
                let library_arena = create_arena_for_span(
                    kernel,
                    created.root_vmar,
                    bexos_boot::USER_START,
                    (
                        load_bias + span_start,
                        loaded_end - (load_bias + span_start),
                    ),
                    VMAR_CAN_MAP_READ | VMAR_CAN_MAP_WRITE | VMAR_CAN_MAP_EXECUTE,
                    "create_library_arena",
                    &mut constructed_vmars,
                )?;
                bexos_userspace::log(&format!(
                    "appd: elf library arena ready package={} library={} base={:#x} size={:#x}\n",
                    request.manifest.package_name,
                    library.package_name,
                    library_arena.base_address,
                    loaded_end - (load_bias + span_start)
                ));
                let source_hash = *blake3::hash(library.image.bytes).as_bytes();
                bexos_userspace::log(&format!(
                    "appd: elf library relocate begin package={} library={}\n",
                    request.manifest.package_name, library.package_name
                ));
                let cached_image = self
                    .relocated_libraries
                    .borrow()
                    .iter()
                    .find(|cached| {
                        cached.source_hash == source_hash
                            && cached.package_name == library.package_name
                            && cached.export_name == library.export_name
                            && cached.abi_version == library.abi_version
                            && cached.load_bias == load_bias
                            && cached.global_scope == global_scope
                            && cached.tls_module == tls_module
                    })
                    .map(|cached| Rc::clone(&cached.image));
                let loaded = if let Some(image) = cached_image {
                    bexos_userspace::log(&format!(
                        "appd: elf library relocate cached package={} library={}\n",
                        request.manifest.package_name, library.package_name
                    ));
                    LoadedLibrary {
                        source_bytes: library.image.bytes,
                        image,
                    }
                } else {
                    let loaded = DynamicLibrary::parse_and_relocate(
                        library.image.bytes,
                        load_bias,
                        LibraryPolicy {
                            soname: library.soname,
                            tls: tls_module,
                            symbol_prefix: library.symbol_prefix,
                            direct_dependencies: &library
                                .direct_dependencies
                                .iter()
                                .map(|d| d.soname.as_str())
                                .collect::<Vec<_>>(),
                        },
                        ELF_MACHINE,
                        &global_scope,
                    )
                    .map_err(LaunchError::Elf)?;
                    self.relocated_libraries
                        .borrow_mut()
                        .push(CachedLoadedLibrary {
                            source_hash,
                            package_name: library.package_name.to_string(),
                            export_name: library.export_name.to_string(),
                            abi_version: library.abi_version,
                            load_bias,
                            global_scope: global_scope.clone(),
                            tls_module,
                            image: Rc::clone(&loaded.image),
                        });
                    loaded
                };
                bexos_userspace::log(&format!(
                    "appd: elf library relocate complete package={} library={} segments={}\n",
                    request.manifest.package_name,
                    library.package_name,
                    loaded.segments.len()
                ));
                for segment in &loaded.segments {
                    let content_hash = *blake3::hash(&segment.bytes).as_bytes();
                    let share =
                        kernel.supports_shared_library_vmos() && segment.rights & RIGHTS_WRITE == 0;
                    let cached = share.then(|| {
                        self.shared_library_segments
                            .borrow()
                            .iter()
                            .find(|cached| {
                                cached.content_hash == content_hash
                                    && cached.package_name == library.package_name
                                    && cached.export_name == library.export_name
                                    && cached.abi_version == library.abi_version
                                    && cached.load_bias == load_bias
                                    && cached.vaddr == segment.vaddr
                                    && cached.size_bytes == segment.size_bytes
                                    && cached.rights == segment.rights
                            })
                            .cloned()
                    });
                    if let Some(cached) = cached.flatten() {
                        bexos_userspace::log(&format!(
                            "appd: elf library segment cached package={} library={} vaddr={:#x} size={:#x}\n",
                            request.manifest.package_name,
                            library.package_name,
                            segment.vaddr,
                            segment.size_bytes
                        ));
                        map_existing_library_segment(
                            kernel,
                            library_arena,
                            cached.vmo,
                            segment.vaddr,
                            segment.size_bytes,
                            segment.rights,
                            &mut constructed_vmars,
                        )?;
                    } else {
                        bexos_userspace::log(&format!(
                            "appd: elf library segment mapping package={} library={} vaddr={:#x} size={:#x} share={}\n",
                            request.manifest.package_name,
                            library.package_name,
                            segment.vaddr,
                            segment.size_bytes,
                            share
                        ));
                        let retained = map_loaded_segment(
                            kernel,
                            library_arena,
                            segment,
                            &mut constructed_vmars,
                            share,
                        )?;
                        if let Some(vmo) = retained {
                            self.shared_library_segments
                                .borrow_mut()
                                .push(CachedLibrarySegment {
                                    content_hash,
                                    package_name: library.package_name.to_string(),
                                    export_name: library.export_name.to_string(),
                                    abi_version: library.abi_version,
                                    load_bias,
                                    vaddr: segment.vaddr,
                                    size_bytes: segment.size_bytes,
                                    rights: segment.rights,
                                    vmo,
                                });
                        }
                    }
                    bexos_userspace::log(&format!(
                        "appd: elf library segment mapped package={} library={} vaddr={:#x}\n",
                        request.manifest.package_name, library.package_name, segment.vaddr
                    ));
                }
                tls_layout
                    .install_relocated(library_index + 1, &loaded)
                    .map_err(LaunchError::Elf)?;
                global_scope.extend(loaded.scope_symbols.clone());
                runtime_symbols.extend(loaded.symbols.clone());
                constructors.extend(loaded.constructors.iter().copied());
                loaded_libraries.push(loaded);
            }

            bexos_userspace::log(&format!(
                "appd: elf link executable begin package={}\n",
                request.manifest.package_name
            ));
            let linked_executable = bexos_elf::link_executable(
                image.bytes,
                ELF_MACHINE,
                tls_layout.modules[0],
                &executable_dependencies,
                &global_scope,
            )
            .map_err(LaunchError::Elf)?;
            bexos_userspace::log(&format!(
                "appd: elf link executable complete package={} dynamic={}\n",
                request.manifest.package_name,
                linked_executable.is_some()
            ));
            if let Some(linked) = &linked_executable {
                tls_layout
                    .install_relocated(0, linked)
                    .map_err(LaunchError::Elf)?;
                for segment in &linked.segments {
                    bexos_userspace::log(&format!(
                        "appd: elf linked segment mapping package={} vaddr={:#x} size={:#x}\n",
                        request.manifest.package_name, segment.vaddr, segment.size_bytes
                    ));
                    map_loaded_segment(
                        kernel,
                        image_arena,
                        segment,
                        &mut constructed_vmars,
                        false,
                    )?;
                }
                constructors.extend(linked.constructors.iter().copied());
                runtime_symbols.extend(linked.symbols.iter().cloned());
            }
            for mapping in parsed
                .mappings
                .iter()
                .filter(|_| linked_executable.is_none())
            {
                if mapping.file_size != 0 {
                    let map_size = page_round(mapping.file_size)
                        .ok_or(LaunchError::Elf(ElfError::Overflow))?;
                    let segment_vmar = segment_vmar(
                        kernel,
                        image_arena,
                        mapping.vaddr,
                        map_size,
                        mapping.rights,
                        "create_image_segment_vmar",
                        &mut constructed_vmars,
                    )?;
                    // Writable segments are private to each driver instance.
                    // A partial last page also needs zero padding for BSS.
                    let private =
                        mapping.file_size % PAGE_SIZE != 0 || mapping.rights & RIGHTS_WRITE != 0;
                    let (mapped_vmo, vmo_offset, release_after_map) =
                        if private || image.vmo.raw == 0 {
                            let start = usize::try_from(mapping.file_offset)
                                .map_err(|_| LaunchError::Elf(ElfError::Overflow))?;
                            let size = usize::try_from(mapping.file_size)
                                .map_err(|_| LaunchError::Elf(ElfError::Overflow))?;
                            let bytes = image
                                .bytes
                                .get(
                                    start
                                        ..start
                                            .checked_add(size)
                                            .ok_or(LaunchError::Elf(ElfError::Overflow))?,
                                )
                                .ok_or(LaunchError::Elf(ElfError::SegmentOutOfBounds))?;
                            let vmo = kernel.create_vmo_from_bytes(bytes).map_err(|source| {
                                kernel_error("create_image_segment_vmo", source)
                            })?;
                            (vmo, 0, true)
                        } else {
                            (
                                image.vmo,
                                image
                                    .vmo_offset
                                    .checked_add(mapping.file_offset)
                                    .ok_or(LaunchError::Elf(ElfError::Overflow))?,
                                false,
                            )
                        };
                    bexos_userspace::log(&format!(
                        "appd: elf segment mapping package={} vaddr={:#x} size={:#x} source_vmo={} source_offset={:#x} private={}\n",
                        request.manifest.package_name,
                        mapping.vaddr,
                        map_size,
                        mapped_vmo.raw,
                        vmo_offset,
                        release_after_map,
                    ));
                    let mapped = kernel
                        .map_vmo_in_vmar(
                            segment_vmar,
                            mapped_vmo,
                            vmo_offset,
                            0,
                            map_size,
                            vmar_flags_from_rights(mapping.rights),
                        )
                        .map_err(|source| kernel_error("map_segment", source));
                    if release_after_map {
                        let _ = kernel.release_vmo(mapped_vmo);
                    }
                    mapped?;
                    bexos_userspace::log(&format!(
                        "appd: elf segment mapped package={} vaddr={:#x} size={:#x}\n",
                        request.manifest.package_name, mapping.vaddr, map_size
                    ));
                }
                if let Some(bss) = mapping.bss {
                    let bss_vmo = kernel
                        .create_vmo(bss.size_bytes, VMO_FLAGS_NONE)
                        .map_err(|source| kernel_error("create_bss_vmo", source))?;
                    owned_vmos.push(bss_vmo);
                    let bss_vmar = segment_vmar(
                        kernel,
                        image_arena,
                        bss.vaddr,
                        bss.size_bytes,
                        mapping.rights & !RIGHTS_EXECUTE,
                        "create_bss_vmar",
                        &mut constructed_vmars,
                    )?;
                    kernel
                        .map_vmo_in_vmar(
                            bss_vmar,
                            bss_vmo,
                            0,
                            0,
                            bss.size_bytes,
                            vmar_flags_from_rights(mapping.rights & !RIGHTS_EXECUTE),
                        )
                        .map_err(|source| kernel_error("map_bss", source))?;
                    kernel
                        .release_vmo(bss_vmo)
                        .map_err(|source| kernel_error("release_bss_vmo", source))?;
                    owned_vmos.retain(|handle| *handle != bss_vmo);
                    bexos_userspace::log(&format!(
                        "appd: elf bss mapped package={} vaddr={:#x} size={:#x}\n",
                        request.manifest.package_name, bss.vaddr, bss.size_bytes
                    ));
                }
            }

            let thread_pointer_vaddr = if tls_layout.mem_size != 0 {
                let tls_vaddr = align_up_to(TLS_LOAD_BASE, tls_layout.align)
                    .ok_or(LaunchError::Elf(ElfError::Overflow))?;
                let (tls_bytes, thread_pointer) =
                    tls::initialize(&tls_layout, ELF_MACHINE, tls_vaddr)
                        .map_err(LaunchError::Elf)?;
                let tls_vmo = kernel
                    .create_vmo_from_bytes(&tls_bytes)
                    .map_err(|source| kernel_error("create_tls_vmo", source))?;
                owned_vmos.push(tls_vmo);
                let tls_vaddr = align_up_to(TLS_LOAD_BASE, tls_layout.align)
                    .ok_or(LaunchError::Elf(ElfError::Overflow))?;
                let tls_vmar = create_arena_for_span(
                    kernel,
                    created.root_vmar,
                    bexos_boot::USER_START,
                    (tls_vaddr, tls_bytes.len() as u64),
                    VMAR_CAN_MAP_READ | VMAR_CAN_MAP_WRITE,
                    "create_tls_vmar",
                    &mut constructed_vmars,
                )?;
                kernel
                    .map_vmo_in_vmar(
                        tls_vmar.vmar,
                        tls_vmo,
                        0,
                        0,
                        tls_bytes.len() as u64,
                        VMAR_CAN_MAP_READ | VMAR_CAN_MAP_WRITE,
                    )
                    .map_err(|source| kernel_error("map_tls", source))?;
                kernel
                    .release_vmo(tls_vmo)
                    .map_err(|source| kernel_error("release_tls_vmo", source))?;
                owned_vmos.retain(|handle| *handle != tls_vmo);
                thread_pointer
            } else {
                0
            };

            let stack_vmo = kernel
                .create_vmo(DEFAULT_STACK_SIZE, VMO_FLAGS_NONE)
                .map_err(|source| kernel_error("create_stack_vmo", source))?;
            owned_vmos.push(stack_vmo);
            let stack_base = DEFAULT_STACK_TOP - DEFAULT_STACK_SIZE;
            let _stack_guard = create_arena_for_span(
                kernel,
                created.root_vmar,
                bexos_boot::USER_START,
                (stack_base - PAGE_SIZE, PAGE_SIZE),
                VMAR_CAN_MAP_READ,
                "create_stack_guard_vmar",
                &mut constructed_vmars,
            )?;
            let stack_vmar = create_arena_for_span(
                kernel,
                created.root_vmar,
                bexos_boot::USER_START,
                (stack_base, DEFAULT_STACK_SIZE),
                VMAR_CAN_MAP_READ | VMAR_CAN_MAP_WRITE,
                "create_stack_vmar",
                &mut constructed_vmars,
            )?;
            kernel
                .map_vmo_in_vmar(
                    stack_vmar.vmar,
                    stack_vmo,
                    0,
                    0,
                    DEFAULT_STACK_SIZE,
                    VMAR_CAN_MAP_READ | VMAR_CAN_MAP_WRITE,
                )
                .map_err(|source| kernel_error("map_stack", source))?;
            kernel
                .release_vmo(stack_vmo)
                .map_err(|source| kernel_error("release_stack_vmo", source))?;
            owned_vmos.retain(|handle| *handle != stack_vmo);

            let service_manager = kernel
                .create_channel()
                .map_err(|source| kernel_error("create_channel", source))?;
            owned_handles.extend([service_manager.local, service_manager.remote]);
            bexos_userspace::log(&format!(
                "appd: elf service channel ready package={}\n",
                request.manifest.package_name
            ));
            let runtime_linker_data = if runtime_symbols.is_empty()
                && constructors.is_empty()
                && tls_layout.mem_size == 0
            {
                None
            } else {
                let bytes = runtime_metadata::encode(
                    &runtime_symbols,
                    &constructors,
                    &tls_layout.template[..tls_layout.mem_size as usize],
                    tls_layout.mem_size,
                    tls_layout.align,
                    &tls_layout.modules,
                )
                .map_err(LaunchError::Elf)?;
                let len = bytes.len() as u64;
                let handle = kernel
                    .create_vmo_from_bytes(&bytes)
                    .map_err(|source| kernel_error("create_linker_data_vmo", source))?;
                owned_vmos.push(handle);
                Some((handle, len))
            };
            let main_thread = kernel
                .start_thread_in_process(
                    created.process,
                    created.address_space,
                    parsed.entry_vaddr,
                    DEFAULT_STACK_TOP,
                    thread_pointer_vaddr,
                    Some(service_manager.remote),
                )
                .map_err(|source| kernel_error("start_thread", source))?;
            owned_handles.push(main_thread);
            bexos_userspace::log(&format!(
                "appd: elf main thread started package={} entry={:#x}\n",
                request.manifest.package_name, parsed.entry_vaddr
            ));

            close_construction_handles(kernel, created, &constructed_vmars)?;

            Ok(LaunchResult {
                process_handle: created.process,
                address_space_handle: created.address_space,
                main_thread_handle: main_thread,
                service_manager_handle: service_manager.local,
                runtime_linker_data,
            })
        })();
        if load_result.is_err() {
            for handle in owned_vmos {
                let _ = kernel.release_vmo(handle);
            }
            for handle in owned_handles {
                let _ = kernel.close_handle(handle);
            }
            cleanup_failed_launch(kernel, created, &constructed_vmars);
        }
        load_result
    }
}

fn image_span(mappings: &[ElfMapping]) -> Result<(u64, u64), ElfError> {
    let mut start = u64::MAX;
    let mut end = 0u64;
    for mapping in mappings {
        start = start.min(mapping.vaddr);
        let file_end = mapping
            .vaddr
            .checked_add(page_round(mapping.file_size).ok_or(ElfError::Overflow)?)
            .ok_or(ElfError::Overflow)?;
        end = end.max(file_end);
        if let Some(bss) = mapping.bss {
            end = end.max(
                bss.vaddr
                    .checked_add(bss.size_bytes)
                    .ok_or(ElfError::Overflow)?,
            );
        }
    }
    if start == u64::MAX || start >= end {
        return Err(ElfError::MissingExecutableSegment);
    }
    Ok((start, page_round(end - start).ok_or(ElfError::Overflow)?))
}

fn vmar_flags_from_rights(rights: u32) -> u32 {
    let mut flags = 0;
    if rights & RIGHTS_READ != 0 {
        flags |= VMAR_CAN_MAP_READ;
    }
    if rights & RIGHTS_WRITE != 0 {
        flags |= VMAR_CAN_MAP_WRITE;
    }
    if rights & RIGHTS_EXECUTE != 0 {
        flags |= VMAR_CAN_MAP_EXECUTE;
    }
    flags
}

fn create_arena_for_span<K: KernelOps>(
    kernel: &mut K,
    parent: KernelHandle,
    parent_base: u64,
    span: (u64, u64),
    rights: u32,
    operation: &'static str,
    constructed_vmars: &mut Vec<KernelHandle>,
) -> Result<CreatedVmar, LaunchError> {
    let (base, size) = span;
    if base < parent_base || size == 0 {
        return Err(LaunchError::Elf(ElfError::InvalidLoadSegment));
    }
    let offset = base
        .checked_sub(parent_base)
        .ok_or(LaunchError::Elf(ElfError::Overflow))?;
    let created = kernel
        .create_sub_vmar(
            parent,
            offset,
            page_round(size).ok_or(LaunchError::Elf(ElfError::Overflow))?,
            rights | VMAR_CAN_MAP_SPECIFIC,
        )
        .map_err(|source| kernel_error(operation, source))?;
    constructed_vmars.push(created.vmar);
    Ok(created)
}

fn segment_vmar<K: KernelOps>(
    kernel: &mut K,
    parent: CreatedVmar,
    vaddr: u64,
    size: u64,
    rights: u32,
    operation: &'static str,
    constructed_vmars: &mut Vec<KernelHandle>,
) -> Result<KernelHandle, LaunchError> {
    Ok(create_arena_for_span(
        kernel,
        parent.vmar,
        parent.base_address,
        (vaddr, size),
        vmar_flags_from_rights(rights),
        operation,
        constructed_vmars,
    )?
    .vmar)
}

fn close_construction_handles<K: KernelOps>(
    kernel: &mut K,
    created: CreatedProcess,
    constructed_vmars: &[KernelHandle],
) -> Result<(), LaunchError> {
    for handle in constructed_vmars.iter().copied() {
        kernel
            .close_handle(handle)
            .map_err(|source| kernel_error("close_vmar_handle", source))?;
    }
    kernel
        .close_handle(created.root_vmar)
        .map_err(|source| kernel_error("close_root_vmar_handle", source))
}

fn cleanup_failed_launch<K: KernelOps>(
    kernel: &mut K,
    created: CreatedProcess,
    constructed_vmars: &[KernelHandle],
) {
    for handle in constructed_vmars.iter().rev().copied() {
        let _ = kernel.destroy_vmar(handle);
        let _ = kernel.close_handle(handle);
    }
    let _ = kernel.close_handle(created.root_vmar);
    let _ = kernel.terminate_process(created.process, -1);
    let _ = kernel.close_handle(created.address_space);
    let _ = kernel.close_handle(created.process);
}

fn map_loaded_segment<K: KernelOps>(
    kernel: &mut K,
    library_arena: CreatedVmar,
    segment: &LoadedLibrarySegment,
    constructed_vmars: &mut Vec<KernelHandle>,
    retain_vmo: bool,
) -> Result<Option<KernelHandle>, LaunchError> {
    let segment_vmo = kernel
        .create_vmo_from_bytes(&segment.bytes)
        .map_err(|source| kernel_error("create_library_segment_vmo", source))?;
    let mapped = map_existing_library_segment(
        kernel,
        library_arena,
        segment_vmo,
        segment.vaddr,
        segment.size_bytes,
        segment.rights,
        constructed_vmars,
    );
    if let Err(error) = mapped {
        let _ = kernel.release_vmo(segment_vmo);
        return Err(error);
    }
    if retain_vmo {
        Ok(Some(segment_vmo))
    } else {
        kernel
            .release_vmo(segment_vmo)
            .map_err(|source| kernel_error("release_library_segment_vmo", source))?;
        Ok(None)
    }
}

fn map_existing_library_segment<K: KernelOps>(
    kernel: &mut K,
    library_arena: CreatedVmar,
    segment_vmo: KernelHandle,
    vaddr: u64,
    size_bytes: u64,
    rights: u32,
    constructed_vmars: &mut Vec<KernelHandle>,
) -> Result<(), LaunchError> {
    let segment_vmar = segment_vmar(
        kernel,
        library_arena,
        vaddr,
        size_bytes,
        rights,
        "create_library_segment_vmar",
        constructed_vmars,
    )?;
    kernel
        .map_vmo_in_vmar(
            segment_vmar,
            segment_vmo,
            0,
            0,
            size_bytes,
            vmar_flags_from_rights(rights),
        )
        .map_err(|source| kernel_error("map_library_segment", source))?;
    Ok(())
}

fn collect_libraries<'a, R: PackageImageResolver>(
    request: &LaunchRequest<'_>,
    resolver: &'a R,
) -> Result<Vec<PackageLibrary<'a>>, ElfError> {
    let mut out = Vec::new();
    let mut visiting = Vec::new();
    for dependency in &request.manifest.library_dependencies {
        let library = resolver
            .resolve_library(&dependency.package_name, dependency.abi_version)
            .map_err(|_| ElfError::MissingLibraryDependency)?;
        collect_library(library, resolver, &mut visiting, &mut out)?;
    }
    Ok(out)
}

fn collect_library<'a, R: PackageImageResolver>(
    library: PackageLibrary<'a>,
    resolver: &'a R,
    visiting: &mut Vec<(&'a str, u32, &'a str)>,
    out: &mut Vec<PackageLibrary<'a>>,
) -> Result<(), ElfError> {
    if library.kind != PackageLibraryKind::Native {
        return Err(ElfError::MissingLibraryDependency);
    }
    if library.soname.is_empty() {
        return Err(ElfError::MissingSoname);
    }
    if out.iter().any(|prior| {
        prior.package_name == library.package_name
            && prior.export_name == library.export_name
            && prior.abi_version == library.abi_version
    }) {
        return Ok(());
    }
    if out.len() >= MAX_LIBRARY_COUNT {
        return Err(ElfError::TooManyLibraries);
    }
    let key = (library.package_name, library.abi_version, library.soname);
    if visiting.contains(&key) {
        return Err(ElfError::LibraryDependencyCycle);
    }
    visiting.push(key);
    for dependency in library.direct_dependencies {
        let dependency_library = resolver
            .resolve_library(&dependency.package_name, dependency.abi_version)
            .map_err(|_| ElfError::MissingLibraryDependency)?;
        if dependency_library.soname != dependency.soname {
            return Err(ElfError::MissingLibraryDependency);
        }
        collect_library(dependency_library, resolver, visiting, out)?;
    }
    let _ = visiting.pop();
    out.push(library);
    Ok(())
}

fn page_round(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|n| n & !(PAGE_SIZE - 1))
}

fn align_up_to(value: u64, align: u64) -> Option<u64> {
    if align <= 1 {
        return Some(value);
    }
    value.checked_add(align - 1).map(|n| n & !(align - 1))
}

fn kernel_error(operation: &'static str, source: super::kernel::KernelError) -> LaunchError {
    bexos_userspace::log(&format!(
        "appd: ELF kernel operation failed operation={operation} error={source:?}\n"
    ));
    LaunchError::Kernel { operation, source }
}

fn elf_options<'a>(request: &'a LaunchRequest<'_>) -> Result<&'a ElfRunnerOptions, LaunchError> {
    match request.process.runner_options.as_ref() {
        Some(ProcessRunnerOptions::Elf(options)) => Ok(options),
        _ => Err(LaunchError::MissingRunnerOptions),
    }
}

fn launch_process_name(package_name: &str, process_name: &str) -> Result<String, LaunchError> {
    let name = format!("{package_name}:{process_name}");
    if name.len() > 64 {
        return Err(LaunchError::ProcessNameTooLong);
    }
    Ok(name)
}
