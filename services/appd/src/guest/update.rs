//! Asynchronous migration coordinator. In particular it never blocks appd on an
//! RPC to appd itself, or debugd on an RPC back to the update transport.
use super::state::AppdState;
use crate::*;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use bexos_userspace::{
    Channel, KernelTransport, Memory, Rpc, Startup, live_migration::now_ms, log,
};
use migration_fidl::*;

pub type Kernel = KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Prepare,
    Initialize,
    Bulk,
    AdoptBulk,
    FinishBulk,
    Delta,
    AdoptDelta,
    Quiesce,
    Validate,
    Activate,
}
pub struct Update {
    package: String,
    generation: u64,
    source: Channel,
    target: Channel,
    replacement: LaunchResult,
    step: Step,
    waiting: bool,
    started: u64,
    preparation_ms: u32,
    cutover_ms: u32,
    sequence: u64,
    delta_records: u32,
    bulk_records: u32,
    record: Vec<u8>,
    pub candidate_record: bexos_app_registry::AppRecord,
    archive_handle: u64,
    archive_len: u64,
    markers: [u64; 3],
    source_markers: [u64; 3],
    freeze_preferences: bool,
}

struct BundleImage {
    runtime: Option<super::resolver::RuntimeImage>,
    package: String,
    path: String,
    bytes: Vec<u8>,
    handle: u64,
    libraries: Vec<BundleLibrary>,
}

struct BundleLibrary {
    package: String,
    export_name: String,
    soname: String,
    symbol_prefix: String,
    abi_version: u32,
    kind: PackageLibraryKind,
    direct_dependencies: Vec<PackageLibraryDependency>,
    bytes: Vec<u8>,
    handle: u64,
}
impl PackageImageResolver for BundleImage {
    fn wasm_runtime_digest(&self) -> [u8; 32] {
        self.runtime
            .as_ref()
            .map_or(*include_bytes!(env!("BEXOS_WASM_RUNNER_DIGEST")), |r| {
                r.digest
            })
    }
    fn resolve_wasm_runtime(&self) -> Result<PackageImage<'_>, PackageImageError> {
        self.runtime
            .as_ref()
            .map(super::resolver::RuntimeImage::image)
            .ok_or(PackageImageError::NotFound)
    }
    fn resolve_executable<'a>(
        &'a self,
        package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        if package != self.package || path != self.path {
            return Err(PackageImageError::InvalidPath);
        }
        Ok(PackageImage {
            bytes: &self.bytes,
            vmo: KernelHandle { raw: self.handle },
            vmo_offset: 0,
        })
    }

    fn resolve_library<'a>(
        &'a self,
        package: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        let image = self
            .libraries
            .iter()
            .find(|library| {
                library.package == package
                    && library.abi_version == abi_version
                    && library.kind == PackageLibraryKind::Native
            })
            .ok_or(PackageImageError::NotFound)?;
        Ok(PackageLibrary {
            package_name: &image.package,
            export_name: &image.export_name,
            soname: &image.soname,
            image: PackageImage {
                bytes: &image.bytes,
                vmo: KernelHandle { raw: image.handle },
                vmo_offset: 0,
            },
            symbol_prefix: &image.symbol_prefix,
            abi_version: image.abi_version,
            kind: image.kind,
            direct_dependencies: &image.direct_dependencies,
        })
    }

    fn resolve_wasm_component<'a>(
        &'a self,
        package: &str,
        export_name: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        let image = self
            .libraries
            .iter()
            .find(|library| {
                library.package == package
                    && library.export_name == export_name
                    && library.abi_version == abi_version
                    && library.kind == PackageLibraryKind::WasmComponent
            })
            .ok_or(PackageImageError::NotFound)?;
        Ok(PackageLibrary {
            package_name: &image.package,
            export_name: &image.export_name,
            soname: &image.soname,
            image: PackageImage {
                bytes: &image.bytes,
                vmo: KernelHandle { raw: image.handle },
                vmo_offset: 0,
            },
            symbol_prefix: &image.symbol_prefix,
            abi_version: image.abi_version,
            kind: image.kind,
            direct_dependencies: &image.direct_dependencies,
        })
    }
}
impl Drop for BundleImage {
    fn drop(&mut self) {
        let _ = Memory::close(self.handle);
        for library in &self.libraries {
            let _ = Memory::close(library.handle);
        }
    }
}

struct ArchiveGuard(Option<u64>);
impl ArchiveGuard {
    fn new(handle: u64) -> Self {
        Self(Some(handle))
    }

    fn retain(mut self) -> u64 {
        self.0.take().unwrap_or(0)
    }
}
impl Drop for ArchiveGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            let _ = Memory::close(handle);
        }
    }
}

struct ArchiveMapping {
    va: u64,
    len: u64,
}
impl Drop for ArchiveMapping {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.va, self.len);
    }
}

fn duplicate_cached_image(image: crate::CachedImageVmo) -> Result<(Vec<u8>, u64), &'static str> {
    if image.handle == 0 || image.len == 0 || image.len > bexos_update::MAX_APP_ARTIFACT_BYTES {
        return Err("cached image size");
    }
    let rounded = bexos_boot::page_round(image.len).ok_or("cached image size")?;
    let duplicate =
        Memory::duplicate(image.handle, 1 | 2 | 16 | 32).map_err(|_| "cached image duplicate")?;
    let va = match Memory::map(duplicate, rounded, 2) {
        Ok(va) => va,
        Err(_) => {
            let _ = Memory::close(duplicate);
            return Err("cached image map");
        }
    };
    let mut bytes = Vec::new();
    bytes.resize(image.len as usize, 0);
    #[cfg(bexos_guest)]
    Memory::commit_range(bytes.as_mut_ptr() as u64, image.len)
        .map_err(|_| "cached image destination commit")?;
    unsafe {
        core::ptr::copy_nonoverlapping(va as *const u8, bytes.as_mut_ptr(), image.len as usize);
    }
    if Memory::unmap(va, rounded).is_err() {
        let _ = Memory::close(duplicate);
        return Err("cached image unmap");
    }
    Ok((bytes, duplicate))
}

fn stage_cached_libraries(
    cached: &crate::DriverRecoveryImage,
    dependencies: &[super::ResolvedLibraryDependency],
) -> Result<Vec<BundleLibrary>, &'static str> {
    let mut libraries = Vec::new();
    for dependency in dependencies {
        let Some(library) = cached.libraries.iter().find(|library| {
            library.package_id == dependency.package_name
                && library.export_name == dependency.export_name
                && library.abi_version == dependency.abi_version
                && library.kind == dependency.kind
        }) else {
            return Err("cached library missing");
        };
        if library.soname != dependency.soname
            || library.symbol_prefix != dependency.symbol_prefix
            || library.direct_dependencies != dependency.direct_dependencies
        {
            return Err("cached library metadata mismatch");
        }
        let (bytes, handle) = duplicate_cached_image(library.image)?;
        libraries.push(BundleLibrary {
            package: library.package_id.clone(),
            export_name: library.export_name.clone(),
            soname: library.soname.clone(),
            symbol_prefix: library.symbol_prefix.clone(),
            abi_version: library.abi_version,
            kind: library.kind,
            direct_dependencies: library.direct_dependencies.clone(),
            bytes,
            handle,
        });
    }
    Ok(libraries)
}

pub fn begin(
    state: &AppdState,
    kernel: &mut Kernel,
    handle: u64,
    len: u64,
    generation: u64,
    target: &str,
) -> Result<Update, String> {
    let guard = ArchiveGuard::new(handle);
    let Some(rounded) = bexos_boot::page_round(len)
        .filter(|_| len > 0 && len <= bexos_update::MAX_APP_ARTIFACT_BYTES)
    else {
        return Err("invalid migration archive size".into());
    };
    let va = Memory::map(handle, rounded, 2).map_err(|_| "archive mapping")?;
    let mapping = ArchiveMapping { va, len: rounded };
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) };
    let old = state
        .services
        .iter()
        .find(|s| s.package == target)
        .ok_or("service not managed by appd")?;
    log(&alloc::format!(
        "appd: migration staging begin target={target} generation={generation}\n"
    ));
    let source_markers = if target == "bexos.driver.storage.nvme" {
        let markers = nvme_markers(Channel(old.manager))?;
        log(&alloc::format!(
            "appd: NVMe source markers captured {},{},{}\n",
            markers[0],
            markers[1],
            markers[2]
        ));
        markers
    } else {
        [0; 3]
    };
    if generation <= old.generation {
        return Err("Rollback generation".into());
    }
    if old.generation == 0 {
        let floor = state
            .registry
            .record(target)
            .map(|record| record.accepted_generation)
            .unwrap_or(0);
        if generation <= floor {
            return Err("Rollback generation".into());
        }
    }
    let record = state
        .registry
        .record(target)
        .map_err(|_| "service manifest unavailable")?;
    #[cfg(feature = "persistent")]
    if state.stores.is_none() && record.protected {
        return Err("durable stores unavailable".into());
    }
    let old_manifest = Manifest::decode(&record.manifest_bytes).map_err(|_| "old manifest")?;
    let old_process = old_manifest
        .processes
        .iter()
        .find(|p| p.name == old.process)
        .ok_or("old process missing")?;
    if old_process.lifecycle.update_strategy != UpdateStrategy::HeartTransplant {
        return Err("service has not opted into heart transplant".into());
    }
    let archive = bexos_app_archive::OpenArchive::parse_and_verify(bytes, &trusted_keys())
        .map_err(|_| "platform archive signature rejected")?;
    log(&alloc::format!(
        "appd: migration archive verified target={target}\n"
    ));
    let manifest_entry = archive
        .find("package.bexmanifest")
        .ok_or("manifest missing")?;
    let manifest_bytes = archive
        .read_file(manifest_entry)
        .map_err(|_| "manifest read")?;
    log(&alloc::format!(
        "appd: migration manifest staged target={target} bytes={}\n",
        manifest_bytes.len()
    ));
    let manifest = Manifest::decode(&manifest_bytes).map_err(|_| "manifest decode")?;
    let freeze_preferences =
        !old_manifest.config_schema.fields.is_empty() || !manifest.config_schema.fields.is_empty();
    if manifest.package_name != target {
        return Err("signed target mismatch".into());
    }
    let mut candidate_record = record.clone();
    candidate_record.manifest_bytes = manifest_bytes.clone();
    candidate_record.display_name = manifest.name.clone();
    candidate_record.version = manifest.package_version.clone();
    candidate_record.multi_version_policy = manifest.multi_version_policy;
    candidate_record.min_bexos_abi_version = manifest.min_bexos_abi_version;
    candidate_record.archive_content_root = archive.content_root();
    candidate_record.signer_key_id = archive.key_id();
    candidate_record.accepted_generation = generation;
    let archive_hash: [u8; 32] = blake3::hash(bytes).into();
    let staged_package = super::migration_archive::name(target, generation, &archive_hash);
    // Stage the verified archive before starting the kernel's migration clock.
    // The activated registry record and this file are made durable together by
    // appd's post-activation BexFS namespace sync. Boot imports ignore staged
    // generation files; only registry activation can select one, so rejection
    // or a crash before that joint commit retains the previous durable source.
    candidate_record.archive_path = alloc::format!("pkg/{staged_package}.bex");
    let process = manifest
        .processes
        .iter()
        .find(|p| p.name == old.process)
        .ok_or("replacement process missing")?;
    if process.lifecycle.update_strategy != UpdateStrategy::HeartTransplant {
        return Err("replacement migration support missing".into());
    }
    if process.shell_role != old_process.shell_role
        || (old_process.shell_role != crate::manifest::ShellRole::None
            && crate::shell::entrypoint(&manifest, old_process.shell_role).is_none())
    {
        return Err("replacement must preserve the selected shell role and entrypoint".into());
    }
    let adapter = super::migration_adapter::prepare(process)?;
    let entry = archive
        .find(
            adapter
                .executable_path
                .strip_prefix("/pkg/")
                .ok_or("package executable path")?,
        )
        .ok_or("replacement executable missing")?;
    log(&alloc::format!(
        "appd: migration executable decompressing target={target} stored_bytes={} output_bytes={}\n",
        entry.stored_len,
        entry.uncompressed_size
    ));
    let elf = archive.read_file(entry).map_err(|_| "executable read")?;
    log(&alloc::format!(
        "appd: migration executable staged target={target} bytes={}\n",
        elf.len()
    ));
    let replacement_runtime = if process.runner == "wasm" {
        match archive.find(crate::runner::runtime_archive::ARCHIVE_PATH) {
            Some(entry) => {
                if entry.uncompressed_size
                    > crate::runner::runtime_archive::MAX_RUNTIME_ARCHIVE_BYTES as u64
                {
                    return Err("runtime archive size".into());
                }
                let bytes = archive
                    .read_file(entry)
                    .map_err(|_| "runtime archive read")?;
                Some(
                    super::resolver::RuntimeImage::from_archive(&bytes)
                        .map_err(|_| "runtime platform signature or role rejected")?,
                )
            }
            None => None,
        }
    } else {
        None
    };
    bexos_userspace::vfs::write_package_archive(state.vfsd, &staged_package, bytes)
        .map_err(|error| alloc::format!("migration archive persistence {error:?}"))?;
    log(&alloc::format!(
        "appd: migration archive persisted target={target}\n"
    ));
    drop(archive);
    drop(mapping);
    let dependency_roots =
        super::resolve_library_dependencies_for_migration(&state.registry, &manifest)
            .map_err(|error| alloc::format!("library deps: {error:?}"))?;
    let libraries = match state
        .driver_images
        .get(target, &old.process)
        .filter(|_| old.hardware != 0)
    {
        Some(cached) => {
            let libraries = stage_cached_libraries(cached, &dependency_roots)
                .map_err(|error| alloc::format!("cached library deps: {error}"))?;
            log(&alloc::format!(
                "appd: migration libraries staged target={target} count={} source=driver-cache\n",
                libraries.len()
            ));
            libraries
        }
        None => {
            let mut libraries = Vec::new();
            for dependency in &dependency_roots {
                let archive_bytes =
                    bexos_userspace::vfs::read_package_archive(state.vfsd, &dependency.archive_id)
                        .map_err(|_| "library archive read")?;
                let archive = bexos_app_archive::OpenArchive::parse(&archive_bytes)
                    .map_err(|_| "library archive parse")?;
                let path = dependency
                    .export_path
                    .strip_prefix("/pkg/")
                    .unwrap_or(&dependency.export_path);
                let entry = archive.find(path).ok_or("library missing")?;
                let bytes = archive.read_file(entry).map_err(|_| "library read")?;
                let handle = Memory::from_bytes(&bytes).map_err(|_| "library VMO")?;
                libraries.push(BundleLibrary {
                    package: dependency.package_name.clone(),
                    export_name: dependency.export_name.clone(),
                    soname: dependency.soname.clone(),
                    symbol_prefix: dependency.symbol_prefix.clone(),
                    abi_version: dependency.abi_version,
                    kind: dependency.kind,
                    direct_dependencies: dependency.direct_dependencies.clone(),
                    bytes,
                    handle,
                });
            }
            log(&alloc::format!(
                "appd: migration libraries staged target={target} count={} source=package-store\n",
                libraries.len()
            ));
            libraries
        }
    };
    super::close_dependency_roots(dependency_roots);
    let image = BundleImage {
        runtime: replacement_runtime.or_else(|| {
            super::resolver::default_runtime_image(state.vfsd, &state.registry, &manifest)
        }),
        package: target.to_string(),
        path: adapter.executable_path.to_string(),
        handle: Memory::from_bytes(&elf).map_err(|_| "executable VMO")?,
        bytes: elf,
        libraries,
    };
    let mut deferred = runner::deferred::Deferred::new(kernel);
    let mut replacement = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::PlatformCore,
                identity: PackageIdentity {
                    package_id: target,
                    signer: "bexos_official_platform_v1",
                    trust_tier: PackageTrustTier::PlatformCore,
                    is_driver: old.hardware != 0,
                },
                runner_policy: Some(&state.config.runner_policy),
                hardware_access: match old.hardware {
                    0 => HardwareAccessTier::None,
                    1 => HardwareAccessTier::Isolated,
                    _ => HardwareAccessTier::Direct,
                },
                realtime_scheduling: crate::runner::realtime_scheduling_for(
                    &manifest,
                    process,
                    PackageIdentity {
                        package_id: target,
                        signer: "bexos_official_platform_v1",
                        trust_tier: PackageTrustTier::PlatformCore,
                        is_driver: old.hardware != 0,
                    },
                    Some(&state.config.runner_policy),
                ),
                resource_group_id: 1,
            },
            &mut deferred,
            &image,
        )
        .map_err(|e| alloc::format!("replacement launch {e:?}"))?;
    log(&alloc::format!(
        "appd: replacement created target={target} process={}\n",
        replacement.process_handle.raw
    ));
    // Account startup against the same preparation budget as the kernel.
    let started = now_ms();
    bexos_userspace::migration::begin(
        old.process_handle,
        replacement.process_handle.raw,
        generation,
        process.lifecycle.preparation_timeout_ms,
        process.lifecycle.migration_timeout_ms,
    )
    .map_err(|e| alloc::format!("kernel handover {e:?}"))?;
    log(&alloc::format!(
        "appd: kernel handover staged target={target}\n"
    ));
    let target_channel = Channel(replacement.service_manager_handle.raw);
    if let Err(e) = Startup::send_migratable_with_service_grants_config_and_linker_data(
        target_channel,
        &[],
        0,
        0,
        "",
        None,
        generation,
        true,
        &[],
        None,
        replacement
            .runtime_linker_data
            .map(|(handle, len)| (handle.raw, len)),
    ) {
        let _ = bexos_userspace::migration::abort();
        return Err(alloc::format!("candidate bootstrap {e:?}"));
    }
    log(&alloc::format!(
        "appd: replacement startup delivered target={target}\n"
    ));
    replacement.main_thread_handle = deferred.start().map_err(|e| {
        let _ = bexos_userspace::migration::abort();
        alloc::format!("candidate start {e:?}")
    })?;
    if target == "bexos.service.teed" {
        super::readiness::pin_secure_monitor_thread(replacement.main_thread_handle.raw).map_err(
            |error| {
                let _ = bexos_userspace::migration::abort();
                alloc::format!("candidate secure-monitor affinity {error:?}")
            },
        )?;
    }
    log(&alloc::format!(
        "appd: replacement thread started target={target}\n"
    ));
    drop(image);
    let retained_archive = guard.retain();
    Ok(Update {
        candidate_record,
        archive_handle: retained_archive,
        archive_len: len,
        package: target.to_string(),
        generation,
        source: Channel(old.migration),
        target: target_channel,
        replacement,
        step: Step::Prepare,
        waiting: false,
        started,
        preparation_ms: process.lifecycle.preparation_timeout_ms,
        cutover_ms: process.lifecycle.migration_timeout_ms,
        sequence: 0,
        delta_records: 0,
        bulk_records: 0,
        record: Vec::new(),
        markers: [0; 3],
        source_markers,
        freeze_preferences,
    })
}

impl Update {
    pub fn archive(&self) -> (u64, u64) {
        (self.archive_handle, self.archive_len)
    }
    pub fn freezes_preferences(&self) -> bool {
        self.freeze_preferences
    }
    pub fn replacement_descriptors(&self) -> (u64, u64, u64) {
        (
            self.replacement.process_handle.raw,
            self.replacement.address_space_handle.raw,
            self.replacement.main_thread_handle.raw,
        )
    }
    pub fn completion_message(&self) -> String {
        alloc::format!(
            "migration completed before={},{},{} markers={},{},{}",
            self.source_markers[0],
            self.source_markers[1],
            self.source_markers[2],
            self.markers[0],
            self.markers[1],
            self.markers[2]
        )
    }
    pub fn phase(&self) -> &'static str {
        match self.step {
            Step::Bulk | Step::AdoptBulk | Step::FinishBulk => "live bulk sync",
            Step::Delta | Step::AdoptDelta => "catching up deltas",
            Step::Quiesce | Step::Validate | Step::Activate => "cutover",
            _ => "preparing",
        }
    }
    pub fn poll(
        &mut self,
        state: &mut AppdState,
        source: &mut bexos_userspace::live_migration::Source,
    ) -> Result<bool, String> {
        // Drain already available protocol progress before scanning all normal
        // service endpoints again. In a self replacement, the source endpoint
        // belongs to this loop too, so dispatch it between coordinator steps.
        // Stop immediately on a pending receive and bound the work per turn.
        for _ in 0..8 {
            let before = (self.step, self.waiting);
            if self.poll_step(state)? {
                return Ok(true);
            }
            if before == (self.step, self.waiting) {
                break;
            }
            source
                .poll(state)
                .map_err(|error| alloc::format!("migration source rejected: {error:?}"))?;
        }
        Ok(false)
    }
    fn preparation_timeout(&self) -> String {
        alloc::format!(
            "migration preparation timeout step={:?} bulk_records={} delta_records={}",
            self.step,
            self.bulk_records,
            self.delta_records,
        )
    }
    fn poll_step(&mut self, state: &mut AppdState) -> Result<bool, String> {
        if now_ms().saturating_sub(self.started) >= self.preparation_ms as u64 {
            return Err(self.preparation_timeout());
        }
        let old = matches!(
            self.step,
            Step::Prepare | Step::Bulk | Step::Delta | Step::Quiesce
        );
        let channel = if old { self.source } else { self.target };
        if !self.waiting {
            if matches!(self.step, Step::Activate) {
                log("appd: sending migration activation\n");
            }
            log(&alloc::format!(
                "appd: migration send step={:?} old={} waiting={}\n",
                self.step,
                old,
                self.waiting
            ));
            if self.send(channel).is_err() {
                return Err(if old {
                    "migration request send"
                } else {
                    "migration rejected status=-8"
                }
                .into());
            }
            self.waiting = true;
            return Ok(false);
        }
        let m = match channel.try_recv() {
            Ok(m) => m,
            Err(kernel_fidl::Status::ErrTimedOut) => return Ok(false),
            Err(error) => {
                log(&alloc::format!(
                    "appd: migration receive failed step={:?} error={error:?}\n",
                    self.step
                ));
                // The kernel may expire the deadline between our initial
                // time check and this receive, closing the candidate endpoint.
                if error == kernel_fidl::Status::ErrPeerClosed
                    && now_ms().saturating_sub(self.started) >= self.preparation_ms as u64
                {
                    return Err(self.preparation_timeout());
                }
                return Err("migration peer closed".into());
            }
        };
        if !m.handles.is_empty() {
            for h in m.handles {
                let _ = Memory::close(h);
            }
            return Err("unexpected migration handles".into());
        }
        if m.bytes.len() < 16 {
            return Err("migration response framing".into());
        }
        if u64::from_le_bytes(m.bytes[..8].try_into().unwrap()) != self.generation {
            return Ok(false);
        }
        let expected_ordinal = match self.step {
            Step::Prepare | Step::Initialize => 1,
            Step::Bulk | Step::AdoptBulk => 2,
            Step::FinishBulk | Step::Delta => 3,
            Step::AdoptDelta | Step::Quiesce => 4,
            Step::Validate => 5,
            Step::Activate => 6,
        };
        let response_ordinal = u64::from_le_bytes(m.bytes[8..16].try_into().unwrap());
        log(&alloc::format!(
            "appd: migration recv step={:?} response_ordinal={} bytes={}\n",
            self.step,
            response_ordinal,
            m.bytes.len()
        ));
        if response_ordinal != expected_ordinal {
            return Err("migration response ordinal".into());
        }
        macro_rules! decode {
            ($t:ty) => {
                <$t>::decode(&m.bytes[16..], &[]).map_err(|_| "migration response decode")?
            };
        }
        self.waiting = false;
        match self.step {
            Step::Prepare => {
                let q = decode!(MigratablePrepareResponse);
                check(q.status)?;
                bexos_userspace::migration::begin_bulk()
                    .map_err(|_| "migration begin bulk".to_string())?;
                self.sequence = q.base_sequence;
                self.step = Step::Initialize;
            }
            Step::Initialize => {
                check(decode!(StateReceiverInitializeResponse).status)?;
                self.step = Step::Bulk;
            }
            Step::Bulk => {
                let q = decode!(MigratableNextBulkResponse);
                check(q.status)?;
                if q.done {
                    self.step = Step::FinishBulk;
                } else {
                    self.record = q.record.to_vec();
                    self.step = Step::AdoptBulk;
                }
            }
            Step::AdoptBulk => {
                check(decode!(StateReceiverAdoptBulkResponse).status)?;
                self.bulk_records = self.bulk_records.saturating_add(1);
                self.step = Step::Bulk;
            }
            Step::FinishBulk => {
                check(decode!(StateReceiverCompleteBulkResponse).status)?;
                self.step = Step::Delta;
            }
            Step::Delta => {
                let q = decode!(MigratableNextDeltaResponse);
                check(q.status)?;
                self.sequence = q.sequence;
                if q.caught_up {
                    self.step = Step::Quiesce;
                } else {
                    self.record = q.record.to_vec();
                    self.step = Step::AdoptDelta;
                }
            }
            Step::AdoptDelta => {
                check(decode!(StateReceiverAdoptDeltaResponse).status)?;
                self.delta_records = self.delta_records.saturating_add(1);
                // A continuously mutating hot key can keep the coalesced dirty
                // set non-empty forever. After bounded live catch-up, quiesce
                // and let the source drain the final frozen set.
                self.step = if self.delta_records >= 1 {
                    Step::Quiesce
                } else {
                    Step::Delta
                };
            }
            Step::Quiesce => {
                let q = decode!(MigratableQuiesceResponse);
                if q.status == 1 {
                    self.step = Step::Delta;
                } else {
                    check(q.status)?;
                    self.sequence = q.final_sequence;
                    self.step = Step::Validate;
                    self.waiting = true;
                }
            }
            Step::Validate => {
                check(decode!(StateReceiverValidateResponse).status)?;
                self.step = Step::Activate;
                self.waiting = true;
            }
            Step::Activate => {
                log("appd: received migration activation response\n");
                let activated = decode!(StateReceiverActivateResponse);
                check(activated.status)?;
                self.markers = [activated.marker0, activated.marker1, activated.marker2];
                self.finish_activation(state)?;
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn finish_activation(&self, state: &mut AppdState) -> Result<(), String> {
        log("appd: finishing migration activation\n");
        let s = state
            .services
            .iter_mut()
            .find(|s| s.package == self.package)
            .ok_or("service disappeared")?;
        state
            .devices
            .replace_process_handle(s.process_handle, self.replacement.process_handle.raw);
        for h in [s.process_handle, s.space_handle, s.thread_handle] {
            if h != 0 {
                log(&alloc::format!("appd: closing retired descriptor={h}\n"));
                let _ = Memory::close(h);
            }
        }
        log("appd: retired descriptors closed\n");
        // Preinstalled services also have launch/watchdog records. Preserve
        // their manager endpoints while moving process ownership to the image
        // that is now active, before the watchdog examines the retired image.
        for launch in &mut state.launches {
            if launch.process_handle == s.process_handle {
                launch.process_handle = self.replacement.process_handle.raw;
                launch.space_handle = self.replacement.address_space_handle.raw;
                launch.thread_handle = self.replacement.main_thread_handle.raw;
            }
        }
        s.process_handle = self.replacement.process_handle.raw;
        s.space_handle = self.replacement.address_space_handle.raw;
        s.thread_handle = self.replacement.main_thread_handle.raw;
        s.generation = self.generation;
        log("appd: activating replacement registry record\n");
        state
            .activate_record()
            .map_err(|error| alloc::format!("replacement registry activation {error:?}"))?;
        log("appd: replacement registry record activated\n");
        let _ = Memory::close(self.target.0);
        log("appd: migration activation bookkeeping complete\n");
        Ok(())
    }
    fn send(&self, channel: Channel) -> Result<(), ()> {
        match self.step {
            Step::Prepare => send(
                channel,
                1,
                &MigratablePrepareRequest {
                    candidate: HandleRef {
                        raw: Memory::duplicate(self.target.0, 1 | 2 | 4 | 32).map_err(|_| ())?,
                    },
                    generation: self.generation,
                    version: bexos_migration::VERSION,
                    preparation_timeout_ms: self.preparation_ms,
                    cutover_timeout_ms: self.cutover_ms,
                },
            ),
            Step::Initialize => send(
                channel,
                1,
                &StateReceiverInitializeRequest {
                    generation: self.generation,
                    version: bexos_migration::VERSION,
                    base_sequence: self.sequence,
                },
            ),
            Step::Bulk => send(channel, 2, &MigratableNextBulkRequest {}),
            Step::AdoptBulk => send(
                channel,
                2,
                &StateReceiverAdoptBulkRequest {
                    record: &self.record,
                },
            ),
            Step::FinishBulk => send(channel, 3, &StateReceiverCompleteBulkRequest {}),
            Step::Delta => send(channel, 3, &MigratableNextDeltaRequest {}),
            Step::AdoptDelta => send(
                channel,
                4,
                &StateReceiverAdoptDeltaRequest {
                    record: &self.record,
                },
            ),
            Step::Quiesce => send(channel, 4, &MigratableQuiesceRequest {}),
            Step::Validate => send(
                channel,
                5,
                &StateReceiverValidateRequest {
                    final_sequence: self.sequence,
                },
            ),
            Step::Activate => send(channel, 6, &StateReceiverActivateRequest {}),
        }
    }
    pub fn abort(&self) {
        let _ = bexos_userspace::migration::abort();
        let _ = send(self.source, 6, &MigratableAbortRequest {});
        let _ = Memory::close(self.archive_handle);
        for h in [
            self.target.0,
            self.replacement.process_handle.raw,
            self.replacement.address_space_handle.raw,
            self.replacement.main_thread_handle.raw,
        ] {
            if h != 0 {
                let _ = Memory::close(h);
            }
        }
    }
}
fn check(status: i32) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(alloc::format!("migration rejected status={status}"))
    }
}
fn send<Q: FidlEncode>(channel: Channel, ordinal: u64, q: &Q) -> Result<(), ()> {
    let mut bytes = alloc::vec![0; 65500];
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    let mut handles = [HandleRef { raw: 0 }; 4];
    let e = q.encode(&mut bytes[8..], &mut handles).map_err(|_| ())?;
    let hs: Vec<_> = handles[..e.handles].iter().map(|h| h.raw).collect();
    channel.send(&bytes[..8 + e.bytes], &hs).map_err(|_| {
        for h in hs {
            let _ = Memory::close(h);
        }
    })
}
fn trusted_keys() -> [bexos_app_archive::TrustedKey<'static>; 1] {
    [bexos_app_archive::TrustedKey {
        key_id: *b"bexos-qemu-test-ed25519-key-v001",
        public_key: &[
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ],
    }]
}
fn nvme_markers(channel: Channel) -> Result<[u64; 3], String> {
    use block_fidl::{FidlDecode, FidlEncode};
    let q = block_fidl::BlockDeviceGetMigrationMarkersRequest {};
    let mut bytes = [0u8; 8];
    let e = q
        .encode(&mut bytes, &mut [])
        .map_err(|_| "NVMe marker encode")?;
    let m = Rpc(channel)
        .call_raw(5, &bytes[..e.bytes], &[], true)
        .map_err(|_| "NVMe marker request")?;
    let r = block_fidl::BlockDeviceGetMigrationMarkersResponse::decode(&m.bytes, &[])
        .map_err(|_| "NVMe marker response")?;
    Ok([r.sq_physical, r.cq_physical, r.payload_physical])
}
