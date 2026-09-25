mod command_runtime;
mod graphics;
mod input_hotplug;
mod launch_migration;
mod locale;
mod migration_adapter;
mod migration_archive;
mod preferences;
mod readiness;
mod resolver;
mod shell;
mod shell_users;
pub mod state;
mod tee_binding;
mod trace_registry;
mod update;

const USERS_STORAGE_READY_TIMEOUT_SECONDS: u64 = 420;
use crate::{
    AppdBroker, AppdWaveOrchestrator, BoundCapability, CapabilityMetadata, ClientContext,
    ExposedService, HandlerId, HardwareAccessTier, KernelFidlOps, KernelHandle, LaunchRequest,
    LibraryExportKind, Lifecycle, LinkType, Manifest, MemoryOpenerRegistry, MemoryPermissionStore,
    OpenKind, OpenerBinding, OpenerScope, PackageIdentity, PackageLibraryKind, PackageTrustTier,
    PermissionDeclaration, PermissionRequirement, PermissionValueGrant, PlatformConfig,
    ResolveOutcome, ResolvedHandler, RunnerRegistry, SYSTEM_UID, SharedVaultNamespaceEntry,
    Visibility, app_storage_namespace_with_shared_vaults_and_dependencies, publish_kernel_services,
    register_manifest_openers,
};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use app_lifecycle_fidl as lifecycle;
use app_opener_fidl as opener_fidl;
use app_service_directory_fidl as service_directory;
use app_version_manager_fidl as version_manager;
use app_worker::{FidlDecode as WorkerDecode, FidlEncode as WorkerEncode};
use app_worker_fidl as app_worker;
use bexos_app_registry::{
    HealthCheckStatus, InstallSource, LifecycleState, MemoryAppRegistry, parse_package_selector,
};
use bexos_bexfs::sys_state::{SYS_STATE_V1_BYTES, SYS_STATE_V2_BYTES, SysStateV2};
use bexos_boot::{
    BOOT_EVIDENCE_FLAG_IOMMU_STRICT, BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK,
    BOOT_EVIDENCE_FLAG_SECURE_BOOT, BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR,
    BOOT_EVIDENCE_VERIFIED_BL33_VERSION, BootEvidenceV1,
};
use bexos_crypto::verify_ed25519;
use bexos_kernel_core::bootfs::Bootfs;
use bexos_trusty_client::protocol::{AVB_UUID, KEYMINT_UUID};
use bexos_trusty_client::services::{
    AVB_CMD_GET_VERSION, AVB_CMD_LOCK_BOOT_STATE, AVB_CMD_READ_LOCK_STATE,
    AVB_CMD_READ_ROLLBACK_INDEX, AVB_CMD_WRITE_LOCK_STATE, AVB_CMD_WRITE_ROLLBACK_INDEX,
    KEYMINT_CMD_SET_BOOT_INFO, KEYMINT_CMD_SET_HAL_INFO, decode_avb_empty, decode_avb_lock_state,
    decode_avb_u64, decode_avb_version, decode_keymint_set_boot_info, decode_keymint_set_hal_info,
    encode_avb_get_version, encode_avb_lock_boot_state, encode_avb_read_lock_state,
    encode_avb_rollback_index, encode_avb_write_lock_state, encode_keymint_set_boot_info,
    encode_keymint_set_hal_info,
};
use bexos_userspace::config::MAX_CONFIG_SNAPSHOT_LEN;
use bexos_userspace::live_migration::{Source, State};
use bexos_userspace::{Channel, KernelTransport, Memory, Rpc, ServiceGrant, Startup, fs, log, vfs};
use lifecycle::{FidlDecode as LifecycleDecode, FidlEncode as LifecycleEncode};
use opener_fidl::{FidlDecode as OpenerDecode, FidlEncode as OpenerEncode};
use service_directory::{
    FidlDecode as ServiceDirectoryDecode, FidlEncode as ServiceDirectoryEncode,
};
use sha2::{Digest, Sha256};
use tee_manager::{
    FidlDecode as TeeDecode, FidlEncode as TeeEncode, HandleRef as TeeHandleRef,
    TeeManagerActivateTrustedAppPackageRequest, TeeManagerActivateTrustedAppPackageResponse,
    TeeManagerCloseSessionRequest, TeeManagerCloseSessionResponse,
    TeeManagerDeactivateTrustedAppPackageRequest, TeeManagerDeactivateTrustedAppPackageResponse,
    TeeManagerInvokeCommandRequest, TeeManagerInvokeCommandResponse, TeeManagerOpenSessionRequest,
    TeeManagerOpenSessionResponse, TeeManagerQueryTrustedAppPackageRequest,
    TeeManagerQueryTrustedAppPackageResponse, TeeStatus, WireStringVector as TeeWireStringVector,
};
use tee_manager_fidl as tee_manager;
use user_manager::{FidlDecode as UserDecode, FidlEncode as UserEncode};
use user_manager_fidl as user_manager;
use version_manager::{FidlDecode as VersionDecode, FidlEncode as VersionEncode};

const NO_WAIT_DEADLINE_NANOS: i64 = 0;

pub async fn main(channel: u64) -> ! {
    if let Err(error) = boot(Channel(channel)).await {
        log(&format!(
            "appd: boot failed: {error}; pivot not completed\n"
        ));
        bexos_userspace::exit();
    }
    loop {
        async_yield().await;
    }
}

async fn boot(initial: Channel) -> Result<(), String> {
    log("appd: boot receive startup\n");
    let startup = Startup::receive(initial).map_err(|e| format!("bootstrap {e:?}"))?;
    log("appd: boot startup received\n");
    if startup.migration_target {
        let state = bexos_userspace::live_migration::receive::<state::AppdState>(
            initial,
            startup.migration_generation,
        )
        .map_err(|e| format!("appd adoption {e:?}"))?;
        serve_lifecycle(state, None).await;
    }
    Memory::close(initial.0).unwrap();
    let boot_handle = startup.resources[0];
    let boot_len = startup.arg0;
    log("appd: boot map bootfs\n");
    let va = Memory::map(boot_handle, boot_len, 2).map_err(|e| format!("BootFS map {e:?}"))?;
    let boot =
        Bootfs::parse(unsafe { core::slice::from_raw_parts(va as *const u8, boot_len as usize) })
            .map_err(|e| format!("BootFS {e:?}"))?;
    log("appd: boot parse config\n");
    let config_bytes = boot
        .find("/boot/platform.pcfg")
        .unwrap()
        .ok_or("missing platform policy")?
        .bytes
        .to_vec();
    let config = PlatformConfig::decode(&config_bytes).map_err(|e| format!("policy {e:?}"))?;
    let verified_boot = validate_secure_bootstrap(&startup, &config, &config_bytes)?;
    let assembly_provenance = boot_package_provenance(&boot)?;
    let mut manifests = Vec::new();
    let mut manifest_caches = Vec::new();
    for i in 0..boot.count() {
        let e = boot.entry(i).unwrap();
        if e.path.ends_with(".bexmanifest") {
            manifests.push(Manifest::decode(e.bytes).map_err(|e| format!("manifest {e:?}"))?);
            manifest_caches.push((e.path.to_string(), e.bytes.to_vec()));
        }
    }
    log("appd: boot manifests loaded\n");
    let mut app_registry = MemoryAppRegistry::new();
    let mut permission_store = MemoryPermissionStore::new();
    let mut opener_registry = MemoryOpenerRegistry::new();
    let mut launches = Vec::new();
    let mut opener_bindings = Vec::new();
    let mut version_manager_bindings = Vec::new();
    let mut app_manager_bindings = Vec::new();
    let mut worker_launcher_bindings = Vec::new();
    let mut service_directory_bindings = Vec::new();
    let mut lazy_state = crate::lazy::LazyActivationState::default();
    let mut permission_routes = crate::PermissionRouteTable::new();
    for (path, bytes) in &manifest_caches {
        let _ =
            app_registry.import_manifest_cache(bytes, InstallSource::Bootfs, true, path.as_str());
    }
    log("appd: boot manifest cache imported\n");
    for manifest in &manifests {
        let _ = permission_store.register_system_declarations(
            &manifest.package_name,
            &manifest.permissions,
            prior_generation_seed(&manifest.package_name),
        );
        register_manifest_openers(&mut opener_registry, OpenerScope::System, manifest, false);
    }
    log("appd: boot manifest policy registered\n");
    let key_bytes = boot
        .find("/boot/qemu-test.key")
        .unwrap()
        .ok_or("missing QEMU test key")?
        .bytes;
    let text = core::str::from_utf8(key_bytes)
        .map_err(|_| "key encoding")?
        .trim();
    if text.len() != 64 {
        return Err("key length".into());
    }
    let mut key = [0; 32];
    for i in 0..32 {
        key[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| "key hex")?;
    }
    let key_vmo = Memory::from_bytes(&key).unwrap();
    log("appd: boot qemu key vmo ready\n");
    key.fill(0);
    log("appd: boot resolver build begin\n");
    let resolver = resolver::Resolver::boot(&boot, boot_handle, &manifests);
    log("appd: boot resolver ready\n");
    let mut gate = readiness::Gate::new();
    if startup.resources.len() >= 4 {
        gate.framebuffer = Some((startup.resources[2], startup.resources[3]));
    }
    gate.trust_tls_roots = boot
        .find("/system/certs/tls_roots.redb")
        .unwrap()
        .ok_or("missing TLS roots")?
        .bytes
        .to_vec();
    gate.trust_app_roots = boot
        .find("/system/certs/app_signing_roots.redb")
        .unwrap()
        .ok_or("missing app signing roots")?
        .bytes
        .to_vec();
    let mut kernel = KernelFidlOps::new(KernelTransport(1), KernelTransport(2), KernelTransport(4));
    let mut broker = AppdBroker::new();
    publish_kernel_services(&mut broker).map_err(|e| format!("broker {e:?}"))?;
    publish_appd_opener_service(&mut broker).map_err(|e| format!("opener broker {e:?}"))?;
    publish_appd_version_manager_service(&mut broker)
        .map_err(|e| format!("version manager broker {e:?}"))?;
    publish_appd_manager_service(&mut broker).map_err(|e| format!("app manager broker {e:?}"))?;
    publish_appd_worker_launcher_service(&mut broker)
        .map_err(|e| format!("worker launcher broker {e:?}"))?;
    publish_appd_device_registry_service(&mut broker)
        .map_err(|e| format!("device registry broker {e:?}"))?;
    publish_appd_service_directory_service(&mut broker)
        .map_err(|e| format!("service directory broker {e:?}"))?;
    log("appd: boot launch driver waves\n");
    let orchestrator = AppdWaveOrchestrator::new();
    orchestrator
        .launch_automatic_with_policy(
            &manifests,
            &config.runner_policy,
            &config.driver_policy,
            |m| {
                let external = assembly_provenance.get(&m.package_name);
                PackageIdentity {
                    package_id: &m.package_name,
                    signer: external
                        .map_or("bexos_official_platform_v1", |entry| entry.signer.as_str()),
                    trust_tier: if external.is_some() {
                        PackageTrustTier::StandardConsumer
                    } else {
                        PackageTrustTier::SystemHardware
                    },
                    is_driver: external.is_some_and(|entry| entry.role == "driver")
                        || m.driver_info.is_some()
                        || m.package_name.starts_with("bexos.driver."),
                }
            },
            5,
            &mut kernel,
            &resolver,
            &mut gate,
        )
        .map_err(|e| format!("driver readiness {e:?}"))?;
    service_directory_bindings.append(&mut gate.service_directory_bindings);
    log("appd: boot driver waves ready\n");
    if let Some((framebuffer, descriptor)) = gate.framebuffer.take() {
        let _ = Memory::close(framebuffer);
        let _ = Memory::close(descriptor);
    }
    let mut boot_graphics = graphics::BootGraphics::connect(&gate);
    boot_graphics.report(1, 30, "Drivers available");
    publish_running_services(
        &mut broker,
        &manifests,
        &gate.services,
        &gate.registry,
        &config,
    )
    .map_err(|e| format!("service broker publish {e:?}"))?;
    let storage = gate.bexfs.ok_or("BexFS missing")?;
    let user_storage = gate.user_bexfs.ok_or("user BexFS missing")?;
    let diskimage = gate.diskimage.ok_or("DiskImage missing")?;
    let archivefs = gate.archivefs.ok_or("ArchiveFS missing")?;
    let memfs = gate.memfs.ok_or("MemFS missing")?;
    let vfsd = gate.vfsd.ok_or("vfsd missing")?;
    let debugd = gate.debugd.ok_or("debugd missing")?;
    let block = gate.nvme.ok_or("NVMe missing")?;
    let teed = gate.teed.ok_or("teed missing")?;
    if let Some(evidence) = verified_boot {
        let rpmb = gate.rpmb.ok_or("RPMB transport driver missing")?;
        let (proxy, transport) = Channel::pair().map_err(|e| format!("RPMB channel {e:?}"))?;
        rpmb.send(b"bexos.rpmb.bind", &[transport.0])
            .map_err(|e| format!("RPMB driver bind {e:?}"))?;
        teed.send(b"bexos.rpmb.bind", &[proxy.0])
            .map_err(|e| format!("RPMB proxy bind {e:?}"))?;
        let ready = teed
            .recv_with_timeout(120)
            .map_err(|e| format!("RPMB proxy readiness {e:?}"))?;
        if ready.bytes != 0i32.to_le_bytes() || !ready.handles.is_empty() {
            return Err("RPMB proxy failed to initialize".into());
        }
        if config.tee_policy.rpmb_anti_rollback
            && evidence.flags & BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK == 0
        {
            validate_avb_rollback(teed, evidence.generation)?;
        }
        initialize_keymint_boot(teed, &evidence)?;
    }
    let _trustd = gate.trustd.ok_or("trustd missing")?;
    let powerd = gate.powerd.ok_or("powerd missing")?;
    let users = gate.users.ok_or("users missing")?;
    wire_power_devices(powerd, &[gate.pci, gate.nvme])
        .map_err(|e| format!("power device registration {e:?}"))?;
    vfs::initialize_package_store(
        vfsd,
        block,
        storage,
        user_storage,
        diskimage,
        archivefs,
        key_vmo,
        "STORAGE",
    )
    .map_err(|e| format!("vfsd package store init {e:?}"))?;
    // usersd starts before the package-backed system data directory exists.
    // Require durable user metadata before exposing the normal boot gate.
    users
        .send(b"bexos.storage.ready", &[])
        .map_err(|e| format!("users storage initialization request {e:?}"))?;
    // Creating the durable store includes filesystem transactions whose RPCs
    // have a 300-second deadline. The outer boot handshake needs additional
    // budget for usersd scheduling and reply delivery after those nested VFS
    // and redb calls complete on slow emulated storage.
    let users_storage = users
        .recv_with_timeout(USERS_STORAGE_READY_TIMEOUT_SECONDS)
        .map_err(|e| format!("users storage initialization response {e:?}"))?;
    if users_storage.bytes != 0i32.to_le_bytes() || !users_storage.handles.is_empty() {
        return Err("users persistent storage initialization failed".into());
    }
    vfs::initialize_tmp_manager(vfsd, memfs).map_err(|e| format!("vfsd tmp manager init {e:?}"))?;
    log("appd: boot import disk manifests\n");
    let disk_package_ids = import_preinstalled_package_manifests(
        vfsd,
        &mut app_registry,
        &mut permission_store,
        &gate.trust_app_roots,
    )?;
    replay_trusted_app_packages(vfsd, teed, &app_registry)
        .map_err(|e| format!("trusted-app replay {e:?}"))?;
    bind_preinstalled_drivers(
        vfsd,
        &disk_package_ids,
        &app_registry,
        &config,
        &mut kernel,
        &mut broker,
        &mut gate,
        &orchestrator,
    )?;
    publish_dormant_registry_services(&mut broker, &app_registry, &mut lazy_state)
        .map_err(|e| format!("lazy service publish {e:?}"))?;
    if gate.virtio_net.is_some() {
        wire_power_devices(powerd, &[gate.virtio_net])
            .map_err(|e| format!("virtio-net power registration {e:?}"))?;
    }
    let driver_images = cache_boot_driver_recovery_images(&boot, &manifests, &gate.services)?;
    drop(resolver);
    drop(manifests);
    Memory::unmap(va, boot_len).map_err(|e| format!("BootFS unmap {e:?}"))?;
    Memory::close(boot_handle).map_err(|e| format!("BootFS close {e:?}"))?;
    let stats = Memory::stats().unwrap();
    log(&format!(
        "appd: post-BootFS memory free_pages={} reclaimed_pages={} reused_pages={}\n",
        stats.free_pages, stats.reclaimed_pages, stats.reused_pages
    ));
    if stats.bootfs_pages != 0 || stats.reclaimed_pages != boot_len / 4096 {
        return Err("BootFS pages remain referenced".into());
    }
    let bootfs_reclaimed_pages = stats.reclaimed_pages;
    let sys = fs::mount(storage, block, "SYS_STATE", key_vmo, false)
        .map_err(|e| format!("SYS_STATE mount {e:?}"))?;
    Memory::close(key_vmo).unwrap();
    boot_graphics.report(2, 55, "Storage mounted");
    boot_graphics.report(3, 70, "Drivers initialized");
    log("appd: /pkg mounted read-only via vfsd; app /data roots scoped by package\n");
    let file =
        fs::open(sys, "boot_state.bin", 1 | 2).map_err(|e| format!("SYS_STATE open {e:?}"))?;
    let mut state_bytes =
        fs::read(file, SYS_STATE_V2_BYTES as u64).map_err(|e| format!("SYS_STATE read {e:?}"))?;
    if state_bytes.len() == SYS_STATE_V2_BYTES
        && SysStateV2::decode(&state_bytes).is_err()
        && state_bytes[SYS_STATE_V1_BYTES..]
            .iter()
            .all(|byte| *byte == 0)
    {
        state_bytes.truncate(SYS_STATE_V1_BYTES);
    }
    let mut state =
        SysStateV2::decode_any(&state_bytes).map_err(|e| format!("SYS_STATE decode {e:?}"))?;
    let prior = state.generation;
    state.generation = state
        .generation
        .checked_add(1)
        .ok_or("generation overflow")?;
    state.sequence = state
        .sequence
        .checked_add(1)
        .ok_or("SYS_STATE sequence overflow")?;
    fs::seek(file, 0).map_err(|e| format!("SYS_STATE seek {e:?}"))?;
    fs::write(file, &state.encode()).map_err(|e| format!("SYS_STATE write {e:?}"))?;
    fs::close(file).map_err(|e| format!("SYS_STATE file close {e:?}"))?;
    fs::sync(sys).map_err(|e| format!("SYS_STATE sync {e:?}"))?;
    let sys_state_root = sys;
    log(&format!(
        "appd: guest SYS_STATE durable generation={} prior={}\n",
        state.generation, prior
    ));
    #[cfg(feature = "persistent")]
    let mut domain_associations = bexos_domain_association::MemoryDomainAssociationCache::new();
    #[cfg(not(feature = "persistent"))]
    let domain_associations = bexos_domain_association::MemoryDomainAssociationCache::new();
    #[cfg(feature = "persistent")]
    let persistent_sync_deferral = bexos_redb::bexos_fs::defer_file_syncs();
    #[cfg(feature = "persistent")]
    let persistent_stores = {
        let system_dir = vfs::get_system_data_directory(vfsd, state::APPD_PACKAGE)
            .map_err(|e| format!("persistent appd data dir {e:?}"))?;
        let stores = crate::stores::AppdStores::open(system_dir, sys_state_root, state.active_slot)
            .map_err(|e| format!("persistent appd stores open {e:?}"))?;
        stores
            .reopen_with_boot_state(
                &mut app_registry,
                &mut opener_registry,
                &mut domain_associations,
            )
            .map_err(|e| format!("persistent appd stores {e:?}"))?;
        crate::permission_persistence::load_system(vfsd, &mut permission_store)
            .map_err(|e| format!("persistent permission store {e:?}"))?;
        log("appd: persistent registry stores installed\n");
        Some(stores)
    };
    reconcile_missing_archives(vfsd, &mut app_registry)
        .map_err(|e| format!("package archive reconciliation {e:?}"))?;
    // Disk-installed handlers are launchable on demand, even when their process
    // has no startup wave. Rebuild after persistent active versions are loaded.
    for command_package in app_registry.list_packages() {
        if app_registry
            .record(&command_package.package_id)
            .is_ok_and(|active| active.version == command_package.version)
        {
            let manifest = Manifest::decode(&command_package.manifest_bytes)
                .map_err(|e| format!("installed opener manifest {e:?}"))?;
            register_manifest_openers(&mut opener_registry, OpenerScope::System, &manifest, false);
        }
    }
    #[cfg(feature = "persistent")]
    if let Some(stores) = &persistent_stores {
        stores
            .replace_from_memory(&app_registry, &opener_registry, &domain_associations)
            .map_err(|e| format!("persistent appd reconciliation {e:?}"))?;
    }
    crate::permission_persistence::sync_system(vfsd, &permission_store)
        .map_err(|e| format!("persistent permission reconciliation {e:?}"))?;
    log("appd: boot launch storage services\n");
    let mut boot_sysui = None;
    launch_preinstalled_storage_services(
        vfsd,
        users,
        &disk_package_ids,
        &mut app_registry,
        &mut launches,
        &mut gate.services,
        &mut kernel,
        &mut broker,
        &mut permission_routes,
        &mut permission_store,
        &mut opener_bindings,
        &mut version_manager_bindings,
        &mut app_manager_bindings,
        &mut worker_launcher_bindings,
        &mut service_directory_bindings,
        &mut lazy_state,
        &domain_associations,
        &config,
        &[],
        &mut boot_sysui,
    )?;
    log("appd: boot storage services ready\n");
    log(&format!(
        "appd: pivot complete; /boot removed; BootFS reclaimed pages={}\n",
        bootfs_reclaimed_pages
    ));
    if let Ok(archive_root) = vfs::get_package_directory(vfsd, "bexos.platform.storage_verify") {
        let manifest_file = fs::open(archive_root, "package.bexmanifest", 1)
            .map_err(|e| format!("archive manifest open {e:?}"))?;
        let manifest_bytes = fs::read(manifest_file, 32768).map_err(|e| {
            let _ = fs::close(manifest_file);
            format!("archive manifest read {e:?}")
        })?;
        let record = app_registry
            .record("bexos.platform.storage_verify")
            .cloned()
            .map_err(|e| format!("registry lookup storage verifier {e:?}"))?;
        app_registry
            .mark_lifecycle(&record.package_key(), LifecycleState::Launching)
            .map_err(|e| format!("registry launch mark {e:?}"))?;
        let manifest =
            Manifest::decode(&manifest_bytes).map_err(|e| format!("disk manifest decode {e:?}"))?;
        fs::close(manifest_file).unwrap();
        let dependency_roots = resolve_library_dependencies(vfsd, &app_registry, &manifest)
            .map_err(|e| format!("library dependencies {e:?}"))?;
        let shared_vault_roots = match resolve_shared_vaults(
            vfsd,
            &app_registry,
            &bexos_domain_association::MemoryDomainAssociationCache::new(),
            &record.package_id,
            SYSTEM_UID,
            &manifest,
        ) {
            Ok(vaults) => vaults,
            Err(e) => {
                close_dependency_roots(dependency_roots);
                return Err(format!("shared vaults {e:?}"));
            }
        };
        let disk_resolver = match resolver::Resolver::disk_with_dependencies(
            archive_root,
            &manifest,
            &dependency_roots,
            vfsd,
            &app_registry,
            None,
        ) {
            Ok(resolver) => resolver,
            Err(error) => {
                close_shared_vault_roots(shared_vault_roots);
                close_dependency_roots(dependency_roots);
                let _ = Memory::close(archive_root.0);
                return Err(format!("storage verifier image resolution {error:?}"));
            }
        };
        let launched = match RunnerRegistry::new().launch(
            &LaunchRequest {
                manifest: &manifest,
                process: &manifest.processes[0],
                trust_tier: PackageTrustTier::PlatformCore,
                identity: PackageIdentity {
                    package_id: &manifest.package_name,
                    signer: "bexos_official_platform_v1",
                    trust_tier: PackageTrustTier::PlatformCore,
                    is_driver: false,
                },
                runner_policy: Some(&config.runner_policy),
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: crate::runner::realtime_scheduling_for(
                    &manifest,
                    &manifest.processes[0],
                    PackageIdentity {
                        package_id: &manifest.package_name,
                        signer: "bexos_official_platform_v1",
                        trust_tier: PackageTrustTier::PlatformCore,
                        is_driver: false,
                    },
                    Some(&config.runner_policy),
                ),
                resource_group_id: 1,
            },
            &mut kernel,
            &disk_resolver,
        ) {
            Ok(launched) => launched,
            Err(e) => {
                close_shared_vault_roots(shared_vault_roots);
                close_dependency_roots(dependency_roots);
                return Err(format!("disk launch {e:?}"));
            }
        };
        drop(disk_resolver);
        let control = Channel(launched.service_manager_handle.raw);
        log("appd: storage verifier duplicate package root\n");
        let pkg_handle = Memory::duplicate(archive_root.0, 1 | 2 | 4 | 32)
            .map_err(|e| format!("pkg handle duplicate {e:?}"))?;
        let graphics_fast_path = u64::from(boot_graphics.active());
        let data_root = if graphics_fast_path == 0 {
            log("appd: storage verifier opening system data root\n");
            match vfs::get_system_data_directory(vfsd, &record.package_id) {
                Ok(root) => {
                    log("appd: storage verifier system data root ready\n");
                    root
                }
                Err(e) => {
                    let _ = Memory::close(pkg_handle);
                    close_shared_vault_roots(shared_vault_roots);
                    close_dependency_roots(dependency_roots);
                    return Err(format!("app data directory {e:?}"));
                }
            }
        } else {
            log("appd: storage verifier using tmp data root for graphics boot\n");
            match vfs::get_tmp_directory(
                vfsd,
                SYSTEM_UID,
                &record.package_id,
                "storage_verify_data",
            ) {
                Ok(root) => root,
                Err(e) => {
                    let _ = Memory::close(pkg_handle);
                    close_shared_vault_roots(shared_vault_roots);
                    close_dependency_roots(dependency_roots);
                    return Err(format!("graphics storage verifier data root {e:?}"));
                }
            }
        };
        log("appd: storage verifier opening tmp root\n");
        let tmp_root =
            match vfs::get_tmp_directory(vfsd, SYSTEM_UID, &record.package_id, "storage_verify") {
                Ok(root) => {
                    log("appd: storage verifier tmp root ready\n");
                    root
                }
                Err(e) => {
                    let _ = Memory::close(pkg_handle);
                    let _ = Memory::close(data_root.0);
                    close_shared_vault_roots(shared_vault_roots);
                    close_dependency_roots(dependency_roots);
                    return Err(format!("app tmp directory {e:?}"));
                }
            };
        let namespace_shared_vaults = shared_vault_roots
            .iter()
            .map(|vault| SharedVaultNamespaceEntry {
                name: vault.name.as_str(),
                directory: KernelHandle {
                    raw: vault.directory.0,
                },
            })
            .collect::<Vec<_>>();
        log("appd: storage verifier building namespace\n");
        let namespace = app_storage_namespace_with_shared_vaults_and_dependencies(
            KernelHandle { raw: pkg_handle },
            KernelHandle { raw: data_root.0 },
            KernelHandle { raw: tmp_root.0 },
            &namespace_shared_vaults,
            &dependency_roots
                .iter()
                .map(|dependency| crate::DependencyNamespaceEntry {
                    package_name: dependency.package_name.as_str(),
                    mount_alias: dependency.mount_alias.as_deref(),
                    directory: KernelHandle {
                        raw: dependency.directory.0,
                    },
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|e| {
            let _ = Memory::close(pkg_handle);
            let _ = Memory::close(data_root.0);
            let _ = Memory::close(tmp_root.0);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            format!("namespace {e:?}")
        })?;
        let namespace_entries = startup_namespace_entries(&namespace);
        log("appd: storage verifier resolving configuration\n");
        let (config_vmo, _) = preferences::launch(&[], archive_root, &record, SYSTEM_UID, &[])
            .map_err(|e| format!("bootstrap configuration {e:?}"))?;
        let linker_data = launched
            .runtime_linker_data
            .map(|(handle, len)| (handle.raw, len));
        let trace_producer = trace_registry::allocate_trace_producer(
            launched.process_handle.raw,
            launched.main_thread_handle.raw,
        )
        .map_err(|e| format!("trace buffer {e:?}"))?;
        let pending_trace = trace_registry::PendingTraceProducer::from_allocation(
            &manifest.package_name,
            &trace_producer,
        );
        log(&format!(
            "appd: storage verifier startup prior={prior} graphics_fast_path={graphics_fast_path}
"
        ));
        Startup::send_migratable_with_service_grants_namespace_config_linker_and_trace_data(
            control,
            &[],
            prior,
            graphics_fast_path,
            &namespace_entries,
            None,
            0,
            false,
            &[],
            config_vmo,
            None,
            linker_data,
            Some(trace_producer.startup),
        )
        .map_err(|e| format!("disk verify startup {e:?}"))?;
        trace_registry::register_now(None, pending_trace);
        Startup::wait_ready(control).map_err(|e| format!("disk verify {e:?}"))?;
        launches.push(state::LaunchRecord {
            package: record.package_id.clone(),
            process: manifest.processes[0].name.clone(),
            instance_id: String::new(),
            process_handle: launched.process_handle.raw,
            space_handle: launched.address_space_handle.raw,
            thread_handle: launched.main_thread_handle.raw,
            manager: control.0,
            progress: 0,
            uid: SYSTEM_UID,
            job_token: 0,
        });
        app_registry
            .mark_lifecycle(&record.package_key(), LifecycleState::Running)
            .map_err(|e| format!("registry running mark {e:?}"))?;
        let _ = permission_store.register_system_declarations(
            &manifest.package_name,
            &manifest.permissions,
            state.generation,
        );
        Memory::close(archive_root.0).map_err(|e| format!("archive root close {e:?}"))?;
        log(&format!(
            "appd: registry launched signed app package={}\n",
            record.package_id
        ));
    } else {
        log("appd: no signed disk verifier archive preinstalled; debugd test install required\n");
    }
    log("appd: proving reclaimed page reuse\n");
    // Reuse proof: keep probes live until first-fit allocation reaches the
    // reclaimed span, then release the probes before accepting debug clients.
    let before = Memory::stats().map_err(|e| format!("memory stats before proof {e:?}"))?;
    if before.reused_pages == 0 {
        let mut probes = Vec::new();
        while Memory::stats()
            .map_err(|e| format!("memory stats during proof {e:?}"))?
            .reused_pages
            == before.reused_pages
        {
            // Anonymous VMOs are lazy: creating an untouched VMO never proves
            // physical frame reuse. Batch physically backed allocations to
            // bound syscall overhead, falling back to one page for fragmented
            // free space. Each live probe consumes at least one physical page.
            if probes.len() as u64 >= before.free_pages.saturating_add(1) {
                for h in probes {
                    let _ = Memory::close(h);
                }
                return Err("bounded BootFS page reuse proof exhausted".into());
            }
            match Memory::create(64 * 4096, 2).or_else(|_| Memory::create(4096, 2)) {
                Ok(h) => probes.push(h),
                Err(_) => {
                    for h in probes {
                        let _ = Memory::close(h);
                    }
                    return Err("no BootFS page reuse".into());
                }
            }
        }
        for h in probes {
            Memory::close(h).map_err(|e| format!("proof VMO close {e:?}"))?;
        }
    }
    let stats = Memory::stats().map_err(|e| format!("memory stats after proof {e:?}"))?;
    log(&format!(
        "appd: reclaimed physical pages reused={}\n",
        stats.reused_pages
    ));
    let (lifecycle_client, lifecycle_server) =
        Channel::pair().map_err(|e| format!("lifecycle channel {e:?}"))?;
    let (app_manager_client, app_manager_server) =
        Channel::pair().map_err(|e| format!("app manager channel {e:?}"))?;
    // Independent callers need independent reply queues; duplicated endpoints
    // let updated's background reconciliation consume debugd's responses.
    let (updated_lifecycle_client, updated_lifecycle_server) =
        Channel::pair().map_err(|e| format!("updated lifecycle channel {e:?}"))?;
    let (updated_app_manager_client, updated_app_manager_server) =
        Channel::pair().map_err(|e| format!("updated app manager channel {e:?}"))?;
    // SysUI authentication and debugd account commands run independently.
    // A duplicate of the manager endpoint would share its untagged reply queue.
    let users_client = gate
        .notified_grant(
            "bexos.user.UserManager",
            "UserManager",
            "Public",
            &[1, 2, 3, 4, 5, 6, 7],
            Some(users),
        )
        .map_err(|e| format!("users debug binding {e:?}"))?
        .endpoint;
    let teed_client = Memory::duplicate(teed.0, 1 | 2 | 4 | 32)
        .map_err(|e| format!("teed debug handoff duplicate {e:?}"))?;
    let updated = gate.updated.ok_or("updated not ready")?;
    updated
        .send(
            b"bexos.updated.handoff.v1",
            &[updated_lifecycle_client.0, updated_app_manager_client.0],
        )
        .map_err(|e| format!("updated lifecycle handoff {e:?}"))?;
    let updated_ack = updated
        .recv()
        .map_err(|e| format!("updated lifecycle handoff ack {e:?}"))?;
    if updated_ack.bytes != b"bexos.updated.handoff.ok" {
        return Err("updated lifecycle handoff bad ack".into());
    }
    let updated_client = Memory::duplicate(updated.0, 1 | 2 | 4 | 32)
        .map_err(|e| format!("updated debug handoff duplicate {e:?}"))?;
    debugd
        .send(
            b"bexos.debugd.handoff.v1",
            &[
                lifecycle_client.0,
                app_manager_client.0,
                users_client,
                teed_client,
                updated_client,
            ],
        )
        .map_err(|e| format!("debugd lifecycle handoff {e:?}"))?;
    let ack = debugd
        .recv()
        .map_err(|e| format!("debugd lifecycle handoff ack {e:?}"))?;
    if ack.bytes != b"bexos.debugd.handoff.ok" {
        return Err("debugd lifecycle handoff bad ack".into());
    }
    log("appd: app lifecycle registry ready for debugd\n");
    let (self_client, self_server) =
        Channel::pair().map_err(|e| format!("self migration {e:?}"))?;
    gate.services.push(state::ManagedService {
        package: state::APPD_PACKAGE.into(),
        process: "appd".into(),
        instance_id: String::new(),
        process_handle: 0,
        space_handle: 0,
        thread_handle: 0,
        manager: 0,
        migration: self_client.0,
        hardware: 0,
        generation: 0,
        archive: 0,
        archive_len: 0,
        resource_group_id: 1,
        resource_job: 0,
    });
    let pci_registry = gate.pci_registry;
    let mut runtime = state::AppdState::cold(
        app_registry,
        config,
        config_bytes,
        broker,
        permission_store,
        opener_registry,
        gate.registry,
        vfsd,
        sys_state_root,
        state.active_slot,
        lifecycle_server,
        users,
        self_server,
        gate.services,
    );
    runtime.input_hotplug.registry = pci_registry.map_or(0, |c| c.0);
    runtime.input_hotplug.driver_packages = disk_package_ids;
    runtime.launches = launches;
    runtime.updated_lifecycle = updated_lifecycle_server;
    runtime.opener_bindings = opener_bindings;
    runtime.version_manager_bindings = version_manager_bindings;
    runtime.app_manager_bindings = app_manager_bindings;
    runtime.worker_launcher_bindings = worker_launcher_bindings;
    runtime.service_directory_bindings = service_directory_bindings;
    runtime.lazy = lazy_state;
    runtime.permission_routes = permission_routes;
    runtime.domain_associations = domain_associations;
    runtime.driver_images = driver_images;
    if let Some(package) = boot_sysui {
        runtime.shell.sysui = package;
    }
    runtime.app_manager_bindings.push(crate::AppManagerBinding {
        channel: app_manager_server.0,
        package: state::APPD_PACKAGE.into(),
        uid: SYSTEM_UID,
        system: true,
    });
    runtime.app_manager_bindings.push(crate::AppManagerBinding {
        channel: updated_app_manager_server.0,
        package: state::APPD_PACKAGE.into(),
        uid: SYSTEM_UID,
        system: true,
    });
    #[cfg(feature = "persistent")]
    {
        runtime.stores = persistent_stores;
        restore_service_generation_floors(&mut runtime);
        if boot_graphics.active() {
            log("appd: graphics boot deferring persistent appd bootstrap sync\n");
        } else {
            fs::sync(sys_state_root)
                .map_err(|e| format!("persistent appd bootstrap sync {e:?}"))?;
        }
        drop(persistent_sync_deferral);
    }
    log("appd: guest persistence and disk-only application verified\n");
    boot_graphics.report(4, 90, "System services ready");
    boot_graphics.report(5, 100, "Ready for compositor");
    #[cfg(not(bootui_transplant_validation))]
    boot_graphics.ready();
    #[cfg(bootui_transplant_validation)]
    log("appd: graphical lifecycle test awaiting replacements\n");
    serve_lifecycle(runtime, Some(teed)).await
}

fn validate_secure_bootstrap(
    startup: &Startup,
    config: &PlatformConfig,
    config_bytes: &[u8],
) -> Result<Option<BootEvidenceV1>, String> {
    if !config.tee_policy.enforce_secure_boot {
        return Ok(None);
    }
    let evidence_handle = *startup
        .resources
        .get(1)
        .ok_or("secure boot evidence missing")?;
    let evidence_len = startup.arg1;
    if evidence_len < BootEvidenceV1::BYTES as u64 {
        return Err("secure boot evidence truncated".into());
    }
    let va = Memory::map(evidence_handle, evidence_len, 2)
        .map_err(|e| format!("secure boot evidence map {e:?}"))?;
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, evidence_len as usize) };
    let evidence = BootEvidenceV1::decode(bytes).ok_or("secure boot evidence decode")?;
    if !matches!(
        evidence.version,
        BOOT_EVIDENCE_VERIFIED_BL33_VERSION | bexos_boot::BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION
    ) {
        let mut signed = [0; BootEvidenceV1::SIGNED_BYTES];
        evidence.encode_unsigned(&mut signed);
        verify_ed25519(&evidence.public_key, &signed, &evidence.signature)
            .map_err(|_| "secure boot evidence signature".to_string())?;
    }
    let actual_policy: [u8; 32] = Sha256::digest(config_bytes).into();
    if actual_policy != evidence.policy_sha256 {
        return Err("secure boot policy measurement mismatch".into());
    }
    if evidence.flags & BOOT_EVIDENCE_FLAG_SECURE_BOOT == 0 {
        return Err("secure boot evidence missing secure flag".into());
    }
    if config.metadata.architecture == crate::platform_config::Architecture::X86_64
        && evidence.flags & BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR == 0
    {
        return Err("secure boot evidence missing x86 SVM monitor flag".into());
    }
    if config.tee_policy.require_secure_persistent_keys
        && matches!(
            config.tee_policy.secure_storage_backend,
            crate::platform_config::SecureStorageBackend::Unspecified
        )
    {
        return Err("secure persistent key storage backend missing".into());
    }
    if config.driver_policy.tier_2_rules.enforce_strict_iommu
        && evidence.flags & BOOT_EVIDENCE_FLAG_IOMMU_STRICT == 0
    {
        return Err("secure boot evidence missing strict IOMMU flag".into());
    }
    if !crate::firmware_policy::accepts(config, &evidence) {
        return Err("secure orchestrator measurement mismatch".into());
    }
    Memory::unmap(va, evidence_len).map_err(|e| format!("secure boot evidence unmap {e:?}"))?;
    log(&format!(
        "appd: secure bootstrap evidence generation={} verified\n",
        evidence.generation
    ));
    Ok(Some(evidence))
}

fn initialize_keymint_boot(teed: Channel, evidence: &BootEvidenceV1) -> Result<(), String> {
    let binding = tee_binding::TeeManagerBinding::new(teed)?;
    let teed = binding.0;
    let (bytes, handles) = call_teed_bound(
        teed,
        5,
        &TeeManagerOpenSessionRequest { uuid: KEYMINT_UUID },
        &[],
    )
    .map_err(|status| format!("KeyMint boot session bind {status:?}"))?;
    let opened = TeeManagerOpenSessionResponse::decode(&bytes, &handles)
        .map_err(|_| "KeyMint boot session decode".to_string())?;
    if opened.status != TeeStatus::Ok {
        return Err(format!("KeyMint boot session open {:?}", opened.status));
    }
    let session_id = opened.session_id;
    let mut verified_set = [0u8; BootEvidenceV1::SIGNED_BYTES];
    evidence.encode_unsigned(&mut verified_set);
    let verified_boot_hash: [u8; 32] = Sha256::digest(verified_set).into();
    let boot_payload =
        encode_keymint_set_boot_info(&evidence.public_key, true, 0, &verified_boot_hash, 20260904)
            .map_err(|_| "KeyMint boot-info encode".to_string())?;
    let result: Result<(), String> = (|| {
        let response = invoke_trusty_boot(
            teed,
            session_id,
            "KeyMint",
            KEYMINT_CMD_SET_BOOT_INFO,
            &boot_payload,
        )?;
        decode_keymint_set_boot_info(&response)
            .map_err(|_| "KeyMint boot-info malformed response".to_string())?;

        let hal_payload = encode_keymint_set_hal_info(1, 202609, 20260904)
            .map_err(|_| "KeyMint HAL-info encode".to_string())?;
        let response = invoke_trusty_boot(
            teed,
            session_id,
            "KeyMint",
            KEYMINT_CMD_SET_HAL_INFO,
            &hal_payload,
        )?;
        decode_keymint_set_hal_info(&response)
            .map_err(|_| "KeyMint HAL-info malformed response".to_string())?;
        // Gatekeeper obtains its per-boot HAT signing key from KeyMint. The
        // standard QEMU platform has one SharedSecret participant (Trusty).
        use bexos_trusty_client::keymint_shared_secret as sharing;
        let request = sharing::get_parameters()
            .map_err(|_| "KeyMint shared-secret parameters encode".to_string())?;
        let response = invoke_trusty_boot(
            teed,
            session_id,
            "KeyMint",
            sharing::GET_PARAMETERS,
            &request,
        )?;
        let request = sharing::compute_for_single_instance(&response)
            .map_err(|error| format!("KeyMint shared-secret parameters: {error:?}"))?;
        let response = invoke_trusty_boot(
            teed,
            session_id,
            "KeyMint",
            sharing::COMPUTE_SHARED_SECRET,
            &request,
        )?;
        sharing::decode_check(&response)
            .map_err(|error| format!("KeyMint shared-secret negotiation: {error:?}"))?;
        Ok(())
    })();
    if let Ok((bytes, handles)) =
        call_teed_bound(teed, 6, &TeeManagerCloseSessionRequest { session_id }, &[])
    {
        let _ = TeeManagerCloseSessionResponse::decode(&bytes, &handles);
    }
    result?;
    log("appd: KeyMint verified boot, HAL information, and authentication-token key installed\n");
    Ok(())
}

fn validate_avb_rollback(teed: Channel, generation: u64) -> Result<(), String> {
    const ROLLBACK_SLOT: u32 = 0;
    let binding = tee_binding::TeeManagerBinding::new(teed)?;
    let teed = binding.0;

    let (bytes, handles) = call_teed_bound(
        teed,
        5,
        &TeeManagerOpenSessionRequest { uuid: AVB_UUID },
        &[],
    )
    .map_err(|status| format!("AVB rollback session bind {status:?}"))?;
    let opened = TeeManagerOpenSessionResponse::decode(&bytes, &handles)
        .map_err(|_| "AVB rollback session decode".to_string())?;
    if opened.status != TeeStatus::Ok {
        return Err(format!("AVB rollback session open {:?}", opened.status));
    }
    let session_id = opened.session_id;
    let result = (|| {
        let response = invoke_trusty_boot(
            teed,
            session_id,
            "AVB",
            AVB_CMD_GET_VERSION,
            &encode_avb_get_version(),
        )?;
        let version = decode_avb_version(&response)
            .map_err(|_| "AVB version malformed response".to_string())?;
        log(&format!("appd: AVB service version {version} reachable\n"));

        let read = encode_avb_rollback_index(AVB_CMD_READ_ROLLBACK_INDEX, ROLLBACK_SLOT, 0)
            .map_err(|_| "AVB rollback read encode".to_string())?;
        let response =
            invoke_trusty_boot(teed, session_id, "AVB", AVB_CMD_READ_ROLLBACK_INDEX, &read)?;
        let stored = decode_avb_u64(&response, AVB_CMD_READ_ROLLBACK_INDEX)
            .map_err(|_| "AVB rollback read malformed response".to_string())?;
        if generation < stored {
            return Err(format!(
                "stale rollback index: image {generation}, stored {stored}"
            ));
        }

        let response = invoke_trusty_boot(
            teed,
            session_id,
            "AVB",
            AVB_CMD_READ_LOCK_STATE,
            &encode_avb_read_lock_state(),
        )?;
        let locked = decode_avb_lock_state(&response)
            .map_err(|_| "AVB lock-state malformed response".to_string())?;
        if !locked {
            if stored != 0 {
                return Err("AVB device is unlocked with an existing rollback index".into());
            }
            let response = invoke_trusty_boot(
                teed,
                session_id,
                "AVB",
                AVB_CMD_WRITE_LOCK_STATE,
                &encode_avb_write_lock_state(true),
            )?;
            decode_avb_empty(&response, AVB_CMD_WRITE_LOCK_STATE)
                .map_err(|_| "AVB lock-state write malformed response".to_string())?;
        }
        if generation > stored {
            let write =
                encode_avb_rollback_index(AVB_CMD_WRITE_ROLLBACK_INDEX, ROLLBACK_SLOT, generation)
                    .map_err(|_| "AVB rollback write encode".to_string())?;
            let response = invoke_trusty_boot(
                teed,
                session_id,
                "AVB",
                AVB_CMD_WRITE_ROLLBACK_INDEX,
                &write,
            )?;
            let written = decode_avb_u64(&response, AVB_CMD_WRITE_ROLLBACK_INDEX)
                .map_err(|_| "AVB rollback write malformed response".to_string())?;
            if written != generation {
                return Err("AVB rollback write returned a different index".into());
            }
        }
        let response = invoke_trusty_boot(
            teed,
            session_id,
            "AVB",
            AVB_CMD_LOCK_BOOT_STATE,
            &encode_avb_lock_boot_state(),
        )?;
        decode_avb_empty(&response, AVB_CMD_LOCK_BOOT_STATE)
            .map_err(|_| "AVB boot-state lock malformed response".to_string())
    })();
    if let Ok((bytes, handles)) =
        call_teed_bound(teed, 6, &TeeManagerCloseSessionRequest { session_id }, &[])
    {
        let _ = TeeManagerCloseSessionResponse::decode(&bytes, &handles);
    }
    result?;
    log("appd: RPMB anti-rollback backend verified\n");
    Ok(())
}

fn invoke_trusty_boot(
    teed: Channel,
    session_id: u64,
    service: &str,
    command_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    const MAX_RESPONSE_LEN: u64 = 64 * 1024;

    let payload_vmo = Memory::from_bytes(payload)
        .map_err(|error| format!("{service} command {command_id} payload VMO {error:?}"))?;
    let call = call_teed_bound(
        teed,
        7,
        &TeeManagerInvokeCommandRequest {
            session_id,
            command_id,
            payload: TeeHandleRef { raw: payload_vmo },
            payload_len: payload.len() as u64,
        },
        &[payload_vmo],
    );
    let (bytes, handles) =
        call.map_err(|status| format!("{service} command {command_id} invoke {status:?}"))?;
    let response = TeeManagerInvokeCommandResponse::decode(&bytes, &handles)
        .map_err(|_| format!("{service} command {command_id} response decode"))?;
    if response.status != TeeStatus::Ok {
        let _ = Memory::close(response.response.raw);
        return Err(format!(
            "{service} command {command_id} rejected {:?}",
            response.status
        ));
    }
    if response.response_len > MAX_RESPONSE_LEN {
        let _ = Memory::close(response.response.raw);
        return Err(format!(
            "{service} command {command_id} response too large: {}",
            response.response_len
        ));
    }
    if response.response_len == 0 {
        Memory::close(response.response.raw)
            .map_err(|error| format!("KeyMint command {command_id} response close {error:?}"))?;
        return Ok(Vec::new());
    }
    let rounded = response
        .response_len
        .checked_add(4095)
        .map(|length| length & !4095)
        .ok_or_else(|| format!("{service} command {command_id} response length overflow"))?;
    let va = match Memory::map(response.response.raw, rounded, 2) {
        Ok(va) => va,
        Err(error) => {
            let _ = Memory::close(response.response.raw);
            return Err(format!(
                "{service} command {command_id} response map {error:?}"
            ));
        }
    };
    let response_bytes =
        unsafe { core::slice::from_raw_parts(va as *const u8, response.response_len as usize) }
            .to_vec();
    let unmap = Memory::unmap(va, rounded);
    let close = Memory::close(response.response.raw);
    unmap.map_err(|error| format!("{service} command {command_id} response unmap {error:?}"))?;
    close.map_err(|error| format!("{service} command {command_id} response close {error:?}"))?;
    Ok(response_bytes)
}

#[cfg(feature = "persistent")]
fn restore_service_generation_floors(state: &mut state::AppdState) {
    #[cfg(feature = "persistent")]
    if let Some(stores) = &state.stores {
        state.kernel_generation_floor = stores.generation_floors.get("kernel").unwrap_or(0);
        state.tee_generation_floor = stores.generation_floors.get("tee").unwrap_or(0);
    }
    for service in &mut state.services {
        if let Ok(record) = state.registry.record(&service.package) {
            service.generation = service.generation.max(record.accepted_generation);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BootPackageProvenance {
    signer: String,
    role: String,
}

fn boot_package_provenance(
    boot: &Bootfs<'_>,
) -> Result<BTreeMap<String, BootPackageProvenance>, String> {
    let Some(entry) = boot
        .find("/boot/manifest/product.assembly")
        .map_err(|_| "assembly index")?
    else {
        return Ok(BTreeMap::new());
    };
    let text = core::str::from_utf8(entry.bytes).map_err(|_| "assembly index encoding")?;
    let mut result = BTreeMap::new();
    for line in text.lines().filter(|line| line.starts_with("package: ")) {
        let mut fields = line.split_ascii_whitespace();
        let _ = fields.next();
        let Some(package_id) = fields.next() else {
            continue;
        };
        let mut signer = None;
        let mut role = None;
        for field in fields {
            signer = signer.or_else(|| field.strip_prefix("signer=").map(ToString::to_string));
            role = role.or_else(|| field.strip_prefix("type=").map(ToString::to_string));
        }
        if let (Some(signer), Some(role)) = (signer, role) {
            if !signer.is_empty() {
                if result
                    .insert(
                        package_id.to_string(),
                        BootPackageProvenance { signer, role },
                    )
                    .is_some()
                {
                    return Err(format!("duplicate assembly provenance for {package_id}"));
                }
            }
        }
    }
    Ok(result)
}

fn import_preinstalled_package_manifests(
    vfsd: Channel,
    registry: &mut MemoryAppRegistry,
    permissions: &mut MemoryPermissionStore,
    app_roots_redb: &[u8],
) -> Result<Vec<String>, String> {
    use bexos_redb::{RedbStorageBackend, mem::MemBlockStore};
    use bexos_trust_store::persistent::TrustStoreDb;

    let roots = TrustStoreDb::open_with_backend(RedbStorageBackend::new(
        MemBlockStore::from_bytes(app_roots_redb.to_vec()),
    ))
    .and_then(|database| database.list_app_roots())
    .map_err(|error| format!("load product app signing roots {error:?}"))?;
    let package_ids =
        vfs::list_package_archives(vfsd).map_err(|e| format!("list package archives {e:?}"))?;
    let mut installed_keys = alloc::collections::BTreeSet::new();
    for package_id in &package_ids {
        // Generation archives are selected by the durable registry, never by
        // directory enumeration. In particular, do not import rejected or
        // interrupted replacement candidates as preinstalled applications.
        if migration_archive::is_staged(package_id) {
            continue;
        }
        let archive_bytes = vfs::read_package_archive(vfsd, package_id)
            .map_err(|e| format!("package archive {package_id} {e:?}"))?;
        let envelope = bexos_app_archive::OpenArchive::parse(&archive_bytes)
            .map_err(|error| format!("parse archive signature {package_id} {error:?}"))?;
        let public_key = envelope
            .signer()
            .certificate_chain
            .first()
            .ok_or_else(|| format!("archive {package_id} has no signer certificate"))?;
        let anchor =
            bexos_trust_store::validate_direct_app_signature(&roots, package_id, 0, public_key)
                .map_err(|error| format!("authorize archive signer {package_id} {error:?}"))?;
        let public_key: &[u8; 32] = anchor
            .public_key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| format!("signing root {} is not Ed25519", anchor.anchor_id))?;
        let trusted = [bexos_app_archive::TrustedKey {
            key_id: envelope.key_id(),
            public_key,
        }];
        let verified_signer = bexos_app_registry::VerifiedSignerMetadata {
            root_anchor_id: anchor.anchor_id.clone(),
            leaf_certificate_fingerprint: *blake3::hash(public_key).as_bytes(),
            signature_algorithm: 1,
            granted_trust_tier: anchor.tier as u8,
        };
        let archive_path = format!("pkg/{package_id}.bex");
        let record = registry
            .install_bundle(bexos_app_registry::InstallRequest {
                archive_bytes: &archive_bytes,
                trusted_keys: &trusted,
                verified_signer: Some(verified_signer),
                source: InstallSource::SystemImage,
                protected: true,
                archive_path: &archive_path,
            })
            .map_err(|e| format!("registry import {package_id} {e:?}"))?;
        installed_keys.insert(record.package_key());
        let manifest = Manifest::decode(&record.manifest_bytes)
            .map_err(|e| format!("disk manifest decode {package_id} {e:?}"))?;
        manifest
            .validate_package_shape()
            .map_err(|e| format!("disk manifest validation {package_id} {e:?}"))?;
        let _ = permissions.register_system_declarations(
            &manifest.package_name,
            &manifest.permissions,
            0,
        );
        log(&format!(
            "appd: boot disk package imported package={}\n",
            manifest.package_name
        ));
    }
    Ok(installed_keys.into_iter().collect())
}

fn replay_trusted_app_packages(
    vfsd: Channel,
    teed: Channel,
    registry: &MemoryAppRegistry,
) -> Result<(), lifecycle::AppLifecycleStatus> {
    for record in registry.list_packages() {
        if let Some(diagnostic) = record.architecture_diagnostic() {
            log(&alloc::format!(
                "appd: package {} unavailable: {}\n",
                record.package_key(),
                diagnostic
            ));
            continue;
        }
        let manifest = Manifest::decode(&record.manifest_bytes)
            .map_err(|_| lifecycle::AppLifecycleStatus::VerifyFailed)?;
        if manifest.package_kind != crate::PackageKind::TrustedApp {
            continue;
        }
        let archive = vfs::read_package_archive(vfsd, &record.archive_id())
            .map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
        let payload = trusted_app_payload(&manifest, &archive)
            .map_err(|_| lifecycle::AppLifecycleStatus::VerifyFailed)?;
        activate_trusted_app_package(Some(teed), &manifest, &record.package_key(), &payload)?;
    }
    Ok(())
}

fn reconcile_missing_archives(
    vfsd: Channel,
    registry: &mut MemoryAppRegistry,
) -> Result<(), fs_fidl::FsStatus> {
    let records = registry.list_packages().to_vec();
    for record in records {
        if record.validate_architecture().is_err() {
            continue;
        }
        if record.install_source == InstallSource::Bootfs {
            continue;
        }
        match vfs::get_package_archive_attributes(vfsd, &record.archive_id()) {
            Ok(_) => {}
            Err(fs_fidl::FsStatus::NotFound) => {
                if record.protected
                    || registry
                        .active_pin(&record.package_id)
                        .is_some_and(|pin| pin.pinned_version == record.version)
                {
                    return Err(fs_fidl::FsStatus::NotFound);
                }
                registry
                    .remove_version(&record.package_id, &record.version)
                    .map_err(|_| fs_fidl::FsStatus::Io)?;
            }
            Err(status) => return Err(status),
        }
    }
    Ok(())
}

fn bind_preinstalled_drivers(
    vfsd: Channel,
    package_ids: &[String],
    registry: &MemoryAppRegistry,
    config: &PlatformConfig,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    broker: &mut AppdBroker,
    gate: &mut readiness::Gate,
    orchestrator: &AppdWaveOrchestrator,
) -> Result<(), String> {
    for package_id in package_ids {
        let record = registry
            .record(package_id)
            .map_err(|e| format!("driver package registry {package_id} {e:?}"))?;
        let manifest = Manifest::decode(&record.manifest_bytes)
            .map_err(|e| format!("driver manifest decode {package_id} {e:?}"))?;
        if manifest.bind_rules.is_empty() {
            continue;
        }
        let archive_root = vfs::get_package_directory(vfsd, &record.archive_id())
            .map_err(|e| format!("driver package directory {package_id} {e:?}"))?;
        let dependency_roots = match resolve_library_dependencies(vfsd, registry, &manifest) {
            Ok(roots) => roots,
            Err(error) => {
                let _ = Memory::close(archive_root.0);
                return Err(format!("driver dependencies {package_id} {error:?}"));
            }
        };
        let disk_resolver = match resolver::Resolver::disk_with_dependencies(
            archive_root,
            &manifest,
            &dependency_roots,
            vfsd,
            registry,
            None,
        ) {
            Ok(resolver) => resolver,
            Err(error) => {
                close_dependency_roots(dependency_roots);
                let _ = Memory::close(archive_root.0);
                return Err(format!("driver image resolution {package_id} {error:?}"));
            }
        };
        let manifests = [manifest];
        let first_new_service = gate.services.len();
        let result = (|| {
            orchestrator
                .launch_automatic_with_policy(
                    &manifests,
                    &config.runner_policy,
                    &config.driver_policy,
                    |m| PackageIdentity {
                        package_id: &m.package_name,
                        signer: if record.install_source == InstallSource::Oci {
                            record
                                .verified_signer
                                .as_ref()
                                .map_or("", |signer| signer.root_anchor_id.as_str())
                        } else {
                            "bexos_official_platform_v1"
                        },
                        trust_tier: PackageTrustTier::SystemHardware,
                        is_driver: true,
                    },
                    4,
                    kernel,
                    &disk_resolver,
                    gate,
                )
                .map_err(|e| format!("driver disk bind {package_id} {e:?}"))?;
            // Installed archives can also back bootstrap drivers. Only publish
            // instances launched by this bind pass; existing managers are already
            // registered and remain live during hotplug.
            publish_running_services(
                broker,
                &manifests,
                &gate.services[first_new_service..],
                &gate.registry,
                config,
            )
            .map_err(|e| format!("driver service publish {package_id} {e:?}"))?;
            Ok::<(), String>(())
        })();
        drop(disk_resolver);
        close_dependency_roots(dependency_roots);
        let closed = Memory::close(archive_root.0)
            .map_err(|e| format!("driver package close {package_id} {e:?}"));
        result?;
        closed?;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreinstalledServiceProcess {
    pub package_id: String,
    pub process_name: String,
    pub wave: u32,
}

fn launch_preinstalled_storage_services(
    vfsd: Channel,
    users: Channel,
    package_ids: &[String],
    registry: &mut MemoryAppRegistry,
    launches: &mut Vec<state::LaunchRecord>,
    services: &mut Vec<state::ManagedService>,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    broker: &mut AppdBroker,
    permission_routes: &mut crate::PermissionRouteTable,
    permissions: &mut MemoryPermissionStore,
    opener_bindings: &mut Vec<OpenerBinding>,
    version_manager_bindings: &mut Vec<u64>,
    app_manager_bindings: &mut Vec<crate::AppManagerBinding>,
    worker_launcher_bindings: &mut Vec<crate::AppManagerBinding>,
    service_directory_bindings: &mut Vec<crate::ServiceDirectoryBinding>,
    lazy: &mut crate::lazy::LazyActivationState,
    domain_associations: &bexos_domain_association::MemoryDomainAssociationCache,
    config: &PlatformConfig,
    component_configs: &[state::ComponentConfigRecord],
    boot_sysui: &mut Option<String>,
) -> Result<(), String> {
    let processes = preinstalled_storage_service_processes(registry, package_ids)?;
    for process in processes {
        let configured_instances = config
            .network_policy
            .isolation_groups
            .iter()
            .filter(|group| {
                group.networkd_package == process.package_id
                    && group.networkd_process == process.process_name
                    || group.netstackd_package == process.package_id
                        && group.netstackd_process == process.process_name
            })
            .map(|group| group.name.clone())
            .collect::<Vec<_>>();
        let instances = if configured_instances.is_empty() {
            vec![None]
        } else {
            configured_instances
                .iter()
                .map(|name| Some(name.as_str()))
                .collect()
        };
        for instance_id in instances {
            let status = launch_application(
                registry,
                launches,
                services,
                vfsd,
                users,
                kernel,
                broker,
                permission_routes,
                permissions,
                opener_bindings,
                version_manager_bindings,
                app_manager_bindings,
                worker_launcher_bindings,
                service_directory_bindings,
                lazy,
                domain_associations,
                config,
                component_configs,
                &process.package_id,
                &process.process_name,
                0,
                SYSTEM_UID,
                None,
                false,
                instance_id,
                None,
                Vec::new(),
                0,
                false,
                None,
            );
            if status != lifecycle::AppLifecycleStatus::Ok {
                return Err(format!(
                    "storage service launch package={} process={} instance={} {status:?}",
                    process.package_id,
                    process.process_name,
                    instance_id.unwrap_or("singleton"),
                ));
            }
            log(&format!(
                "appd: launched preinstalled service package={} process={} instance={} wave={}\n",
                process.package_id,
                process.process_name,
                instance_id.unwrap_or("singleton"),
                process.wave,
            ));
        }
        if process.package_id == preferences::PACKAGE && boot_sysui.is_none() {
            match preferences::resolve_shell_selection(
                registry,
                services,
                vfsd,
                component_configs,
                SYSTEM_UID,
                "sysui_package",
            ) {
                Ok(package) => {
                    log(&format!(
                        "appd: captured boot system shell selection package={package}\n"
                    ));
                    *boot_sysui = Some(package);
                }
                Err(error) => log(&format!(
                    "appd: early system shell selection deferred error={error:?}\n"
                )),
            }
        }
    }
    Ok(())
}

fn preinstalled_storage_service_processes(
    registry: &MemoryAppRegistry,
    package_ids: &[String],
) -> Result<Vec<PreinstalledServiceProcess>, String> {
    let mut manifests = Vec::new();
    for package_id in package_ids {
        let manifest_bytes = registry
            .load_manifest(package_id)
            .map_err(|e| format!("storage service registry {package_id} {e:?}"))?;
        let manifest = Manifest::decode(&manifest_bytes)
            .map_err(|e| format!("storage service manifest decode {package_id} {e:?}"))?;
        manifests.push(manifest);
    }
    Ok(preinstalled_storage_service_plan(&manifests))
}

pub fn preinstalled_storage_service_plan(
    manifests: &[Manifest],
) -> Vec<PreinstalledServiceProcess> {
    let mut processes = Vec::new();
    for manifest in manifests {
        if !manifest.bind_rules.is_empty() {
            continue;
        }
        for process in &manifest.processes {
            if process.shell_role != crate::manifest::ShellRole::None {
                continue;
            }
            if manifest.process_has_lazy_exposures(&process.name) {
                continue;
            }
            if process.service {
                if let Some(wave) = process.wave {
                    processes.push(PreinstalledServiceProcess {
                        package_id: manifest.package_name.clone(),
                        process_name: process.name.clone(),
                        wave,
                    });
                }
            }
        }
    }
    processes.sort_by(|a, b| {
        a.wave
            .cmp(&b.wave)
            .then_with(|| a.package_id.cmp(&b.package_id))
            .then_with(|| a.process_name.cmp(&b.process_name))
    });
    processes
}

async fn serve_lifecycle(mut state: state::AppdState, teed: Option<Channel>) -> ! {
    ensure_user_state_watcher(&mut state);
    let _ = publish_appd_opener_service(&mut state.broker);
    let _ = publish_appd_version_manager_service(&mut state.broker);
    let _ = publish_appd_manager_service(&mut state.broker);
    let _ = publish_appd_device_registry_service(&mut state.broker);
    let _ = publish_appd_service_directory_service(&mut state.broker);
    let mut source = Source::new(Some(state.migration));
    let mut kernel = KernelFidlOps::new(KernelTransport(1), KernelTransport(2), KernelTransport(4));
    #[cfg(bootui_transplant_validation)]
    let mut test_graphics_released = false;
    let mut pending: Option<update::Update> = None;
    let mut last_update = if state.generation != 0 {
        "migration completed".into()
    } else {
        String::new()
    };
    log("appd: lifecycle dispatch ready\n");
    let mut activation_sync_pending = false;
    let mut activation_completion_reported = false;
    let mut activation_sync_after = 0u64;
    let activation_sync_delay = bexos_userspace::syscall::frequency().saturating_mul(30);
    loop {
        if let Err(error) = source.poll(&state) {
            let _ = bexos_userspace::migration::abort();
            last_update = format!("migration source rejected: {error:?}");
            log(&alloc::format!("appd: {last_update}\n"));
        }
        if let Some(task) = pending.as_mut() {
            match task.poll(&mut state, &mut source, &mut kernel) {
                Ok(true) => {
                    last_update = task.completion_message();
                    source.changed(0);
                    source.changed_keys(
                        (0..state.services.len()).map(|i| state::SERVICE_BASE + i as u64),
                    );
                    source.changed_keys(
                        (0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64),
                    );
                    log(&alloc::format!("appd: {last_update}\n"));
                    match task.continue_package(&state, &mut kernel) {
                        Ok(Some(next)) => {
                            state.pending_archive = next.archive();
                            state.pending_replacement = next.replacement_descriptors();
                            state.pending_record = Some(next.candidate_record.clone());
                            pending = Some(next);
                            last_update = "migration continuing with next service instance".into();
                            source.changed(4);
                        }
                        Ok(None) => {
                            pending = None;
                            activation_sync_pending = true;
                            activation_completion_reported = false;
                            activation_sync_after = bexos_userspace::syscall::ticks()
                                .saturating_add(activation_sync_delay);
                            log("appd: migration coordinator resources released\n");
                        }
                        Err(error) => {
                            state.pending_record = None;
                            state.pending_archive = (0, 0);
                            state.pending_replacement = (0, 0, 0);
                            last_update = error;
                            pending = None;
                            source.changed(4);
                        }
                    }
                }
                Ok(false) => {
                    last_update = task.phase().into();
                }
                Err(error) => {
                    log(&alloc::format!("appd: migration task failed: {error}\n"));
                    task.abort();
                    state.pending_record = None;
                    state.pending_archive = (0, 0);
                    state.pending_replacement = (0, 0, 0);
                    source.changed(4);
                    last_update = error;
                    pending = None;
                }
            }
        }
        if source.quiescing() {
            #[cfg(feature = "persistent")]
            let _ = state.sync_stores();
            async_yield().await;
            continue;
        }
        #[cfg(bootui_transplant_validation)]
        if !test_graphics_released
            && pending.is_none()
            && ["bexos.driver.display.virtio_gpu", "bexos.service.scened"]
                .iter()
                .all(|p| {
                    state
                        .services
                        .iter()
                        .any(|s| s.package == *p && s.generation != 0)
                })
        {
            if let Some(s) = state
                .services
                .iter()
                .find(|s| s.package == "bexos.service.scened")
            {
                let _ = Channel(s.manager).send(b"bexos.graphics.ready", &[]);
                test_graphics_released = true;
            }
        }
        let freeze_preferences = pending
            .as_ref()
            .is_some_and(|task| task.freezes_preferences());
        if freeze_preferences {
            if state.preferences_frozen != Some(true) && preferences::freeze(&state, true).is_ok() {
                state.preferences_frozen = Some(true);
            }
        } else if state.preferences_frozen != Some(false) {
            // This cache is reconstructed after appd's own transplant, while
            // prefsd retains the freeze used to stage it. Reconcile an unknown
            // state with the provider instead of assuming it is already thawed.
            log("appd: thawing preferences for lifecycle dispatch\n");
            if preferences::freeze(&state, false).is_ok() {
                state.preferences_frozen = Some(false);
            }
            log("appd: preference thaw returned\n");
        }
        if pending.is_none() && !activation_sync_pending {
            input_hotplug::poll(&mut state, &mut kernel, &mut source);
            if crate::package_install::acquire_hardware_driver(&mut state) {
                source.changed(crate::package_install::KEY);
            }
            poll_completed_boot_service(&mut state, &mut source);
            poll_lazy_provider_control(&mut state);
            poll_lifecycle_watchdog(&mut state, &mut kernel, &mut source);
            let shell_before_users = state.shell.encode();
            poll_user_state_watcher(&mut state);
            if shell_before_users != state.shell.encode() {
                source.changed(shell::KEY);
            }
            poll_permission_route_clients(&mut state);
            shell::poll(&mut state, &mut kernel, &mut source);
            preferences::poll_registration(&mut state);
        }
        shell::poll_requests(
            &mut state,
            &mut kernel,
            &mut source,
            pending.is_none() && !activation_sync_pending,
        );
        for channel in [state.lifecycle, state.updated_lifecycle]
            .into_iter()
            .filter(|c| c.0 != 0)
        {
            if let Ok(m) = channel.try_recv() {
                if activation_sync_pending {
                    activation_sync_after =
                        bexos_userspace::syscall::ticks().saturating_add(activation_sync_delay);
                }
                let (ordinal, req) = envelope(&m.bytes);
                log(&format!("appd: lifecycle request ordinal={ordinal}\n"));
                let hs = lifecycle_refs(&m.handles);
                let preferences_ready = if matches!(ordinal, 2 | 3) {
                    let ready = preferences::freeze(&state, true).is_ok();
                    if ready {
                        state.preferences_frozen = Some(true);
                    }
                    ready
                } else {
                    true
                };
                let old_count = state.registry.list_packages().len();
                match ordinal {
                    17 => {
                        let status = match lifecycle::AppLifecycleControlTerminateSelectedShellForDebugRequest::decode(req, &hs) {
                            Err(_) => lifecycle::AppLifecycleStatus::InvalidArgs,
                            Ok(_) if channel.0 != state.lifecycle.0 || pending.is_some() => {
                                lifecycle::AppLifecycleStatus::AccessDenied
                            }
                            Ok(q) if !state.shell.authorized(q.package_id, q.uid) => {
                                lifecycle::AppLifecycleStatus::AccessDenied
                            }
                            Ok(q) => match state.launches.iter().position(|launch| {
                                launch.package == q.package_id && launch.uid == q.uid
                            }) {
                                None => lifecycle::AppLifecycleStatus::NotFound,
                                Some(index) => {
                                    let process = state.launches[index].process_handle;
                                    if crate::runner::KernelOps::terminate_process(
                                        &mut kernel,
                                        crate::runner::KernelHandle { raw: process },
                                        -9,
                                    )
                                    .is_err()
                                    {
                                        lifecycle::AppLifecycleStatus::LaunchFailed
                                    } else {
                                        // The debug operation owns this explicit crash
                                        // injection, so retire the launch immediately.
                                        // Waiting for a later task-signal poll can leave a
                                        // terminated migrated handle recorded as running.
                                        let launch = state.launches.remove(index);
                                        cleanup_dead_launch(&mut state, &launch);
                                        source.changed(shell::KEY);
                                        lifecycle::AppLifecycleStatus::Ok
                                    }
                                }
                            },
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlTerminateSelectedShellForDebugResponse {
                                status,
                            },
                        );
                    }
                    16 => {
                        let response = if channel.0 != state.lifecycle.0 {
                            bexos_debug_wire::PreferencesResponse {
                                status: -2,
                                ..Default::default()
                            }
                        } else if pending.is_some() {
                            bexos_debug_wire::PreferencesResponse {
                                status: -11,
                                ..Default::default()
                            }
                        } else {
                            match lifecycle::AppLifecycleControlPreferencesRequest::decode(req, &hs)
                            {
                                Ok(q) => preferences::debug_request(&state, q.request),
                                Err(_) => bexos_debug_wire::PreferencesResponse {
                                    status: -8,
                                    ..Default::default()
                                },
                            }
                        };
                        let mut bytes = Vec::new();
                        bexos_debug_wire::encode_preferences_response(&response, &mut bytes);
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlPreferencesResponse { response: &bytes },
                        );
                    }
                    15 => {
                        let status =
                            match lifecycle::AppLifecycleControlBindDebugOpenerRequest::decode(
                                req, &hs,
                            ) {
                                Ok(q)
                                    if channel.0 == state.lifecycle.0
                                        && pending.is_none()
                                        && (q.uid == SYSTEM_UID
                                            || user_is_unlocked(state.users, q.uid)) =>
                                {
                                    let scope = if q.uid == SYSTEM_UID {
                                        OpenerScope::System
                                    } else {
                                        OpenerScope::User(q.uid)
                                    };
                                    if matches!(
                                        state.openers.resolve(
                                            scope,
                                            OpenKind::Interface("bexos.shell.ShellProvider"),
                                        ),
                                        ResolveOutcome::NoHandler
                                    ) {
                                        let _ = Memory::close(q.opener.raw);
                                        lifecycle::AppLifecycleStatus::NotFound
                                    } else {
                                        state.opener_bindings.push(OpenerBinding {
                                            channel: q.opener.raw,
                                            package: "bexos.driver.debugd".into(),
                                            uid: q.uid,
                                            system: q.uid == SYSTEM_UID,
                                        });
                                        source.changed(0);
                                        lifecycle::AppLifecycleStatus::Ok
                                    }
                                }
                                Ok(_) => {
                                    close_handles(&m.handles);
                                    lifecycle::AppLifecycleStatus::AccessDenied
                                }
                                Err(_) => {
                                    close_handles(&m.handles);
                                    lifecycle::AppLifecycleStatus::InvalidArgs
                                }
                            };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlBindDebugOpenerResponse { status },
                        );
                    }
                    7 => {
                        let q = lifecycle::AppLifecycleControlGetProcessProgressRequest::decode(
                            req, &hs,
                        );
                        let launch = q.ok().and_then(|q| {
                            state
                                .launches
                                .iter()
                                .rev()
                                .find(|l| l.package == q.package_id && l.progress != 0)
                        });
                        let h = launch.map(|l| {
                            let _ = Channel(l.manager).send(b"progress.tick", &[]);
                            l.progress
                        });
                        let values = h.and_then(|h| Memory::map(h, 4096, 2).ok()).map(|va| {
                            let completed = unsafe { core::ptr::read_volatile(va as *const u64) };
                            let errors =
                                unsafe { core::ptr::read_volatile((va + 8) as *const u64) };
                            let _ = Memory::unmap(va, 4096);
                            (completed, errors)
                        });
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlGetProcessProgressResponse {
                                status: if values.is_some() {
                                    lifecycle::AppLifecycleStatus::Ok
                                } else {
                                    lifecycle::AppLifecycleStatus::NotFound
                                },
                                completed: values.map_or(0, |v| v.0),
                                errors: values.map_or(0, |v| v.1),
                            },
                        );
                    }
                    5 => {
                        let request =
                            lifecycle::AppLifecycleControlBeginMigrationRequest::decode(req, &hs);
                        let status = match request {
                            Ok(q) if pending.is_none() && preferences_ready => match update::begin(
                                &state,
                                &mut kernel,
                                q.archive.raw,
                                q.archive_len,
                                q.generation,
                                q.target,
                            ) {
                                Ok(task) => {
                                    state.pending_archive = task.archive();
                                    state.pending_replacement = task.replacement_descriptors();
                                    state.pending_record = Some(task.candidate_record.clone());
                                    source.changed(4);
                                    pending = Some(task);
                                    last_update = "migration preparing".into();
                                    lifecycle::AppLifecycleStatus::Ok
                                }
                                Err(error) => {
                                    last_update = error;
                                    lifecycle::AppLifecycleStatus::VerifyFailed
                                }
                            },
                            Ok(q) => {
                                let _ = Memory::close(q.archive.raw);
                                lifecycle::AppLifecycleStatus::InvalidArgs
                            }
                            Err(_) => lifecycle::AppLifecycleStatus::InvalidArgs,
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlBeginMigrationResponse {
                                status,
                                message: &last_update,
                            },
                        );
                    }
                    9 => {
                        log("appd: stored migration request received\n");
                        let request =
                        lifecycle::AppLifecycleControlBeginMigrationFromStoredArchiveRequest::decode(
                            req, &hs,
                        );
                        let status = match request {
                            Ok(q) if pending.is_none() && preferences_ready => {
                                match vfs::read_update_archive(state.vfsd, q.archive_id)
                                    .map_err(|e| alloc::format!("stored update archive {e:?}"))
                                    .and_then(|archive| {
                                        let len = archive.len() as u64;
                                        let handle = Memory::from_bytes(&archive).map_err(|e| {
                                            alloc::format!("stored update VMO {e:?}")
                                        })?;
                                        // The archive is mapped back into update::begin. Release the
                                        // VFS response before verification/decompression so a
                                        // self-update does not retain two userspace copies.
                                        drop(archive);
                                        match update::begin(
                                            &state,
                                            &mut kernel,
                                            handle,
                                            len,
                                            q.generation,
                                            q.target,
                                        ) {
                                            Ok(task) => Ok(task),
                                            Err(error) => {
                                                let _ = Memory::close(handle);
                                                Err(error)
                                            }
                                        }
                                    }) {
                                    Ok(task) => {
                                        state.pending_archive = task.archive();
                                        state.pending_replacement = task.replacement_descriptors();
                                        state.pending_record = Some(task.candidate_record.clone());
                                        source.changed(4);
                                        pending = Some(task);
                                        last_update = "migration preparing".into();
                                        lifecycle::AppLifecycleStatus::Ok
                                    }
                                    Err(error) => {
                                        last_update = error;
                                        lifecycle::AppLifecycleStatus::VerifyFailed
                                    }
                                }
                            }
                            Ok(_) => lifecycle::AppLifecycleStatus::InvalidArgs,
                            Err(_) => lifecycle::AppLifecycleStatus::InvalidArgs,
                        };
                        lifecycle_reply(
                        channel,
                        &lifecycle::AppLifecycleControlBeginMigrationFromStoredArchiveResponse {
                            status,
                            message: &last_update,
                        },
                    );
                    }
                    6 => {
                        let request =
                            lifecycle::AppLifecycleControlGetMigrationStatusRequest::decode(
                                req, &hs,
                            );
                        let service = request.ok().and_then(|q| {
                            state
                                .services
                                .iter()
                                .filter(|s| s.package == q.target)
                                .min_by_key(|s| s.generation)
                        });
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlGetMigrationStatusResponse {
                                status: if service.is_some() {
                                    lifecycle::AppLifecycleStatus::Ok
                                } else {
                                    lifecycle::AppLifecycleStatus::NotFound
                                },
                                generation: service.map_or(0, |s| s.generation),
                                pending: pending.is_some(),
                                message: &last_update,
                            },
                        );
                        if activation_sync_pending && pending.is_none() {
                            activation_completion_reported = true;
                        }
                    }
                    1 => {
                        let _ = lifecycle::AppLifecycleControlListAppsRequest::decode(req, &hs);
                        let records = state.registry.list_packages();
                        let apps = records.iter().map(lifecycle_info).collect::<Vec<_>>();
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlListAppsResponse {
                                status: lifecycle::AppLifecycleStatus::Ok,
                                apps: lifecycle::WireVector::from_slice(&apps),
                            },
                        );
                    }
                    2 => {
                        let request =
                            lifecycle::AppLifecycleControlInstallBundleRequest::decode(req, &hs);
                        let (mut status, package_id) = match request {
                            Ok(request) if pending.is_none() && preferences_ready => {
                                install_lifecycle_bundle(
                                    &mut state.registry,
                                    &mut state.permissions,
                                    &mut state.openers,
                                    state.vfsd,
                                    teed,
                                    request,
                                )
                            }
                            Ok(request) => {
                                let _ = Memory::close(request.archive.raw);
                                (lifecycle::AppLifecycleStatus::LaunchFailed, String::new())
                            }
                            Err(_) => (lifecycle::AppLifecycleStatus::InvalidArgs, String::new()),
                        };
                        if status == lifecycle::AppLifecycleStatus::Ok {
                            let _ = preferences::reconcile(&mut state);
                        }
                        // Complete the durable checkpoint before acknowledging the
                        // install, so the next lifecycle request cannot overtake it.
                        if status == lifecycle::AppLifecycleStatus::Ok
                            && state.sync_stores().is_err()
                        {
                            status = lifecycle::AppLifecycleStatus::Storage;
                        }
                        if status == lifecycle::AppLifecycleStatus::Ok {
                            let _ = publish_dormant_registry_services(
                                &mut state.broker,
                                &state.registry,
                                &mut state.lazy,
                            );
                            let policy_event = state
                                .registry
                                .record(&package_id)
                                .map(|record| {
                                    (record.package_id.clone(), record.package_instance_id)
                                })
                                .ok();
                            if let Some((package_id, package_instance_id)) = policy_event {
                                notify_package_policy_watchers(
                                    &mut state,
                                    &package_id,
                                    package_instance_id,
                                    app_worker::PackagePolicyEventKind::Installed,
                                );
                            }
                        }
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlInstallBundleResponse {
                                status,
                                package_id: &package_id,
                            },
                        );
                    }
                    3 => {
                        let request =
                            lifecycle::AppLifecycleControlUninstallRequest::decode(req, &hs);
                        let mut removed_package = String::new();
                        let status = match request {
                            Ok(request) if pending.is_none() && preferences_ready => {
                                removed_package = request.package_id.to_string();
                                uninstall_lifecycle_app(
                                    &mut state.registry,
                                    &mut state.permissions,
                                    &mut state.openers,
                                    state.vfsd,
                                    teed,
                                    request.package_id,
                                )
                            }
                            Ok(_) => lifecycle::AppLifecycleStatus::LaunchFailed,
                            Err(_) => lifecycle::AppLifecycleStatus::InvalidArgs,
                        };
                        if status == lifecycle::AppLifecycleStatus::Ok {
                            let _ = preferences::reconcile(&mut state);
                            let removed_caller = state
                                .permission_routes
                                .remove_caller_package(&removed_package);
                            close_permission_routes(removed_caller);
                            let removed_provider = state
                                .permission_routes
                                .remove_provider_package(&removed_package);
                            close_permission_routes(removed_provider);
                            state.broker.remove_provider_package(&removed_package);
                            let removed_lazy = state.lazy.remove_package(&removed_package);
                            close_bound_capabilities(&removed_lazy);
                            let _ = crate::permission_persistence::sync_system(
                                state.vfsd,
                                &state.permissions,
                            );
                            notify_package_policy_watchers(
                                &mut state,
                                &removed_package,
                                0,
                                app_worker::PackagePolicyEventKind::Uninstalled,
                            );
                        }
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlUninstallResponse { status },
                        );
                    }
                    4 => {
                        let request = lifecycle::AppLifecycleControlLaunchRequest::decode(req, &hs);
                        let status = match request {
                            Ok(request) if pending.is_none() => launch_lifecycle_app(
                                &mut state.registry,
                                &mut state.launches,
                                &mut state.services,
                                state.vfsd,
                                state.users,
                                &mut kernel,
                                &mut state.broker,
                                &mut state.permission_routes,
                                &mut state.permissions,
                                &mut state.opener_bindings,
                                &mut state.version_manager_bindings,
                                &mut state.app_manager_bindings,
                                &mut state.worker_launcher_bindings,
                                &mut state.service_directory_bindings,
                                &mut state.lazy,
                                &state.domain_associations,
                                &state.config,
                                &state.component_configs,
                                request.package_id,
                                request.process_name,
                                request.arg0,
                                request.uid,
                                None,
                                false,
                            ),
                            Ok(_) => lifecycle::AppLifecycleStatus::LaunchFailed,
                            Err(_) => lifecycle::AppLifecycleStatus::InvalidArgs,
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlLaunchResponse { status },
                        );
                    }
                    8 => {
                        let request =
                            lifecycle::AppLifecycleControlRequestPermissionRequest::decode(
                                req, &hs,
                            );
                        let (status, state_value, granted_values, granted_handle) = match request {
                            Ok(request) if pending.is_none() => request_optional_permission(
                                &state.registry,
                                &state.launches,
                                state.vfsd,
                                state.users,
                                &mut kernel,
                                &state.broker,
                                &mut state.permission_routes,
                                &mut state.permissions,
                                request.package_id,
                                request.process_name,
                                request.uid,
                                request.permission_name,
                                request.service_name,
                                request.capability,
                                request.requested_values,
                            ),
                            Ok(_) => (
                                lifecycle::AppLifecycleStatus::AccessDenied,
                                lifecycle::PermissionState::Denied,
                                Vec::new(),
                                0,
                            ),
                            Err(_) => (
                                lifecycle::AppLifecycleStatus::InvalidArgs,
                                lifecycle::PermissionState::Denied,
                                Vec::new(),
                                0,
                            ),
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlRequestPermissionResponse {
                                status,
                                state: state_value,
                                granted_values: lifecycle::WireStringVector::from_slice(
                                    &granted_values
                                        .iter()
                                        .map(|value| value.as_str())
                                        .collect::<Vec<_>>(),
                                ),
                                granted_handle: lifecycle::HandleRef {
                                    raw: granted_handle,
                                },
                            },
                        );
                    }
                    10 => {
                        let request =
                            lifecycle::AppLifecycleControlGetComponentConfigRequest::decode(
                                req, &hs,
                            );
                        let (status, generation, config) = match request {
                            Ok(request) if channel.0 == state.lifecycle.0 => {
                                match preferences::operator(
                                    &state,
                                    request.package_id,
                                    0,
                                    0,
                                    &[],
                                    &[],
                                ) {
                                    Ok((status, generation, bytes, _)) => (
                                        preferences::lifecycle_status(status),
                                        generation,
                                        Some(bytes),
                                    ),
                                    Err(e) => (
                                        preferences::lifecycle_status(
                                            bexos_userspace::preferences::status(e),
                                        ),
                                        0,
                                        None,
                                    ),
                                }
                            }
                            _ => (lifecycle::ComponentConfigStatus::InvalidArgs, 0, None),
                        };
                        let (status, raw, config_len) = match config {
                            Some(bytes) => match Memory::from_bytes(&bytes) {
                                Ok(raw) => (status, raw, bytes.len() as u64),
                                Err(_) => (lifecycle::ComponentConfigStatus::Storage, 0, 0),
                            },
                            None => (status, 0, 0),
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlGetComponentConfigResponse {
                                status,
                                generation,
                                config: lifecycle::HandleRef { raw },
                                config_len,
                            },
                        );
                    }
                    11 => {
                        let request =
                            lifecycle::AppLifecycleControlSetComponentConfigRequest::decode(
                                req, &hs,
                            );
                        let (status, generation, message) = match request {
                            Ok(request) if pending.is_none() && channel.0 == state.lifecycle.0 => {
                                match read_config_vmo(request.config.raw, request.config_len) {
                                    Ok(bytes) => preferences::mutate(
                                        &state,
                                        request.package_id,
                                        1,
                                        request.expected_generation,
                                        &bytes,
                                        &[],
                                    ),
                                    Err(status) => (
                                        status,
                                        request.expected_generation,
                                        "invalid config".into(),
                                    ),
                                }
                            }
                            Ok(request) => {
                                let _ = Memory::close(request.config.raw);
                                (
                                    lifecycle::ComponentConfigStatus::Busy,
                                    0,
                                    "migration pending".to_string(),
                                )
                            }
                            Err(_) => (
                                lifecycle::ComponentConfigStatus::InvalidArgs,
                                0,
                                "invalid request".to_string(),
                            ),
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlSetComponentConfigResponse {
                                status,
                                generation,
                                message: &message,
                            },
                        );
                    }
                    12 => {
                        let request =
                            lifecycle::AppLifecycleControlResetComponentConfigRequest::decode(
                                req, &hs,
                            );
                        let (status, generation, message) = match request {
                            Ok(request) if pending.is_none() && channel.0 == state.lifecycle.0 => {
                                preferences::mutate(
                                    &state,
                                    request.package_id,
                                    2,
                                    request.expected_generation,
                                    &[],
                                    &[],
                                )
                            }
                            Ok(_) => (
                                lifecycle::ComponentConfigStatus::Busy,
                                0,
                                "migration pending".to_string(),
                            ),
                            Err(_) => (
                                lifecycle::ComponentConfigStatus::InvalidArgs,
                                0,
                                "invalid request".to_string(),
                            ),
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlResetComponentConfigResponse {
                                status,
                                generation,
                                message: &message,
                            },
                        );
                    }
                    13 => {
                        let request =
                            lifecycle::AppLifecycleControlGetGenerationFloorRequest::decode(
                                req, &hs,
                            );
                        let (status, generation) = match request {
                            Ok(q) => state
                                .floor(q.target)
                                .map(|floor| (lifecycle::AppLifecycleStatus::Ok, floor))
                                .unwrap_or((lifecycle::AppLifecycleStatus::NotFound, 0)),
                            Err(_) => (lifecycle::AppLifecycleStatus::InvalidArgs, 0),
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlGetGenerationFloorResponse {
                                status,
                                generation,
                            },
                        );
                    }
                    14 => {
                        let request =
                            lifecycle::AppLifecycleControlCommitGenerationFloorRequest::decode(
                                req, &hs,
                            );
                        let (status, generation) = match request {
                            Ok(q) if pending.is_none() => {
                                match state.commit_floor(q.target, q.generation) {
                                    Ok(floor) => {
                                        source.changed(0);
                                        (lifecycle::AppLifecycleStatus::Ok, floor)
                                    }
                                    Err(_) => (lifecycle::AppLifecycleStatus::Storage, 0),
                                }
                            }
                            Ok(_) => (lifecycle::AppLifecycleStatus::LaunchFailed, 0),
                            Err(_) => (lifecycle::AppLifecycleStatus::InvalidArgs, 0),
                        };
                        lifecycle_reply(
                            channel,
                            &lifecycle::AppLifecycleControlCommitGenerationFloorResponse {
                                status,
                                generation,
                            },
                        );
                    }
                    _ => {}
                }
                if matches!(ordinal, 2 | 3 | 4 | 11 | 12 | 14 | 15) {
                    #[cfg(feature = "persistent")]
                    if ordinal != 2 {
                        let _ = state.sync_stores();
                    }
                    source.changed_keys(state.registry_keys(old_count));
                    source.changed_keys(
                        (0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64),
                    );
                    source.changed(0);
                    source.changed_keys(
                        (0..state.services.len()).map(|i| state::SERVICE_BASE + i as u64),
                    );
                    source.changed(5);
                    source.changed_keys(
                        (0..state.opener_bindings.len())
                            .map(|i| state::OPENER_BINDING_BASE + i as u64),
                    );
                    source.changed_keys(
                        (0..state.component_configs.len())
                            .map(|i| state::COMPONENT_CONFIG_BASE + i as u64),
                    );
                }
            }
        }
        if pending.is_none()
            && activation_sync_pending
            && activation_completion_reported
            && bexos_userspace::syscall::ticks() >= activation_sync_after
        {
            match state.sync_stores() {
                Ok(()) => {
                    activation_sync_pending = false;
                }
                Err(error) => {
                    log(&alloc::format!(
                        "appd: deferred activation sync failed: {error:?}\n"
                    ));
                }
            }
        }
        if pending.is_none() && !activation_sync_pending {
            let mut opener_updates = false;
            let old_opener_count = state.opener_bindings.len();
            let mut closed_openers = Vec::new();
            for index in 0..state.opener_bindings.len() {
                let binding = state.opener_bindings[index].clone();
                loop {
                    match Channel(binding.channel).try_recv() {
                        Ok(message) => {
                            handle_opener_message(
                                &mut state,
                                &mut kernel,
                                &binding,
                                &message.bytes,
                                &message.handles,
                            );
                            opener_updates = true;
                        }
                        Err(kernel_fidl::Status::ErrPeerClosed) => {
                            let _ = Memory::close(binding.channel);
                            closed_openers.push(binding.channel);
                            opener_updates = true;
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
            state
                .opener_bindings
                .retain(|b| !closed_openers.contains(&b.channel));
            if opener_updates {
                source.changed(state::COMMAND_STATE_KEY);
                #[cfg(feature = "persistent")]
                let _ = state.sync_stores();
                source.changed(0);
                source.changed(5);
                source
                    .changed_keys((0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64));
                source.changed_keys(
                    (0..old_opener_count.max(state.opener_bindings.len()))
                        .map(|i| state::OPENER_BINDING_BASE + i as u64),
                );
            }

            let mut version_updates = false;
            for channel in state.version_manager_bindings.clone() {
                while let Ok(message) = Channel(channel).try_recv() {
                    handle_version_manager_message(
                        &mut state,
                        channel,
                        &message.bytes,
                        &message.handles,
                    );
                    version_updates = true;
                }
            }
            if version_updates {
                #[cfg(feature = "persistent")]
                let _ = state.sync_stores();
                source.changed_keys(state.registry_keys(state.registry.list_packages().len()));
                source.changed_keys(
                    (0..state.version_manager_bindings.len())
                        .map(|i| state::VERSION_MANAGER_BINDING_BASE + i as u64),
                );
            }

            let configured = crate::package_install::acquire_configured(&mut state);
            let (completed, installed, drivers) = crate::package_install::poll(&mut state);
            // Pending reads are volatile migration state. Only completed installs
            // change the durable registry; queuing or failing a download must
            // not force a filesystem checkpoint in the service broker.
            let mut app_manager_updates = installed;
            if !drivers.is_empty() {
                for driver in &drivers {
                    if !state.input_hotplug.driver_packages.contains(driver) {
                        state.input_hotplug.driver_packages.push(driver.clone());
                    }
                }
                let mut gate = readiness::Gate::new();
                gate.registry = core::mem::take(&mut state.devices);
                gate.services = core::mem::take(&mut state.services);
                let bound = bind_preinstalled_drivers(
                    state.vfsd,
                    &drivers,
                    &state.registry,
                    &state.config,
                    &mut kernel,
                    &mut state.broker,
                    &mut gate,
                    &AppdWaveOrchestrator::new(),
                );
                state.devices = gate.registry;
                state.services = gate.services;
                if let Err(error) = bound {
                    log(&format!("appd: configured driver bind failed {error}\n"));
                }
                source.changed_keys(state.keys());
            }
            if configured || completed {
                source.changed(crate::package_install::KEY);
            }
            for binding in state.app_manager_bindings.clone() {
                while let Ok(message) = Channel(binding.channel).try_recv() {
                    app_manager_updates |= crate::manager::handle_app_manager_message(
                        &mut state,
                        binding.channel,
                        &message.bytes,
                        &message.handles,
                    );
                    source.changed(crate::package_install::KEY);
                }
            }
            if app_manager_updates {
                source.changed(crate::package_install::KEY);
                source.changed_keys(state.registry_keys(state.registry.list_packages().len()));
                #[cfg(feature = "persistent")]
                let _ = state.sync_stores();
                source.changed(6);
                source.changed(5);
                source.changed_keys(
                    (0..state.app_manager_bindings.len())
                        .map(|i| state::APP_MANAGER_BINDING_BASE + i as u64),
                );
            }

            #[cfg(feature = "persistent")]
            let directory_launches_before = state.launches.len();
            let mut service_directory_updates = false;
            for binding in state.service_directory_bindings.clone() {
                while let Ok(message) = Channel(binding.channel).try_recv() {
                    handle_service_directory_message(
                        &mut state,
                        &mut kernel,
                        &binding,
                        binding.channel,
                        &message.bytes,
                        &message.handles,
                    );
                    service_directory_updates = true;
                }
            }
            if service_directory_updates {
                source.changed(shell::KEY);
                // Connecting to an already-running provider only changes channel
                // bindings. Persist only if lazy activation launched a process.
                #[cfg(feature = "persistent")]
                if state.launches.len() != directory_launches_before {
                    let _ = state.sync_stores();
                }
                source.changed(state::SERVICE_DIRECTORY_STATE_KEY);
                source.changed(state::LAZY_STATE_KEY);
                source
                    .changed_keys((0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64));
                source.changed_keys(
                    (0..state.services.len()).map(|i| state::SERVICE_BASE + i as u64),
                );
            }

            let mut worker_updates = false;
            for binding in state.worker_launcher_bindings.clone() {
                while let Ok(message) = Channel(binding.channel).try_recv() {
                    handle_worker_launcher_message(
                        &mut state,
                        &mut kernel,
                        &binding,
                        binding.channel,
                        &message.bytes,
                        &message.handles,
                    );
                    worker_updates = true;
                }
            }
            if worker_updates {
                #[cfg(feature = "persistent")]
                let _ = state.sync_stores();
                source
                    .changed_keys((0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64));
                source.changed_keys(
                    (0..state.worker_policy_watchers.len())
                        .map(|i| state::WORKER_POLICY_WATCHER_BASE + i as u64),
                );
            }

            if command_runtime::poll(&mut state, &mut kernel) {
                source.changed(state::COMMAND_STATE_KEY);
                source.changed(0);
                source.changed(5);
                source
                    .changed_keys((0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64));
            }
        }
        async_yield().await;
    }
}

async fn async_yield() {
    bexos_userspace::yield_now();
}

fn poll_completed_boot_service(state: &mut state::AppdState, source: &mut Source) {
    let mut completed = None;
    for (index, service) in state.services.iter().enumerate() {
        if !matches!(
            service.package.as_str(),
            "bexos.service.splashd" | "bexos.service.scened"
        ) {
            continue;
        }
        let Ok(message) = Channel(service.manager).try_recv() else {
            continue;
        };
        if message.bytes == bexos_graphics_runtime::scheduling::PROFILE_MESSAGE
            && message.handles.len() == 1
        {
            if let Err(error) =
                graphics::apply_render_profile(service.thread_handle, message.handles[0])
            {
                log(&format!(
                    "appd: graphics profile attachment failed {error:?}\n"
                ));
            }
            continue;
        }
        if service.package == "bexos.service.splashd"
            && message.bytes == b"bexos.graphics.completed"
            && message.handles.is_empty()
        {
            completed = Some(index);
        }
        for h in message.handles {
            let _ = Memory::close(h);
        }
    }
    let Some(index) = completed else {
        return;
    };
    let service = state.services.remove(index);
    let _ = state
        .registry
        .mark_lifecycle(&service.package, LifecycleState::Stopped);
    state.broker.remove_provider_package(&service.package);
    for h in [
        service.process_handle,
        service.space_handle,
        service.thread_handle,
        service.manager,
        service.migration,
        service.archive,
    ] {
        if h != 0 {
            let _ = Memory::close(h);
        }
    }
    // BootFS services are outside the restart watchdog. Removing their managed
    // migration entry also prevents a completed splash from being replaced.
    source.changed_keys(state.keys());
    source.changed(state::SERVICE_BASE + state.services.len() as u64);
    log("appd: splash completed; restart disabled\n");
}

fn poll_lazy_provider_control(state: &mut state::AppdState) {
    let services = state.services.clone();
    for service in services {
        let uid = state
            .launches
            .iter()
            .find(|launch| {
                launch.package == service.package
                    && launch.process == service.process
                    && launch.manager == service.manager
            })
            .map_or(SYSTEM_UID, |launch| launch.uid);
        if !state.lazy.providers().iter().any(|provider| {
            provider.package == service.package
                && provider.process == service.process
                && provider.uid == uid
        }) {
            continue;
        }
        let Ok(message) = Channel(service.manager).try_recv() else {
            continue;
        };
        let Ok(text) = core::str::from_utf8(&message.bytes) else {
            close_handles(&message.handles);
            continue;
        };
        let Some(generation) = text
            .strip_prefix("bexos.lazy.idle.v1|")
            .and_then(|value| value.parse::<u64>().ok())
        else {
            close_handles(&message.handles);
            continue;
        };
        let accepted =
            state
                .lazy
                .begin_idle_stop(&service.package, &service.process, uid, generation);
        let response = if accepted {
            alloc::format!("bexos.lazy.idle.ok|{generation}")
        } else {
            alloc::format!("bexos.lazy.idle.busy|{generation}")
        };
        let _ = Channel(service.manager).send(response.as_bytes(), &[]);
        close_handles(&message.handles);
    }
}

fn republish_dormant_provider(
    state: &mut state::AppdState,
    package: &str,
    process: &str,
    uid: u64,
) {
    let Ok(record) = state.registry.record(package) else {
        return;
    };
    let Ok(manifest) = Manifest::decode(&record.manifest_bytes) else {
        return;
    };
    for service in manifest.services_exposed.iter().cloned() {
        if service.activation != crate::ServiceActivation::Lazy {
            continue;
        }
        let Ok(provider) = manifest.service_provider_process(&service) else {
            continue;
        };
        if provider.name != process {
            continue;
        }
        let _ = state.broker.publish_dormant_interface(
            manifest.package_name.clone(),
            service,
            process.to_string(),
        );
        state.lazy.declare_manifest(&manifest, uid);
    }
}

fn poll_lifecycle_watchdog(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    source: &mut Source,
) {
    let now_ns = monotonic_ns().unwrap_or_else(|| {
        bexos_userspace::syscall::ticks().saturating_mul(1_000_000_000)
            / bexos_userspace::syscall::frequency().max(1)
    });
    let policy = state.config.app_lifecycle_policy;
    let mut changed = false;
    let mut registry_changed = false;
    let mut removed_indexes = Vec::new();
    let launch_snapshot = state.launches.clone();
    for (index, launch) in launch_snapshot.iter().enumerate() {
        if launch.job_token != 0 {
            continue;
        }
        if !is_watchdog_managed(&state.registry, &launch.package) {
            continue;
        }
        ensure_watchdog_record(state, launch, now_ns);
        if process_terminated(launch.process_handle) {
            removed_indexes.push(index);
        }
    }
    for index in removed_indexes.into_iter().rev() {
        registry_changed = true;
        let launch = state.launches.remove(index);
        if state
            .lazy
            .is_intentional_stop(&launch.package, &launch.process, launch.uid)
        {
            cleanup_dead_launch(state, &launch);
            republish_dormant_provider(state, &launch.package, &launch.process, launch.uid);
            let relaunch = state
                .lazy
                .stopped(&launch.package, &launch.process, launch.uid);
            if relaunch {
                let initial =
                    state
                        .lazy
                        .take_starting_pending(&launch.package, &launch.process, launch.uid);
                let lazy_idle_timeout_ms =
                    initial.first().map_or(0, |binding| binding.idle_timeout_ms);
                let _ = launch_application(
                    &mut state.registry,
                    &mut state.launches,
                    &mut state.services,
                    state.vfsd,
                    state.users,
                    kernel,
                    &mut state.broker,
                    &mut state.permission_routes,
                    &mut state.permissions,
                    &mut state.opener_bindings,
                    &mut state.version_manager_bindings,
                    &mut state.app_manager_bindings,
                    &mut state.worker_launcher_bindings,
                    &mut state.service_directory_bindings,
                    &mut state.lazy,
                    &state.domain_associations,
                    &state.config,
                    &state.component_configs,
                    &launch.package,
                    &launch.process,
                    0,
                    launch.uid,
                    None,
                    false,
                    None,
                    None,
                    initial,
                    lazy_idle_timeout_ms,
                    false,
                    None,
                );
            }
            changed = true;
            continue;
        }
        cleanup_dead_launch(state, &launch);
        let _ = state
            .lazy
            .stopped(&launch.package, &launch.process, launch.uid);
        republish_dormant_provider(state, &launch.package, &launch.process, launch.uid);
        let has_rollback = state
            .registry
            .active_pin(&launch.package)
            .and_then(|pin| pin.rollback_target_version.as_ref())
            .is_some();
        if let Some(watchdog) = state.watchdogs.iter_mut().find(|watchdog| {
            watchdog.package_id == launch.package
                && watchdog.process_name == launch.process
                && watchdog.instance_id == launch.instance_id
                && watchdog.uid == launch.uid
        }) {
            match watchdog.exit(now_ns, policy, has_rollback) {
                crate::watchdog::WatchdogAction::RollbackAndRestart { package_id, .. } => {
                    let _ = state
                        .registry
                        .mark_pin_health(&package_id, HealthCheckStatus::CrashLoop);
                    let _ = state.registry.rollback_to_previous(&package_id);
                }
                crate::watchdog::WatchdogAction::StopCrashLoop { package_id } => {
                    let _ = state
                        .registry
                        .mark_pin_health(&package_id, HealthCheckStatus::CrashLoop);
                }
                crate::watchdog::WatchdogAction::Restart { .. }
                | crate::watchdog::WatchdogAction::PromoteHealthy { .. } => {}
            }
            changed = true;
        }
    }
    let mut due = Vec::new();
    for watchdog in &mut state.watchdogs {
        if let Some(action) = watchdog.ready(now_ns, policy) {
            if let crate::watchdog::WatchdogAction::PromoteHealthy { package_id } = action {
                let _ = state
                    .registry
                    .mark_pin_health(&package_id, HealthCheckStatus::Healthy);
                changed = true;
            }
        }
        // Completed commands stay stopped. Selected shells are recovered by
        // session policy with their role grants, including adopted records.
        if !state
            .registry
            .record(&watchdog.package_id)
            .ok()
            .and_then(|r| Manifest::decode(&r.manifest_bytes).ok())
            .is_some_and(|m| crate::watchdog::automatic_restart(&m, &watchdog.process_name))
        {
            continue;
        }
        if watchdog.restart_due(now_ns)
            && !state.launches.iter().any(|launch| {
                launch.package == watchdog.package_id
                    && launch.process == watchdog.process_name
                    && launch.instance_id == watchdog.instance_id
                    && launch.uid == watchdog.uid
            })
        {
            let lazy_without_demand = state.lazy.providers().iter().any(|provider| {
                provider.package == watchdog.package_id
                    && provider.process == watchdog.process_name
                    && provider.uid == watchdog.uid
            }) && !state.lazy.has_pending_demand(
                &watchdog.package_id,
                &watchdog.process_name,
                watchdog.uid,
            );
            if lazy_without_demand {
                continue;
            }
            due.push((
                watchdog.package_id.clone(),
                watchdog.process_name.clone(),
                watchdog.instance_id.clone(),
                watchdog.uid,
            ));
            watchdog.mark_restarted(now_ns);
            registry_changed = true;
            let _ = state.registry.mark_pin_health(
                &watchdog.package_id,
                crate::watchdog::health_for_phase(watchdog.phase),
            );
            changed = true;
        }
    }
    for (package, process, instance_id, uid) in due {
        let _ = launch_application(
            &mut state.registry,
            &mut state.launches,
            &mut state.services,
            state.vfsd,
            state.users,
            kernel,
            &mut state.broker,
            &mut state.permission_routes,
            &mut state.permissions,
            &mut state.opener_bindings,
            &mut state.version_manager_bindings,
            &mut state.app_manager_bindings,
            &mut state.worker_launcher_bindings,
            &mut state.service_directory_bindings,
            &mut state.lazy,
            &state.domain_associations,
            &state.config,
            &state.component_configs,
            &package,
            &process,
            0,
            uid,
            None,
            false,
            (!instance_id.is_empty()).then_some(instance_id.as_str()),
            None,
            Vec::new(),
            0,
            false,
            None,
        );
    }
    if changed {
        // Healthy promotion only changes active-pin health. Rewriting every
        // package/permission database here blocks unrelated lifecycle requests.
        let result = if registry_changed {
            state.sync_stores()
        } else {
            state.sync_active_pins()
        };
        if let Err(error) = result {
            log(&format!("appd: watchdog persistence failed: {error:?}\n"));
        }
        source.changed(0);
        source.changed_keys(
            (0..state.registry.active_pins().len()).map(|i| state::ACTIVE_PIN_BASE + i as u64),
        );
        if registry_changed {
            source.changed_keys(state.registry_keys(state.registry.list_packages().len()));
        }
        source.changed_keys((0..state.launches.len()).map(|i| state::LAUNCH_BASE + i as u64));
        source.changed_keys((0..state.watchdogs.len()).map(|i| state::WATCHDOG_BASE + i as u64));
    }
}

fn monotonic_ns() -> Option<u64> {
    let mut client = kernel_fidl::ClockPublicClient::new(KernelTransport(8));
    let mut req_bytes = [0u8; 64];
    let mut req_handles = [kernel_fidl::HandleRef { raw: 0 }; 1];
    let mut resp_bytes = [0u8; 64];
    let mut resp_handles = [kernel_fidl::HandleRef { raw: 0 }; 1];
    let response = client
        .get_time(
            &kernel_fidl::ClockGetTimeRequest {
                clock_type: kernel_fidl::ClockType::Monotonic,
            },
            &mut req_bytes,
            &mut req_handles,
            &mut resp_bytes,
            &mut resp_handles,
        )
        .ok()?;
    (response.status == kernel_fidl::Status::Ok).then_some(response.nanos)
}

fn process_terminated(process_handle: u64) -> bool {
    let item = kernel_fidl::InlineVectorStruct1 {
        h: kernel_fidl::HandleRef {
            raw: process_handle,
        },
        signals: kernel_fidl::Signals::TERMINATED,
    };
    let items = [item];
    let mut client = kernel_fidl::TaskControlPublicClient::new(KernelTransport(3));
    let mut req_bytes = [0u8; 128];
    let mut req_handles = [kernel_fidl::HandleRef { raw: 0 }; 4];
    let mut resp_bytes = [0u8; 64];
    let mut resp_handles = [kernel_fidl::HandleRef { raw: 0 }; 1];
    client
        .wait_many(
            &kernel_fidl::TaskControlWaitManyRequest {
                items: kernel_fidl::WireVector::from_slice(&items),
                deadline_nanos: NO_WAIT_DEADLINE_NANOS,
            },
            &mut req_bytes,
            &mut req_handles,
            &mut resp_bytes,
            &mut resp_handles,
        )
        .is_ok_and(|response| {
            response.status == kernel_fidl::Status::Ok
                && response.observed_signals.0 & kernel_fidl::Signals::TERMINATED.0 != 0
        })
}

pub(crate) fn handle_peer_closed(handle: u64) -> bool {
    let item = kernel_fidl::InlineVectorStruct1 {
        h: kernel_fidl::HandleRef { raw: handle },
        signals: kernel_fidl::Signals::PEER_CLOSED,
    };
    let items = [item];
    let mut client = kernel_fidl::TaskControlPublicClient::new(KernelTransport(3));
    let mut req_bytes = [0u8; 128];
    let mut req_handles = [kernel_fidl::HandleRef { raw: 0 }; 4];
    let mut resp_bytes = [0u8; 64];
    let mut resp_handles = [kernel_fidl::HandleRef { raw: 0 }; 1];
    client
        .wait_many(
            &kernel_fidl::TaskControlWaitManyRequest {
                items: kernel_fidl::WireVector::from_slice(&items),
                deadline_nanos: NO_WAIT_DEADLINE_NANOS,
            },
            &mut req_bytes,
            &mut req_handles,
            &mut resp_bytes,
            &mut resp_handles,
        )
        .is_ok_and(|response| {
            response.status == kernel_fidl::Status::Ok
                && response.observed_signals.0 & kernel_fidl::Signals::PEER_CLOSED.0 != 0
        })
}

fn poll_permission_route_clients(state: &mut state::AppdState) {
    let closed = state
        .permission_routes
        .routes()
        .iter()
        .filter(|route| handle_peer_closed(route.client_endpoint.object_id))
        .map(|route| route.client_endpoint.object_id)
        .collect::<Vec<_>>();
    for endpoint in closed {
        let removed = state.permission_routes.remove_client_endpoint(endpoint);
        close_permission_routes(removed);
    }
}

fn is_watchdog_managed(registry: &MemoryAppRegistry, package_id: &str) -> bool {
    registry.record(package_id).is_ok_and(|record| {
        record.protected
            && record.multi_version_policy == crate::MultiVersionPolicy::SingleActiveOnly
    })
}

fn ensure_watchdog_record(state: &mut state::AppdState, launch: &state::LaunchRecord, now_ns: u64) {
    if state.watchdogs.iter().any(|watchdog| {
        watchdog.package_id == launch.package
            && watchdog.process_name == launch.process
            && watchdog.instance_id == launch.instance_id
            && watchdog.uid == launch.uid
    }) {
        return;
    }
    if let Ok(record) = state.registry.record(&launch.package) {
        if !Manifest::decode(&record.manifest_bytes).is_ok_and(|m| {
            m.processes
                .iter()
                .any(|p| p.name == launch.process && p.service)
        }) {
            return;
        }
        state.watchdogs.push(crate::watchdog::WatchdogRecord::new(
            record.package_id.clone(),
            launch.process.clone(),
            launch.instance_id.clone(),
            launch.uid,
            record.version.clone(),
            now_ns,
        ));
    }
}

fn cleanup_dead_launch(state: &mut state::AppdState, launch: &state::LaunchRecord) {
    // Lifecycle and migration records share process descriptors. Retire only
    // the migration-specific handles here; the launch owns the common set.
    state.services.retain(|service| {
        if service.process_handle != launch.process_handle {
            return true;
        }
        for handle in [service.migration, service.archive, service.resource_job] {
            if handle != 0 && !launch.handles().contains(&handle) {
                let _ = Memory::close(handle);
            }
        }
        false
    });
    let _ = state
        .registry
        .mark_lifecycle(&launch.package, LifecycleState::Stopped);
    if launch.instance_id.is_empty() {
        state.broker.remove_provider_package(&launch.package);
    } else {
        state
            .broker
            .remove_instance(&launch.package, &launch.instance_id);
        for domain in state
            .config
            .network_policy
            .domains
            .iter()
            .filter(|domain| domain.isolation_group == launch.instance_id)
        {
            state.broker.remove_instance(&launch.package, &domain.name);
        }
    }
    state
        .opener_bindings
        .retain(|binding| binding.package != launch.package || binding.uid != launch.uid);
    state
        .version_manager_bindings
        .retain(|channel| *channel != launch.manager);
    state
        .app_manager_bindings
        .retain(|binding| binding.package != launch.package || binding.uid != launch.uid);
    state
        .worker_launcher_bindings
        .retain(|binding| binding.package != launch.package || binding.uid != launch.uid);
    let removed =
        state
            .permission_routes
            .remove_caller_process(&launch.package, &launch.process, launch.uid);
    close_permission_routes(removed);
    for handle in launch.handles() {
        let _ = Memory::close(handle);
    }
}

fn handle_version_manager_message(
    state: &mut state::AppdState,
    channel: u64,
    bytes: &[u8],
    handles: &[u64],
) {
    let (ordinal, req) = envelope(bytes);
    let hs = version_manager_refs(handles);
    match ordinal {
        1 => {
            let request = version_manager::VersionManagerListVersionsRequest::decode(req, &hs);
            let mut entries = Vec::new();
            let status = match request {
                Ok(request) => {
                    for record in state.registry.list_packages() {
                        if record.package_id != request.package_id {
                            continue;
                        }
                        match version_manager_entry(state.vfsd, &state.registry, record) {
                            Ok(entry) => entries.push(entry),
                            Err(status) => {
                                let response =
                                    version_manager::VersionManagerListVersionsResponse {
                                        status,
                                        versions: version_manager::WireVector::from_slice(&[]),
                                    };
                                version_manager_reply(Channel(channel), &response);
                                return;
                            }
                        }
                    }
                    if entries.is_empty() {
                        version_manager::VersionManagerStatus::NotFound
                    } else {
                        version_manager::VersionManagerStatus::Ok
                    }
                }
                Err(_) => version_manager::VersionManagerStatus::InvalidArgs,
            };
            version_manager_reply(
                Channel(channel),
                &version_manager::VersionManagerListVersionsResponse {
                    status,
                    versions: version_manager::WireVector::from_slice(&entries),
                },
            );
        }
        2 => {
            let request = version_manager::VersionManagerPinActiveVersionRequest::decode(req, &hs);
            let status = match request {
                Ok(request) => match state.registry.pin_active_version(
                    request.package_id,
                    semver_from_fidl(&request.target_version),
                ) {
                    Ok(()) => match state.sync_stores() {
                        Ok(()) => version_manager::VersionManagerStatus::Ok,
                        Err(_) => version_manager::VersionManagerStatus::Storage,
                    },
                    Err(_) => version_manager::VersionManagerStatus::NotFound,
                },
                Err(_) => version_manager::VersionManagerStatus::InvalidArgs,
            };
            version_manager_reply(
                Channel(channel),
                &version_manager::VersionManagerPinActiveVersionResponse { status },
            );
        }
        3 => {
            let request =
                version_manager::VersionManagerRollbackToPreviousRequest::decode(req, &hs);
            let mut restored_source = crate::SemVer::default();
            let status = match request {
                Ok(request) => match state.registry.rollback_to_previous(request.package_id) {
                    Ok(version) => {
                        restored_source = version;
                        match state.sync_stores() {
                            Ok(()) => version_manager::VersionManagerStatus::Ok,
                            Err(_) => version_manager::VersionManagerStatus::Storage,
                        }
                    }
                    Err(_) => version_manager::VersionManagerStatus::NotFound,
                },
                Err(_) => version_manager::VersionManagerStatus::InvalidArgs,
            };
            let restored_version = semver_to_fidl(&restored_source);
            version_manager_reply(
                Channel(channel),
                &version_manager::VersionManagerRollbackToPreviousResponse {
                    status,
                    restored_version,
                },
            );
        }
        4 => {
            let request =
                version_manager::VersionManagerPruneInactiveVersionsRequest::decode(req, &hs);
            let (status, reclaimed_bytes) = match request {
                Ok(request) => prune_inactive_versions(
                    &mut state.registry,
                    state.vfsd,
                    request.package_id,
                    request.keep_last_n,
                )
                .and_then(|bytes| {
                    state.sync_stores().map(|_| bytes).map_err(|_| PruneError {
                        status: fs_fidl::FsStatus::Io,
                        reclaimed_bytes: bytes,
                    })
                })
                .map(|bytes| (version_manager::VersionManagerStatus::Ok, bytes))
                .unwrap_or_else(|error| {
                    (
                        if error.status == fs_fidl::FsStatus::NotFound {
                            version_manager::VersionManagerStatus::NotFound
                        } else {
                            version_manager::VersionManagerStatus::Storage
                        },
                        error.reclaimed_bytes,
                    )
                }),
                Err(_) => (version_manager::VersionManagerStatus::InvalidArgs, 0),
            };
            version_manager_reply(
                Channel(channel),
                &version_manager::VersionManagerPruneInactiveVersionsResponse {
                    status,
                    reclaimed_bytes,
                },
            );
        }
        _ => {}
    }
}

fn version_manager_entry<'a>(
    vfsd: Channel,
    registry: &MemoryAppRegistry,
    record: &'a bexos_app_registry::AppRecord,
) -> Result<version_manager::AppVersionEntry<'a>, version_manager::VersionManagerStatus> {
    let disk_usage_bytes = match record.install_source {
        InstallSource::Bootfs => 0,
        _ => {
            vfs::get_package_archive_attributes(vfsd, &record.archive_id())
                .map_err(|_| version_manager::VersionManagerStatus::Storage)?
                .storage_allocated_bytes
        }
    };
    Ok(version_manager::AppVersionEntry {
        package_id: &record.package_id,
        version: semver_to_fidl(&record.version),
        policy: version_policy_to_fidl(record.multi_version_policy),
        is_active: registry
            .active_pin(&record.package_id)
            .is_some_and(|pin| pin.pinned_version == record.version),
        disk_usage_bytes,
    })
}

fn prune_inactive_versions(
    registry: &mut MemoryAppRegistry,
    vfsd: Channel,
    package_id: &str,
    keep_last_n: u32,
) -> Result<u64, PruneError> {
    let candidates = registry
        .prune_inactive_candidates(package_id, keep_last_n)
        .map_err(|error| match error {
            bexos_app_registry::RegistryError::NotFound => PruneError {
                status: fs_fidl::FsStatus::NotFound,
                reclaimed_bytes: 0,
            },
            _ => PruneError {
                status: fs_fidl::FsStatus::InvalidArgs,
                reclaimed_bytes: 0,
            },
        })?;
    if candidates.is_empty() {
        return Ok(0);
    }
    let mut attrs = Vec::new();
    for record in &candidates {
        if record.install_source == InstallSource::Bootfs {
            attrs.push(0);
        } else {
            attrs.push(
                vfs::get_package_archive_attributes(vfsd, &record.archive_id())
                    .map_err(|status| PruneError {
                        status,
                        reclaimed_bytes: 0,
                    })?
                    .storage_allocated_bytes,
            );
        }
    }
    let mut reclaimed = 0u64;
    for (record, bytes) in candidates.iter().zip(attrs) {
        if record.install_source != InstallSource::Bootfs {
            match vfs::delete_package_archive(vfsd, &record.package_key()) {
                Ok(()) | Err(fs_fidl::FsStatus::NotFound) => {}
                Err(status) => {
                    return Err(PruneError {
                        status,
                        reclaimed_bytes: reclaimed,
                    });
                }
            }
        }
        registry
            .remove_version(&record.package_id, &record.version)
            .map_err(|_| PruneError {
                status: fs_fidl::FsStatus::Io,
                reclaimed_bytes: reclaimed,
            })?;
        reclaimed = reclaimed.saturating_add(bytes);
    }
    Ok(reclaimed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PruneError {
    status: fs_fidl::FsStatus,
    reclaimed_bytes: u64,
}

fn semver_to_fidl(version: &crate::SemVer) -> version_manager::SemVer<'_> {
    version_manager::SemVer {
        major: version.major,
        minor: version.minor,
        patch: version.patch,
        build: version.build,
        prerelease: &version.prerelease,
    }
}

fn semver_from_fidl(version: &version_manager::SemVer<'_>) -> crate::SemVer {
    crate::SemVer {
        major: version.major,
        minor: version.minor,
        patch: version.patch,
        build: version.build,
        prerelease: version.prerelease.to_string(),
    }
}

fn version_policy_to_fidl(
    policy: crate::MultiVersionPolicy,
) -> version_manager::MultiVersionPolicy {
    match policy {
        crate::MultiVersionPolicy::ParallelExecution => {
            version_manager::MultiVersionPolicy::ParallelExecution
        }
        crate::MultiVersionPolicy::SharedStorageMulti => {
            version_manager::MultiVersionPolicy::SharedStorageMulti
        }
        _ => version_manager::MultiVersionPolicy::SingleActiveOnly,
    }
}

fn publish_running_services(
    broker: &mut AppdBroker,
    manifests: &[Manifest],
    services: &[state::ManagedService],
    devices: &crate::DeviceRegistry,
    config: &PlatformConfig,
) -> Result<(), crate::BindError> {
    for managed in services {
        if let Some(manifest) = manifests
            .iter()
            .find(|manifest| manifest.package_name == managed.package)
        {
            let active_nodes: Vec<_> = devices
                .nodes()
                .iter()
                .filter_map(|node| match &node.state {
                    crate::DeviceNodeState::Active(binding)
                        if binding.package_id == managed.package
                            && binding.process_name == managed.process
                            && binding
                                .manager_channel
                                .is_some_and(|c| c.object_id == managed.manager) =>
                    {
                        Some(node.info.node_id)
                    }
                    _ => None,
                })
                .collect();
            if active_nodes.is_empty() {
                publish_manifest_services_for_manager(
                    broker,
                    manifest,
                    managed.manager,
                    (!managed.instance_id.is_empty()).then_some(managed.instance_id.as_str()),
                    Some(config),
                )?;
            } else {
                for node_id in active_nodes {
                    publish_manifest_services_for_device(
                        broker,
                        manifest,
                        managed.manager,
                        node_id,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn publish_dormant_registry_services(
    broker: &mut AppdBroker,
    registry: &MemoryAppRegistry,
    lazy: &mut crate::lazy::LazyActivationState,
) -> Result<(), crate::BindError> {
    for record in registry.list_packages() {
        let Ok(active) = registry.record(&record.package_id) else {
            continue;
        };
        if active.version != record.version {
            continue;
        }
        let Ok(manifest) = Manifest::decode(&record.manifest_bytes) else {
            continue;
        };
        if manifest.validate_package_shape().is_err() {
            continue;
        }
        for service in manifest.services_exposed.iter().cloned() {
            if service.activation != crate::ServiceActivation::Lazy {
                continue;
            }
            let provider = manifest
                .service_provider_process(&service)
                .map_err(|_| crate::BindError::InvalidCapability)?;
            broker.publish_dormant_interface(
                manifest.package_name.clone(),
                service.clone(),
                provider.name.clone(),
            )?;
            lazy.declare_manifest(&manifest, SYSTEM_UID);
        }
    }
    Ok(())
}

fn publish_appd_opener_service(broker: &mut AppdBroker) -> Result<(), crate::BindError> {
    broker.publish_interface(
        state::APPD_PACKAGE,
        ExposedService {
            name: "Opener".into(),
            protocol: "bexos.app.opener.Opener".into(),
            lifecycle: Lifecycle::Singleton,
            visibility: Visibility::Public,
            bind_permission: None,
            metadata: Vec::new(),
            capabilities: vec![public_capability(&[1, 2, 3, 4, 5, 6])],
            ..ExposedService::default()
        },
        bexos_kernel_core::ipc::Capability {
            object_id: 0,
            rights: 0b11,
        },
    )
}

fn publish_appd_version_manager_service(broker: &mut AppdBroker) -> Result<(), crate::BindError> {
    broker.publish_interface(
        state::APPD_PACKAGE,
        ExposedService {
            name: "VersionManager".into(),
            protocol: "bexos.app.version_manager.VersionManager".into(),
            lifecycle: Lifecycle::Singleton,
            visibility: Visibility::Public,
            bind_permission: None,
            metadata: Vec::new(),
            capabilities: vec![public_capability(&[1, 2, 3, 4])],
            ..ExposedService::default()
        },
        bexos_kernel_core::ipc::Capability {
            object_id: 0,
            rights: 0b11,
        },
    )
}

fn publish_appd_manager_service(broker: &mut AppdBroker) -> Result<(), crate::BindError> {
    broker.publish_interface(
        state::APPD_PACKAGE,
        ExposedService {
            name: "AppManager".into(),
            protocol: "bexos.app.manager.AppManager".into(),
            lifecycle: Lifecycle::Singleton,
            visibility: Visibility::Public,
            bind_permission: None,
            metadata: Vec::new(),
            capabilities: vec![public_capability(&[1, 2, 3, 4])],
            ..ExposedService::default()
        },
        bexos_kernel_core::ipc::Capability {
            object_id: 0,
            rights: 0b11,
        },
    )
}

fn publish_appd_worker_launcher_service(broker: &mut AppdBroker) -> Result<(), crate::BindError> {
    let method_ordinals = app_worker::WORKER_LAUNCHER_PUBLIC_METHODS
        .iter()
        .map(|method| method.ordinal)
        .collect();
    broker.publish_interface(
        state::APPD_PACKAGE,
        ExposedService {
            name: "WorkerLauncher".into(),
            protocol: "bexos.app.worker.WorkerLauncher".into(),
            lifecycle: Lifecycle::Singleton,
            visibility: Visibility::Public,
            bind_permission: Some(crate::kernel_services::SYSTEM_PRIVILEGED_PERMISSION.into()),
            metadata: Vec::new(),
            capabilities: vec![CapabilityMetadata {
                capability: "Public".into(),
                permission: None,
                method_ordinals,
            }],
            ..ExposedService::default()
        },
        bexos_kernel_core::ipc::Capability {
            object_id: 0,
            rights: 0b11,
        },
    )
}

fn publish_appd_device_registry_service(broker: &mut AppdBroker) -> Result<(), crate::BindError> {
    broker.publish_interface(
        state::APPD_PACKAGE,
        ExposedService {
            name: "bexos.hardware.manager.DeviceRegistry".into(),
            protocol: "DeviceRegistry".into(),
            lifecycle: Lifecycle::Singleton,
            visibility: Visibility::Public,
            bind_permission: Some(crate::kernel_services::SYSTEM_PRIVILEGED_PERMISSION.into()),
            metadata: Vec::new(),
            capabilities: vec![
                public_capability(&[1, 2]),
                CapabilityMetadata {
                    capability: "UsbDescendantRegistrar".into(),
                    permission: None,
                    method_ordinals: vec![1, 2],
                },
                CapabilityMetadata {
                    capability: "I2cDescendantRegistrar".into(),
                    permission: None,
                    method_ordinals: vec![1, 2],
                },
                CapabilityMetadata {
                    capability: "SpiDescendantRegistrar".into(),
                    permission: None,
                    method_ordinals: vec![1, 2],
                },
            ],
            ..ExposedService::default()
        },
        bexos_kernel_core::ipc::Capability {
            object_id: 0,
            rights: 0b11,
        },
    )
}

fn publish_appd_service_directory_service(broker: &mut AppdBroker) -> Result<(), crate::BindError> {
    broker.publish_interface(
        state::APPD_PACKAGE,
        ExposedService {
            name: "ServiceDirectory".into(),
            protocol: "bexos.app.service_directory.ServiceDirectory".into(),
            lifecycle: Lifecycle::Singleton,
            visibility: Visibility::Public,
            bind_permission: None,
            metadata: Vec::new(),
            capabilities: vec![public_capability(&[1, 2, 3])],
            ..ExposedService::default()
        },
        bexos_kernel_core::ipc::Capability {
            object_id: 0,
            rights: 0b11,
        },
    )
}

fn public_capability(method_ordinals: &[u64]) -> CapabilityMetadata {
    CapabilityMetadata {
        capability: "Public".into(),
        permission: None,
        method_ordinals: method_ordinals.to_vec(),
    }
}

fn publish_manifest_services_for_manager(
    broker: &mut AppdBroker,
    manifest: &Manifest,
    manager: u64,
    instance_id: Option<&str>,
    config: Option<&PlatformConfig>,
) -> Result<(), crate::BindError> {
    for mut service in manifest.services_exposed.iter().cloned() {
        if service.activation == crate::ServiceActivation::Lazy {
            let provider = manifest
                .service_provider_process(&service)
                .map_err(|_| crate::BindError::InvalidCapability)?;
            broker.update_provider_manager(
                &manifest.package_name,
                &provider.name,
                bexos_kernel_core::ipc::Capability {
                    object_id: manager,
                    rights: 0b11,
                },
            );
            continue;
        }
        let endpoint = bexos_kernel_core::ipc::Capability {
            object_id: manager,
            rights: 0b11,
        };
        let Some(instance_id) = instance_id else {
            broker.publish_interface(manifest.package_name.clone(), service, endpoint)?;
            continue;
        };
        if service.name == "bexos.net.SocketProvider" {
            let Some(config) = config else {
                return Err(crate::BindError::InvalidCapability);
            };
            for domain in config
                .network_policy
                .domains
                .iter()
                .filter(|domain| domain.isolation_group == instance_id)
            {
                let mut scoped = service.clone();
                scoped
                    .metadata
                    .retain(|metadata| metadata.key != "network.domain");
                scoped.metadata.push(crate::manifest::Metadata {
                    key: "network.domain".into(),
                    value: domain.name.clone(),
                });
                broker.publish_instance_interface(
                    manifest.package_name.clone(),
                    domain.name.clone(),
                    scoped,
                    endpoint,
                )?;
            }
        } else if service.name == "bexos.net.Netstack" {
            let is_default =
                config.is_some_and(|config| {
                    config.network_policy.domains.iter().any(|domain| {
                        domain.system_default && domain.isolation_group == instance_id
                    })
                });
            if is_default {
                broker.publish_interface(manifest.package_name.clone(), service, endpoint)?;
            }
        } else {
            service
                .metadata
                .retain(|metadata| metadata.key != "network.instance");
            service.metadata.push(crate::manifest::Metadata {
                key: "network.instance".into(),
                value: instance_id.into(),
            });
            broker.publish_instance_interface(
                manifest.package_name.clone(),
                instance_id,
                service,
                endpoint,
            )?;
        }
    }
    Ok(())
}

fn publish_manifest_services_for_device(
    broker: &mut AppdBroker,
    manifest: &Manifest,
    manager: u64,
    node_id: u64,
) -> Result<(), crate::BindError> {
    for service in manifest.services_exposed.iter().cloned() {
        broker.publish_instance_interface(
            manifest.package_name.clone(),
            node_id.to_string(),
            service,
            bexos_kernel_core::ipc::Capability {
                object_id: manager,
                rights: 0b11,
            },
        )?;
    }
    Ok(())
}

fn wire_power_devices(
    powerd: Channel,
    drivers: &[Option<Channel>],
) -> Result<(), kernel_fidl::Status> {
    for driver in drivers.iter().flatten() {
        let (power_client, power_server) = Channel::pair()?;
        driver.send(
            b"bexos.power.DevicePowerControl|DevicePowerControl|Public|1",
            &[power_server.0],
        )?;
        powerd.send(
            b"bexos.power.DevicePowerControl|DevicePowerControl|Public|1",
            &[power_client.0],
        )?;
    }
    Ok(())
}

fn startup_service_grants(
    bindings: &[BoundCapability],
    package_id: &str,
    uid: u64,
) -> Vec<ServiceGrant> {
    bindings
        .iter()
        .map(|binding| ServiceGrant {
            service: binding.service_name.clone(),
            protocol: binding.protocol.clone(),
            capability: binding.capability.clone(),
            method_ordinals: binding.method_ordinals.clone(),
            permission_values: binding.permission_values.clone(),
            caller_package: Some(package_id.to_string()),
            caller_uid: Some(uid),
            caller_foreground: true,
            provider_instance_id: binding.provider_instance_id.clone(),
            endpoint: binding.client_endpoint.object_id,
        })
        .collect()
}

fn incoming_service_grants(bindings: &[BoundCapability]) -> Vec<ServiceGrant> {
    bindings
        .iter()
        .map(|binding| ServiceGrant {
            service: binding.service_name.clone(),
            protocol: binding.protocol.clone(),
            capability: binding.capability.clone(),
            method_ordinals: binding.method_ordinals.clone(),
            permission_values: binding.permission_values.clone(),
            caller_package: Some(binding.caller_package.clone()),
            caller_uid: binding.caller_uid,
            caller_foreground: binding.caller_foreground,
            provider_instance_id: binding.provider_instance_id.clone(),
            endpoint: binding.provider_endpoint.object_id,
        })
        .collect()
}

fn startup_namespace_entries(
    namespace: &crate::StartupNamespace,
) -> Vec<bexos_userspace::NamespaceEntry> {
    namespace
        .entries()
        .iter()
        .map(|entry| bexos_userspace::NamespaceEntry {
            path: entry.path.clone(),
            directory: entry.directory.raw,
        })
        .collect()
}

fn notify_bound_providers(bindings: &[BoundCapability]) -> Result<(), kernel_fidl::Status> {
    for binding in bindings {
        if binding.activation == crate::ServiceActivation::Lazy {
            continue;
        }
        if is_appd_opener_binding(binding)
            || is_appd_version_manager_binding(binding)
            || is_appd_manager_binding(binding)
            || is_appd_worker_launcher_binding(binding)
            || is_appd_service_directory_binding(binding)
            || is_appd_device_registry_binding(binding)
        {
            continue;
        }
        if is_kernel_binding(binding) {
            let _ = Memory::close(binding.provider_endpoint.object_id);
            continue;
        }
        let metadata = provider_binding_metadata(binding);
        let endpoint = Memory::duplicate(
            binding.provider_endpoint.object_id,
            binding.provider_endpoint.rights,
        )?;
        Channel(binding.provider_manager.object_id).send(&metadata, &[endpoint])?;
    }
    Ok(())
}

fn deliver_provider_endpoint(
    binding: &BoundCapability,
    metadata: &[u8],
) -> Result<(), kernel_fidl::Status> {
    if is_kernel_binding(binding) {
        return Ok(());
    }
    let endpoint = Memory::duplicate(
        binding.provider_endpoint.object_id,
        binding.provider_endpoint.rights,
    )?;
    match Channel(binding.provider_manager.object_id).send(metadata, &[endpoint]) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = Memory::close(endpoint);
            Err(error)
        }
    }
}

fn deliver_bound_capabilities_to_manager(
    bindings: Vec<BoundCapability>,
    manager: u64,
) -> Result<(), kernel_fidl::Status> {
    for mut binding in bindings {
        binding.provider_manager = bexos_kernel_core::ipc::Capability {
            object_id: manager,
            rights: 0b11,
        };
        let metadata = provider_binding_metadata(&binding);
        deliver_provider_endpoint(&binding, &metadata)?;
    }
    Ok(())
}

fn provider_uid_for_binding(
    binding: &BoundCapability,
) -> Result<u64, lifecycle::AppLifecycleStatus> {
    match binding.lifecycle {
        Lifecycle::Singleton => Ok(SYSTEM_UID),
        Lifecycle::UserScopedSingleton => binding
            .caller_uid
            .ok_or(lifecycle::AppLifecycleStatus::AccessDenied),
        _ => Err(lifecycle::AppLifecycleStatus::AccessDenied),
    }
}

fn activate_lazy_bindings(
    registry: &mut MemoryAppRegistry,
    launches: &mut Vec<state::LaunchRecord>,
    services: &mut Vec<state::ManagedService>,
    vfsd: Channel,
    users: Channel,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    broker: &mut AppdBroker,
    permission_routes: &mut crate::PermissionRouteTable,
    permissions: &mut MemoryPermissionStore,
    opener_bindings: &mut Vec<OpenerBinding>,
    version_manager_bindings: &mut Vec<u64>,
    app_manager_bindings: &mut Vec<crate::AppManagerBinding>,
    worker_launcher_bindings: &mut Vec<crate::AppManagerBinding>,
    service_directory_bindings: &mut Vec<crate::ServiceDirectoryBinding>,
    lazy: &mut crate::lazy::LazyActivationState,
    domain_associations: &bexos_domain_association::MemoryDomainAssociationCache,
    config: &PlatformConfig,
    component_configs: &[state::ComponentConfigRecord],
    bindings: &[BoundCapability],
) -> lifecycle::AppLifecycleStatus {
    for binding in bindings
        .iter()
        .filter(|binding| binding.activation == crate::ServiceActivation::Lazy)
    {
        let provider_process = match binding.provider_process.clone() {
            Some(process) => process,
            None => return lifecycle::AppLifecycleStatus::InvalidArgs,
        };
        let provider_uid = match provider_uid_for_binding(binding) {
            Ok(uid) => uid,
            Err(status) => return status,
        };
        let demand = match lazy.demand(binding.clone(), provider_uid) {
            Ok(demand) => demand,
            Err(_) => return lifecycle::AppLifecycleStatus::AccessDenied,
        };
        match demand {
            crate::lazy::LazyDemand::AlreadyRunning => {
                let Some(manager) = launches
                    .iter()
                    .rev()
                    .find(|launch| {
                        launch.package == binding.provider_package
                            && launch.process == provider_process
                            && launch.uid == provider_uid
                    })
                    .map(|launch| launch.manager)
                else {
                    return lifecycle::AppLifecycleStatus::LaunchFailed;
                };
                let queued =
                    lazy.running(&binding.provider_package, &provider_process, provider_uid);
                if deliver_bound_capabilities_to_manager(queued, manager).is_err() {
                    return lifecycle::AppLifecycleStatus::LaunchFailed;
                }
            }
            crate::lazy::LazyDemand::LaunchRequired => {
                let initial = lazy.take_starting_pending(
                    &binding.provider_package,
                    &provider_process,
                    provider_uid,
                );
                let status = launch_application(
                    registry,
                    launches,
                    services,
                    vfsd,
                    users,
                    kernel,
                    broker,
                    permission_routes,
                    permissions,
                    opener_bindings,
                    version_manager_bindings,
                    app_manager_bindings,
                    worker_launcher_bindings,
                    service_directory_bindings,
                    lazy,
                    domain_associations,
                    config,
                    component_configs,
                    &binding.provider_package,
                    &provider_process,
                    0,
                    provider_uid,
                    None,
                    false,
                    None,
                    None,
                    initial,
                    binding.idle_timeout_ms,
                    false,
                    None,
                );
                if status != lifecycle::AppLifecycleStatus::Ok {
                    let failed = lazy.failed_launch(
                        &binding.provider_package,
                        &provider_process,
                        provider_uid,
                    );
                    close_bound_capabilities(&failed);
                    return status;
                }
            }
            crate::lazy::LazyDemand::QueuedWhileStarting => {
                return lifecycle::AppLifecycleStatus::LaunchFailed;
            }
            crate::lazy::LazyDemand::QueuedWhileStopping => {}
        }
    }
    lifecycle::AppLifecycleStatus::Ok
}

fn replay_permission_routes_for_provider(
    routes: &mut crate::PermissionRouteTable,
    manifest: &Manifest,
    manager: u64,
) {
    let closed = routes.take_incompatible_provider(&manifest.package_name, manifest);
    close_permission_routes(closed);
    let indices = routes.replayable_for_provider(
        &manifest.package_name,
        bexos_kernel_core::ipc::Capability {
            object_id: manager,
            rights: 0b11,
        },
    );
    for index in indices {
        let Some(route) = routes.routes().get(index) else {
            continue;
        };
        let Ok(endpoint) = Memory::duplicate(
            route.retained_endpoint.object_id,
            route.retained_endpoint.rights,
        ) else {
            continue;
        };
        if Channel(manager).send(&route.metadata, &[endpoint]).is_err() {
            let _ = Memory::close(endpoint);
        }
    }
}

fn close_permission_routes(routes: Vec<crate::PermissionRoute>) {
    for route in routes {
        let _ = Memory::close(route.retained_endpoint.object_id);
        let _ = Memory::close(route.client_endpoint.object_id);
    }
}

fn is_appd_device_registry_binding(binding: &BoundCapability) -> bool {
    binding.provider_package == state::APPD_PACKAGE
        && binding.service_name == "bexos.hardware.manager.DeviceRegistry"
        && binding.protocol == "DeviceRegistry"
}

fn is_kernel_binding(binding: &BoundCapability) -> bool {
    binding.provider_package == crate::kernel_services::KERNEL_PROVIDER_PACKAGE
}

fn is_appd_opener_binding(binding: &BoundCapability) -> bool {
    binding.provider_package == state::APPD_PACKAGE
        && binding.service_name == "Opener"
        && binding.protocol == "bexos.app.opener.Opener"
}

fn is_appd_version_manager_binding(binding: &BoundCapability) -> bool {
    binding.provider_package == state::APPD_PACKAGE
        && binding.service_name == "VersionManager"
        && binding.protocol == "bexos.app.version_manager.VersionManager"
}

fn is_appd_manager_binding(binding: &BoundCapability) -> bool {
    binding.provider_package == state::APPD_PACKAGE
        && binding.service_name == "AppManager"
        && binding.protocol == "bexos.app.manager.AppManager"
}

fn is_appd_worker_launcher_binding(binding: &BoundCapability) -> bool {
    binding.provider_package == state::APPD_PACKAGE
        && binding.service_name == "WorkerLauncher"
        && binding.protocol == "bexos.app.worker.WorkerLauncher"
}

fn is_appd_service_directory_binding(binding: &BoundCapability) -> bool {
    binding.provider_package == state::APPD_PACKAGE
        && binding.service_name == "ServiceDirectory"
        && binding.protocol == "bexos.app.service_directory.ServiceDirectory"
}

fn close_bound_capabilities(bindings: &[BoundCapability]) {
    for binding in bindings {
        let _ = Memory::close(binding.client_endpoint.object_id);
        let _ = Memory::close(binding.provider_endpoint.object_id);
    }
}

pub(super) struct ResolvedLibraryDependency {
    package_name: String,
    export_name: String,
    mount_alias: Option<String>,
    export_path: String,
    symbol_prefix: String,
    abi_version: u32,
    soname: String,
    kind: PackageLibraryKind,
    direct_dependencies: Vec<crate::PackageLibraryDependency>,
    archive_id: String,
    directory: Channel,
}

pub(super) struct ResolvedSharedVault {
    name: String,
    directory: Channel,
}

pub fn shared_vault_storage_domain(
    cache: &bexos_domain_association::MemoryDomainAssociationCache,
    package_id: &str,
    peers: &[String],
) -> Result<Option<String>, fs_fidl::FsStatus> {
    if package_id.starts_with("system:") || package_id.starts_with("user:") {
        return Ok(None);
    }
    let mut selected = bexos_domain_association::authoritative_domain_for_package(package_id)
        .map_err(|_| fs_fidl::FsStatus::InvalidArgs)?;
    for peer in peers {
        if peer.starts_with("system:") || peer.starts_with("user:") {
            return Err(fs_fidl::FsStatus::AccessDenied);
        }
        let peer_domain = authoritative_domain_for_package_prefix(peer)
            .map_err(|_| fs_fidl::FsStatus::InvalidArgs)?;
        if peer_domain == selected {
            continue;
        }
        if !domains_bidirectionally_trust(cache, &selected, &peer_domain) {
            return Err(fs_fidl::FsStatus::AccessDenied);
        }
        if peer_domain < selected {
            selected = peer_domain;
        }
    }
    Ok(Some(selected))
}

pub fn shared_vault_allowed_for_runtime(
    registry: &MemoryAppRegistry,
    package_id: &str,
    manifest: &Manifest,
    uid: u64,
    vault: &crate::SharedVault,
) -> Result<(), fs_fidl::FsStatus> {
    validate_shared_vault_manifest(vault)?;
    if uid == SYSTEM_UID {
        if !package_id.starts_with("system:") {
            return Err(fs_fidl::FsStatus::AccessDenied);
        }
        if vault
            .allowed_peer_package_prefixes
            .iter()
            .any(|peer| !peer.starts_with("system:"))
        {
            return Err(fs_fidl::FsStatus::AccessDenied);
        }
    }
    if package_id.starts_with("system:") || package_id.starts_with("user:") {
        for peer in &vault.allowed_peer_package_prefixes {
            if !peer.starts_with("system:") && !peer.starts_with("user:") {
                return Err(fs_fidl::FsStatus::AccessDenied);
            }
            if peer != package_id
                && !peer_manifest_declares_vault(registry, peer, package_id, &vault.name)
            {
                return Err(fs_fidl::FsStatus::AccessDenied);
            }
        }
    } else {
        let _ = shared_vault_storage_domain(
            &bexos_domain_association::MemoryDomainAssociationCache::new(),
            package_id,
            &[],
        )?;
    }
    if !manifest
        .shared_vaults
        .iter()
        .any(|declared| declared.name == vault.name)
    {
        return Err(fs_fidl::FsStatus::AccessDenied);
    }
    Ok(())
}

fn validate_shared_vault_manifest(vault: &crate::SharedVault) -> Result<(), fs_fidl::FsStatus> {
    validate_shared_vault_name(&vault.name)?;
    if vault.access == crate::SharedVaultAccess::Unspecified {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    for peer in &vault.allowed_peer_package_prefixes {
        validate_package_prefix(peer)?;
    }
    Ok(())
}

fn authoritative_domain_for_package_prefix(prefix: &str) -> Result<String, fs_fidl::FsStatus> {
    if prefix.starts_with("system:") || prefix.starts_with("user:") {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    let reversed = prefix.split_once(':').map_or(prefix, |(left, _)| left);
    if reversed.is_empty() {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    bexos_domain_association::canonical_domain(
        &reversed.split('.').rev().collect::<Vec<_>>().join("."),
    )
    .map_err(|_| fs_fidl::FsStatus::InvalidArgs)
}

fn peer_manifest_declares_vault(
    registry: &MemoryAppRegistry,
    peer_prefix: &str,
    package_id: &str,
    vault_name: &str,
) -> bool {
    registry.list_packages().iter().any(|record| {
        package_matches_prefix(&record.package_id, peer_prefix)
            && Manifest::decode(&record.manifest_bytes).is_ok_and(|manifest| {
                manifest.shared_vaults.iter().any(|vault| {
                    vault.name == vault_name
                        && vault
                            .allowed_peer_package_prefixes
                            .iter()
                            .any(|peer| package_matches_prefix(package_id, peer))
                })
            })
    })
}

fn domains_bidirectionally_trust(
    cache: &bexos_domain_association::MemoryDomainAssociationCache,
    left: &str,
    right: &str,
) -> bool {
    domain_trusts(cache, left, right) && domain_trusts(cache, right, left)
}

fn domain_trusts(
    cache: &bexos_domain_association::MemoryDomainAssociationCache,
    owner: &str,
    peer: &str,
) -> bool {
    cache.get(owner).is_ok_and(|record| {
        record.trusted_peer_domains.iter().any(|origin| {
            bexos_domain_association::origin_domain(origin)
                .is_ok_and(|domain| domain.as_str() == peer)
        })
    })
}

fn validate_package_prefix(prefix: &str) -> Result<(), fs_fidl::FsStatus> {
    if prefix.is_empty()
        || prefix.len() > 128
        || !prefix.contains(':')
        || prefix
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_' && b != b':')
    {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    Ok(())
}

fn package_matches_prefix(package_id: &str, prefix: &str) -> bool {
    package_id == prefix
        || package_id
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with(':'))
}

fn validate_shared_vault_name(name: &str) -> Result<(), fs_fidl::FsStatus> {
    if name.is_empty()
        || name.len() > 64
        || name == "."
        || name == ".."
        || name
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_')
    {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    Ok(())
}

fn cache_boot_driver_recovery_images(
    boot: &Bootfs<'_>,
    manifests: &[Manifest],
    services: &[state::ManagedService],
) -> Result<crate::DriverRecoveryImageCache, String> {
    let mut cache = crate::DriverRecoveryImageCache::new();
    for service in services {
        if service.hardware == 0 {
            continue;
        }
        let Some(manifest) = manifests
            .iter()
            .find(|manifest| manifest.package_name == service.package)
        else {
            continue;
        };
        let Some(process) = manifest
            .processes
            .iter()
            .find(|process| process.name == service.process)
        else {
            continue;
        };
        if process.lifecycle.update_strategy != crate::manifest::UpdateStrategy::HeartTransplant {
            continue;
        }
        let package_path = process_executable_path(process).ok_or_else(|| {
            format!(
                "driver executable path missing {}:{}",
                service.package, service.process
            )
        })?;
        let executable_bytes = boot_package_bytes(boot, &manifest.package_name, package_path)?;
        let executable = match Memory::from_bytes(executable_bytes) {
            Ok(handle) => crate::CachedImageVmo {
                handle,
                len: executable_bytes.len() as u64,
            },
            Err(_) => return Err(format!("driver executable cache VMO {}", service.package)),
        };
        let mut libraries = Vec::new();
        let mut visiting = Vec::new();
        for dependency in &manifest.library_dependencies {
            if let Err(error) = cache_boot_library_recursive(
                boot,
                manifests,
                dependency,
                &mut visiting,
                &mut libraries,
            ) {
                let _ = Memory::close(executable.handle);
                close_cached_libraries(&libraries);
                return Err(error);
            }
        }
        log(&format!(
            "appd: cached driver recovery image package={} process={} libraries={}\n",
            service.package,
            service.process,
            libraries.len()
        ));
        cache.upsert(crate::DriverRecoveryImage {
            package_id: service.package.clone(),
            process_name: service.process.clone(),
            executable,
            libraries,
        });
    }
    Ok(cache)
}

fn cache_boot_library_recursive(
    boot: &Bootfs<'_>,
    manifests: &[Manifest],
    dependency: &crate::manifest::LibraryDependency,
    visiting: &mut Vec<(String, u32)>,
    libraries: &mut Vec<crate::CachedLibraryVmo>,
) -> Result<(), String> {
    if dependency.abi_version == 0 {
        return Err("driver library dependency ABI version missing".into());
    }
    if libraries.iter().any(|library| {
        library.package_id == dependency.package_name
            && library.abi_version == dependency.abi_version
    }) {
        return Ok(());
    }
    let key = (dependency.package_name.clone(), dependency.abi_version);
    if visiting.contains(&key) {
        return Err(format!(
            "driver library dependency cycle {}:{}",
            dependency.package_name, dependency.abi_version
        ));
    }
    if visiting.len() >= 16 || libraries.len() >= 64 {
        return Err("driver library dependency limit exceeded".into());
    }
    let selector = parse_package_selector(
        &dependency.package_name,
        dependency.version_requirement.as_deref(),
    )
    .map_err(|_| "driver library selector".to_string())?;
    let manifest = manifests
        .iter()
        .find(|manifest| manifest.package_name == selector.package)
        .ok_or_else(|| {
            format!(
                "driver library manifest missing {}",
                dependency.package_name
            )
        })?;
    if manifest.package_kind != crate::manifest::PackageKind::Library {
        return Err(format!(
            "driver library dependency is not a library {}",
            dependency.package_name
        ));
    }
    let export = manifest
        .library_exports
        .iter()
        .find(|export| {
            export.abi_version == dependency.abi_version
                && !export.path.is_empty()
                && (export.kind == LibraryExportKind::WasmComponent
                    || (!export.soname.is_empty() && !export.symbol_prefix.is_empty()))
        })
        .ok_or_else(|| format!("driver library export missing {}", dependency.package_name))?;
    if libraries.iter().any(|library| {
        library.package_id == manifest.package_name
            && library.export_name == export.name
            && library.abi_version == export.abi_version
    }) {
        return Ok(());
    }
    visiting.push(key);
    let child_result = (|| -> Result<(), String> {
        for child in &manifest.library_dependencies {
            cache_boot_library_recursive(boot, manifests, child, visiting, libraries)?;
        }
        Ok(())
    })();
    let _ = visiting.pop();
    child_result?;
    let mut direct_dependencies = Vec::new();
    for child in &manifest.library_dependencies {
        let child_selector =
            parse_package_selector(&child.package_name, child.version_requirement.as_deref())
                .map_err(|_| "driver child library selector".to_string())?;
        let child_manifest = manifests
            .iter()
            .find(|manifest| manifest.package_name == child_selector.package)
            .ok_or_else(|| {
                format!(
                    "driver child library manifest missing {}",
                    child.package_name
                )
            })?;
        let child_export = child_manifest
            .library_exports
            .iter()
            .find(|export| export.abi_version == child.abi_version && !export.path.is_empty())
            .ok_or_else(|| format!("driver child library export missing {}", child.package_name))?;
        direct_dependencies.push(crate::PackageLibraryDependency {
            package_name: child_manifest.package_name.clone(),
            abi_version: child_export.abi_version,
            soname: child_export.soname.clone(),
        });
    }
    let bytes = boot_package_bytes(boot, &manifest.package_name, &export.path)?;
    let handle = Memory::from_bytes(bytes)
        .map_err(|_| format!("driver library cache VMO {}", manifest.package_name))?;
    libraries.push(crate::CachedLibraryVmo {
        package_id: manifest.package_name.clone(),
        export_name: export.name.clone(),
        soname: export.soname.clone(),
        symbol_prefix: export.symbol_prefix.clone(),
        abi_version: export.abi_version,
        kind: match export.kind {
            LibraryExportKind::Native => PackageLibraryKind::Native,
            LibraryExportKind::WasmComponent => PackageLibraryKind::WasmComponent,
            LibraryExportKind::Unspecified => return Err("driver library kind unspecified".into()),
        },
        direct_dependencies,
        image: crate::CachedImageVmo {
            handle,
            len: bytes.len() as u64,
        },
    });
    Ok(())
}

fn process_executable_path(process: &crate::manifest::Process) -> Option<&str> {
    match &process.runner_options {
        Some(crate::ProcessRunnerOptions::Elf(options)) => Some(&options.path),
        Some(crate::ProcessRunnerOptions::Wasm(options)) => Some(&options.path),
        Some(crate::ProcessRunnerOptions::Nix(options)) => Some(&options.path),
        _ => None,
    }
}

fn boot_package_bytes<'a>(
    boot: &'a Bootfs<'a>,
    package: &str,
    package_path: &str,
) -> Result<&'a [u8], String> {
    let Some(path) = package_path.strip_prefix("/pkg/") else {
        return Err(format!("package path must be under /pkg: {package_path}"));
    };
    let boot_path = format!("/boot/pkg/{package}/{path}");
    boot.find(&boot_path)
        .map_err(|_| format!("boot package lookup {boot_path}"))?
        .map(|entry| entry.bytes)
        .ok_or_else(|| format!("boot package file missing {boot_path}"))
}

fn close_cached_libraries(libraries: &[crate::CachedLibraryVmo]) {
    for library in libraries {
        let _ = Memory::close(library.image.handle);
    }
}

fn resolve_shared_vaults(
    vfsd: Channel,
    registry: &MemoryAppRegistry,
    domains: &bexos_domain_association::MemoryDomainAssociationCache,
    package_id: &str,
    uid: u64,
    manifest: &Manifest,
) -> Result<Vec<ResolvedSharedVault>, fs_fidl::FsStatus> {
    let mut roots = Vec::new();
    for vault in &manifest.shared_vaults {
        shared_vault_allowed_for_runtime(registry, package_id, manifest, uid, vault)?;
        let domain = match shared_vault_storage_domain(
            domains,
            package_id,
            &vault.allowed_peer_package_prefixes,
        )? {
            Some(domain) => domain,
            None => String::new(),
        };
        let system = uid == SYSTEM_UID && package_id.starts_with("system:");
        let directory =
            match vfs::get_shared_vault_directory(vfsd, uid, &domain, &vault.name, system) {
                Ok(directory) => directory,
                Err(status) => {
                    close_shared_vault_roots(roots);
                    return Err(status);
                }
            };
        roots.push(ResolvedSharedVault {
            name: vault.name.clone(),
            directory,
        });
    }
    Ok(roots)
}

fn resolve_library_dependencies(
    vfsd: Channel,
    registry: &MemoryAppRegistry,
    manifest: &Manifest,
) -> Result<Vec<ResolvedLibraryDependency>, fs_fidl::FsStatus> {
    resolve_library_dependencies_with_dirs(Some(vfsd), registry, manifest)
}

fn resolve_library_dependencies_for_migration(
    registry: &MemoryAppRegistry,
    manifest: &Manifest,
) -> Result<Vec<ResolvedLibraryDependency>, fs_fidl::FsStatus> {
    resolve_library_dependencies_with_dirs(None, registry, manifest)
}

fn resolve_library_dependencies_with_dirs(
    vfsd: Option<Channel>,
    registry: &MemoryAppRegistry,
    manifest: &Manifest,
) -> Result<Vec<ResolvedLibraryDependency>, fs_fidl::FsStatus> {
    let mut roots = Vec::new();
    let mut visiting = Vec::new();
    for dependency in &manifest.library_dependencies {
        if let Err(status) = resolve_library_dependency_recursive(
            vfsd,
            registry,
            dependency,
            dependency.mount_alias.clone(),
            &mut visiting,
            &mut roots,
        ) {
            close_dependency_roots(roots);
            return Err(status);
        }
    }
    Ok(roots)
}

fn resolve_library_dependency_recursive(
    vfsd: Option<Channel>,
    registry: &MemoryAppRegistry,
    dependency: &crate::manifest::LibraryDependency,
    mount_alias: Option<String>,
    visiting: &mut Vec<(String, u32)>,
    roots: &mut Vec<ResolvedLibraryDependency>,
) -> Result<(), fs_fidl::FsStatus> {
    if dependency.abi_version == 0 {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    let selector = parse_package_selector(
        &dependency.package_name,
        dependency.version_requirement.as_deref(),
    )
    .map_err(|_| fs_fidl::FsStatus::InvalidArgs)?;
    let lookup = match selector.version_requirement.as_deref() {
        Some(version) => alloc::format!("{}:{version}", selector.package),
        None => selector.package.clone(),
    };
    let record = registry
        .record(&lookup)
        .map_err(|_| fs_fidl::FsStatus::NotFound)?;
    let dependency_manifest =
        Manifest::decode(&record.manifest_bytes).map_err(|_| fs_fidl::FsStatus::InvalidArgs)?;
    if dependency_manifest.package_kind != crate::manifest::PackageKind::Library {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    let export = dependency_manifest
        .library_exports
        .iter()
        .find(|export| {
            export.abi_version == dependency.abi_version
                && !export.path.is_empty()
                && (export.kind == LibraryExportKind::WasmComponent
                    || (!export.symbol_prefix.is_empty() && !export.soname.is_empty()))
        })
        .ok_or(fs_fidl::FsStatus::InvalidArgs)?;
    let key = (record.package_id.clone(), export.abi_version);
    if roots.iter().any(|root| {
        root.package_name == key.0
            && root.export_name == export.name
            && root.abi_version == export.abi_version
    }) {
        return Ok(());
    }
    if visiting.contains(&key) {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    visiting.push(key);
    for child in &dependency_manifest.library_dependencies {
        resolve_library_dependency_recursive(vfsd, registry, child, None, visiting, roots)?;
    }
    let _ = visiting.pop();
    // Finish fallible metadata validation before acquiring a directory handle.
    // On an error the caller can then close every acquired root in `roots`.
    let direct_dependencies =
        library_dependency_metadata(registry, &dependency_manifest.library_dependencies)?;
    let directory = match vfsd {
        Some(vfsd) => vfs::get_package_directory(vfsd, &record.archive_id())?,
        None => Channel(0),
    };
    roots.push(ResolvedLibraryDependency {
        package_name: record.package_id.clone(),
        export_name: export.name.clone(),
        mount_alias,
        export_path: export.path.clone(),
        symbol_prefix: export.symbol_prefix.clone(),
        abi_version: export.abi_version,
        soname: export.soname.clone(),
        kind: match export.kind {
            LibraryExportKind::Native => PackageLibraryKind::Native,
            LibraryExportKind::WasmComponent => PackageLibraryKind::WasmComponent,
            LibraryExportKind::Unspecified => return Err(fs_fidl::FsStatus::InvalidArgs),
        },
        direct_dependencies,
        archive_id: record.archive_id(),
        directory,
    });
    Ok(())
}

fn library_dependency_metadata(
    registry: &MemoryAppRegistry,
    dependencies: &[crate::manifest::LibraryDependency],
) -> Result<Vec<crate::PackageLibraryDependency>, fs_fidl::FsStatus> {
    let mut out = Vec::new();
    for dependency in dependencies {
        if dependency.abi_version == 0 {
            return Err(fs_fidl::FsStatus::InvalidArgs);
        }
        let selector = parse_package_selector(
            &dependency.package_name,
            dependency.version_requirement.as_deref(),
        )
        .map_err(|_| fs_fidl::FsStatus::InvalidArgs)?;
        let lookup = match selector.version_requirement.as_deref() {
            Some(version) => alloc::format!("{}:{version}", selector.package),
            None => selector.package.clone(),
        };
        let record = registry
            .record(&lookup)
            .map_err(|_| fs_fidl::FsStatus::NotFound)?;
        let manifest =
            Manifest::decode(&record.manifest_bytes).map_err(|_| fs_fidl::FsStatus::InvalidArgs)?;
        let export = manifest
            .library_exports
            .iter()
            .find(|export| {
                export.abi_version == dependency.abi_version
                    && !export.path.is_empty()
                    && (export.kind == LibraryExportKind::WasmComponent
                        || (!export.symbol_prefix.is_empty() && !export.soname.is_empty()))
            })
            .ok_or(fs_fidl::FsStatus::InvalidArgs)?;
        out.push(crate::PackageLibraryDependency {
            package_name: record.package_id.clone(),
            abi_version: export.abi_version,
            soname: export.soname.clone(),
        });
    }
    Ok(out)
}

fn close_dependency_roots(roots: Vec<ResolvedLibraryDependency>) {
    for root in roots {
        if root.directory.0 != 0 {
            let _ = Memory::close(root.directory.0);
        }
    }
}

fn close_shared_vault_roots(roots: Vec<ResolvedSharedVault>) {
    for root in roots {
        let _ = Memory::close(root.directory.0);
    }
}

fn provider_binding_metadata(binding: &BoundCapability) -> Vec<u8> {
    let mut metadata = String::new();
    metadata.push_str(&binding.service_name);
    metadata.push('|');
    metadata.push_str(&binding.protocol);
    metadata.push('|');
    metadata.push_str(&binding.capability);
    metadata.push('|');
    for (index, ordinal) in binding.method_ordinals.iter().enumerate() {
        if index != 0 {
            metadata.push(',');
        }
        metadata.push_str(&ordinal.to_string());
    }
    metadata.push('|');
    for (index, value) in binding.permission_values.iter().enumerate() {
        if index != 0 {
            metadata.push(',');
        }
        metadata.push_str(value);
    }
    metadata.push('|');
    metadata.push_str(&binding.caller_package);
    metadata.push('|');
    if let Some(uid) = binding.caller_uid {
        metadata.push_str(&uid.to_string());
    }
    metadata.push('|');
    metadata.push_str(if binding.caller_foreground {
        "fg"
    } else {
        "bg"
    });
    metadata.push('|');
    if let Some(instance_id) = &binding.provider_instance_id {
        metadata.push_str(instance_id);
    }
    metadata.into_bytes()
}

fn install_lifecycle_bundle(
    registry: &mut MemoryAppRegistry,
    permissions: &mut MemoryPermissionStore,
    openers: &mut MemoryOpenerRegistry,
    vfsd: Channel,
    teed: Option<Channel>,
    request: lifecycle::AppLifecycleControlInstallBundleRequest,
) -> (lifecycle::AppLifecycleStatus, String) {
    let rounded = match bexos_boot::page_round(request.archive_len) {
        Some(rounded) => rounded,
        None => return (lifecycle::AppLifecycleStatus::InvalidArgs, String::new()),
    };
    let va = match Memory::map(request.archive.raw, rounded, 2) {
        Ok(va) => va,
        Err(_) => return (lifecycle::AppLifecycleStatus::AccessDenied, String::new()),
    };
    let archive =
        unsafe { core::slice::from_raw_parts(va as *const u8, request.archive_len as usize) };
    let archive_bytes = archive.to_vec();
    let result = install_archive_bytes(
        registry,
        permissions,
        openers,
        vfsd,
        teed,
        &archive_bytes,
        InstallSource::Debugd,
        "pkg/debugd-upload.bex",
    );
    let _ = Memory::unmap(va, rounded);
    let _ = Memory::close(request.archive.raw);
    result
}

fn read_config_vmo(handle: u64, len: u64) -> Result<Vec<u8>, lifecycle::ComponentConfigStatus> {
    if handle == 0 || len == 0 || len as usize > MAX_CONFIG_SNAPSHOT_LEN {
        let _ = Memory::close(handle);
        return Err(lifecycle::ComponentConfigStatus::InvalidArgs);
    }
    let Some(rounded) = bexos_boot::page_round(len) else {
        let _ = Memory::close(handle);
        return Err(lifecycle::ComponentConfigStatus::InvalidArgs);
    };
    let va = match Memory::map(handle, rounded, 2) {
        Ok(va) => va,
        Err(_) => {
            let _ = Memory::close(handle);
            return Err(lifecycle::ComponentConfigStatus::Storage);
        }
    };
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) }.to_vec();
    let _ = Memory::unmap(va, rounded);
    let _ = Memory::close(handle);
    Ok(bytes)
}

fn trusted_app_payload(manifest: &Manifest, archive_bytes: &[u8]) -> Result<Vec<u8>, ()> {
    let info = manifest.trusted_app.as_ref().ok_or(())?;
    let archive = bexos_app_archive::OpenArchive::parse(archive_bytes).map_err(|_| ())?;
    let path = info
        .archive_payload_path
        .strip_prefix("/pkg/")
        .unwrap_or(&info.archive_payload_path);
    let entry = archive.find(path).ok_or(())?;
    archive.read_file(entry).map_err(|_| ())
}

fn bind_teed_manager(
    teed: Option<Channel>,
    ordinals: &[u64],
) -> Result<Channel, lifecycle::AppLifecycleStatus> {
    let teed = teed.ok_or(lifecycle::AppLifecycleStatus::NotFound)?;
    let (client, provider) = Channel::pair().map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
    let mut metadata = String::from("tee_manager|TeeManager|Public|");
    for (index, ordinal) in ordinals.iter().enumerate() {
        if index != 0 {
            metadata.push(',');
        }
        metadata.push_str(&ordinal.to_string());
    }
    metadata.push('|');
    teed.send(metadata.as_bytes(), &[provider.0])
        .map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
    Ok(client)
}

fn activate_trusted_app_package(
    teed: Option<Channel>,
    manifest: &Manifest,
    package_id: &str,
    payload: &[u8],
) -> Result<(), lifecycle::AppLifecycleStatus> {
    let info = manifest
        .trusted_app
        .as_ref()
        .ok_or(lifecycle::AppLifecycleStatus::VerifyFailed)?;
    let handle = Memory::from_bytes(payload).map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
    let ports: Vec<_> = info
        .allowed_service_ports
        .iter()
        .map(|port| port.as_str())
        .collect();
    let request = TeeManagerActivateTrustedAppPackageRequest {
        package_id,
        provider: &info.provider,
        uuid: info.uuid,
        secure_version: info.secure_version,
        archive_payload: tee_manager::HandleRef { raw: handle },
        archive_payload_len: payload.len() as u64,
        service_ports: TeeWireStringVector::from_slice(&ports),
        protected: info.protected,
        uninstallable: info.uninstallable,
    };
    let (bytes, handles) = call_teed_raw(teed, 11, &request, &[handle])?;
    let response = TeeManagerActivateTrustedAppPackageResponse::decode(&bytes, &handles)
        .map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
    if response.status == TeeStatus::Ok {
        Ok(())
    } else {
        Err(lifecycle_status_from_tee(response.status))
    }
}

fn query_trusted_app_package(
    teed: Option<Channel>,
    package_id: &str,
) -> Result<u32, lifecycle::AppLifecycleStatus> {
    let request = TeeManagerQueryTrustedAppPackageRequest { package_id };
    let (bytes, handles) = call_teed_raw(teed, 13, &request, &[])?;
    let response = TeeManagerQueryTrustedAppPackageResponse::decode(&bytes, &handles)
        .map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
    if response.status == TeeStatus::Ok {
        Ok(response.active_sessions)
    } else {
        Err(lifecycle_status_from_tee(response.status))
    }
}

fn deactivate_trusted_app_package(
    teed: Option<Channel>,
    package_id: &str,
) -> Result<(), lifecycle::AppLifecycleStatus> {
    let request = TeeManagerDeactivateTrustedAppPackageRequest { package_id };
    let (bytes, handles) = call_teed_raw(teed, 12, &request, &[])?;
    let response = TeeManagerDeactivateTrustedAppPackageResponse::decode(&bytes, &handles)
        .map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
    if response.status == TeeStatus::Ok {
        Ok(())
    } else {
        Err(lifecycle_status_from_tee(response.status))
    }
}

fn call_teed_raw<Q: TeeEncode>(
    teed: Option<Channel>,
    ordinal: u64,
    request: &Q,
    handles: &[u64],
) -> Result<(Vec<u8>, Vec<TeeHandleRef>), lifecycle::AppLifecycleStatus> {
    let endpoint = bind_teed_manager(teed, &[ordinal])?;
    let result = call_teed_bound(endpoint, ordinal, request, handles);
    let _ = Memory::close(endpoint.0);
    result
}

fn call_teed_bound<Q: TeeEncode>(
    endpoint: Channel,
    ordinal: u64,
    request: &Q,
    handles: &[u64],
) -> Result<(Vec<u8>, Vec<TeeHandleRef>), lifecycle::AppLifecycleStatus> {
    let mut bytes = alloc::vec![0; 8192];
    let mut refs = [tee_manager::HandleRef { raw: 0 }; 16];
    let encoded = request
        .encode(&mut bytes, &mut refs)
        .map_err(|_| lifecycle::AppLifecycleStatus::InvalidArgs)?;
    let message = Rpc(endpoint)
        .call_raw(ordinal, &bytes[..encoded.bytes], handles, true)
        .map_err(|_| lifecycle::AppLifecycleStatus::Storage)?;
    let response_refs: Vec<_> = message
        .handles
        .iter()
        .map(|raw| TeeHandleRef { raw: *raw })
        .collect();
    Ok((message.bytes, response_refs))
}

fn lifecycle_status_from_tee(status: TeeStatus) -> lifecycle::AppLifecycleStatus {
    match status {
        TeeStatus::Ok => lifecycle::AppLifecycleStatus::Ok,
        TeeStatus::ErrInvalidArgs => lifecycle::AppLifecycleStatus::InvalidArgs,
        TeeStatus::ErrAccessDenied => lifecycle::AppLifecycleStatus::AccessDenied,
        TeeStatus::ErrNotFound => lifecycle::AppLifecycleStatus::NotFound,
        TeeStatus::ErrVerifyFailed => lifecycle::AppLifecycleStatus::VerifyFailed,
        _ => lifecycle::AppLifecycleStatus::Storage,
    }
}

pub(crate) fn install_archive_bytes(
    registry: &mut MemoryAppRegistry,
    permissions: &mut MemoryPermissionStore,
    openers: &mut MemoryOpenerRegistry,
    vfsd: Channel,
    teed: Option<Channel>,
    archive_bytes: &[u8],
    source: InstallSource,
    archive_path: &str,
) -> (lifecycle::AppLifecycleStatus, String) {
    let trusted = [bexos_app_archive::TrustedKey {
        key_id: *b"bexos-qemu-test-ed25519-key-v001",
        public_key: &[
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ],
    }];
    let request = bexos_app_registry::InstallRequest {
        archive_bytes,
        trusted_keys: &trusted,
        verified_signer: Some(bexos_app_registry::VerifiedSignerMetadata {
            root_anchor_id: "bexos-dev-app-root".into(),
            leaf_certificate_fingerprint: blake3::hash(trusted[0].public_key).into(),
            signature_algorithm: 1,
            granted_trust_tier: 1,
        }),
        source,
        protected: false,
        archive_path,
    };
    log("appd: install archive verification begin\n");
    let parsed = bexos_app_registry::record_from_bundle(request.clone());
    let Ok(parsed) = parsed else {
        return (lifecycle::AppLifecycleStatus::VerifyFailed, String::new());
    };
    let Ok(manifest) = Manifest::decode(&parsed.manifest_bytes) else {
        return (lifecycle::AppLifecycleStatus::VerifyFailed, String::new());
    };
    if manifest.validate_package_shape().is_err() {
        return (lifecycle::AppLifecycleStatus::VerifyFailed, String::new());
    }
    let trusted_payload = if manifest.package_kind == crate::PackageKind::TrustedApp {
        match trusted_app_payload(&manifest, archive_bytes) {
            Ok(payload) => Some(payload),
            Err(_) => return (lifecycle::AppLifecycleStatus::VerifyFailed, String::new()),
        }
    } else {
        None
    };
    // The caller's path describes the upload/staging source. Publish the path
    // actually written below so launch and persistent reopen use the same file.
    let stored_archive_path = alloc::format!("pkg/{}.bex", parsed.package_key());
    let request = bexos_app_registry::InstallRequest {
        archive_path: &stored_archive_path,
        ..request
    };
    log("appd: install archive verified; persisting payload\n");
    if vfs::write_package_archive(vfsd, &parsed.package_key(), archive_bytes).is_err() {
        log("appd: package archive write failed; install rejected\n");
        return (lifecycle::AppLifecycleStatus::Storage, String::new());
    }
    log("appd: install payload persisted; publishing registry record\n");
    let record = match registry.install_bundle(request) {
        Ok(record) => record,
        Err(_) => {
            let _ = vfs::delete_package_archive(vfsd, &parsed.package_key());
            return (lifecycle::AppLifecycleStatus::VerifyFailed, String::new());
        }
    };
    if let Some(payload) = trusted_payload.as_deref() {
        match activate_trusted_app_package(teed, &manifest, &record.package_key(), payload) {
            Ok(()) => {}
            Err(status) => {
                let _ = registry.uninstall_package(&record.package_key());
                let _ = vfs::delete_package_archive(vfsd, &parsed.package_key());
                return (status, String::new());
            }
        }
    }
    if let Ok(manifest) = Manifest::decode(&record.manifest_bytes) {
        let _ = permissions.register_system_declarations(
            &manifest.package_name,
            &manifest.permissions,
            0,
        );
        register_manifest_openers(openers, OpenerScope::System, &manifest, false);
    }
    (lifecycle::AppLifecycleStatus::Ok, record.package_key())
}

fn uninstall_lifecycle_app(
    registry: &mut MemoryAppRegistry,
    permissions: &mut MemoryPermissionStore,
    openers: &mut MemoryOpenerRegistry,
    vfsd: Channel,
    teed: Option<Channel>,
    package_id: &str,
) -> lifecycle::AppLifecycleStatus {
    let record = match registry.record_for_removal(package_id).cloned() {
        Ok(record) => record,
        Err(bexos_app_registry::RegistryError::NotFound) => {
            return lifecycle::AppLifecycleStatus::NotFound;
        }
        Err(_) => return lifecycle::AppLifecycleStatus::InvalidArgs,
    };
    if record.protected {
        return lifecycle::AppLifecycleStatus::AccessDenied;
    }
    if let Ok(manifest) = Manifest::decode(&record.manifest_bytes) {
        if manifest.package_kind == crate::PackageKind::TrustedApp {
            match query_trusted_app_package(teed, package_id) {
                Ok(active_sessions) if active_sessions != 0 => {
                    return lifecycle::AppLifecycleStatus::AccessDenied;
                }
                Ok(_) => {}
                Err(status) => return status,
            }
            if let Err(status) = deactivate_trusted_app_package(teed, package_id) {
                return status;
            }
        }
    }
    match vfs::delete_package_archive(vfsd, &record.package_key()) {
        Ok(()) | Err(fs_fidl::FsStatus::NotFound) => {}
        Err(status) => {
            log(&format!(
                "appd: uninstall archive delete failed package={} path={} status={:?}\n",
                record.package_key(),
                record.archive_path,
                status
            ));
            return lifecycle::AppLifecycleStatus::Storage;
        }
    }
    match registry.uninstall_package(package_id) {
        Ok(record) => {
            permissions.remove_package(&record.package_id);
            openers.remove_package(&record.package_id);
            lifecycle::AppLifecycleStatus::Ok
        }
        Err(bexos_app_registry::RegistryError::ProtectedPackage) => {
            lifecycle::AppLifecycleStatus::AccessDenied
        }
        Err(bexos_app_registry::RegistryError::NotFound) => lifecycle::AppLifecycleStatus::NotFound,
        Err(_) => lifecycle::AppLifecycleStatus::InvalidArgs,
    }
}

fn launch_lifecycle_app(
    registry: &mut MemoryAppRegistry,
    launches: &mut Vec<state::LaunchRecord>,
    services: &mut Vec<state::ManagedService>,
    vfsd: Channel,
    users: Channel,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    broker: &mut AppdBroker,
    permission_routes: &mut crate::PermissionRouteTable,
    permissions: &mut MemoryPermissionStore,
    opener_bindings: &mut Vec<OpenerBinding>,
    version_manager_bindings: &mut Vec<u64>,
    app_manager_bindings: &mut Vec<crate::AppManagerBinding>,
    worker_launcher_bindings: &mut Vec<crate::AppManagerBinding>,
    service_directory_bindings: &mut Vec<crate::ServiceDirectoryBinding>,
    lazy: &mut crate::lazy::LazyActivationState,
    domain_associations: &bexos_domain_association::MemoryDomainAssociationCache,
    config: &PlatformConfig,
    component_configs: &[state::ComponentConfigRecord],
    package_id: &str,
    process_name: &str,
    arg0: u64,
    uid: u64,
    job_control: Option<u64>,
    require_job_target: bool,
) -> lifecycle::AppLifecycleStatus {
    launch_application(
        registry,
        launches,
        services,
        vfsd,
        users,
        kernel,
        broker,
        permission_routes,
        permissions,
        opener_bindings,
        version_manager_bindings,
        app_manager_bindings,
        worker_launcher_bindings,
        service_directory_bindings,
        lazy,
        domain_associations,
        config,
        component_configs,
        package_id,
        process_name,
        arg0,
        uid,
        job_control,
        require_job_target,
        None,
        None,
        Vec::new(),
        0,
        false,
        None,
    )
}

fn launch_application(
    registry: &mut MemoryAppRegistry,
    launches: &mut Vec<state::LaunchRecord>,
    services: &mut Vec<state::ManagedService>,
    vfsd: Channel,
    users: Channel,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    broker: &mut AppdBroker,
    permission_routes: &mut crate::PermissionRouteTable,
    permissions: &mut MemoryPermissionStore,
    opener_bindings: &mut Vec<OpenerBinding>,
    version_manager_bindings: &mut Vec<u64>,
    app_manager_bindings: &mut Vec<crate::AppManagerBinding>,
    worker_launcher_bindings: &mut Vec<crate::AppManagerBinding>,
    service_directory_bindings: &mut Vec<crate::ServiceDirectoryBinding>,
    lazy: &mut crate::lazy::LazyActivationState,
    domain_associations: &bexos_domain_association::MemoryDomainAssociationCache,
    config: &PlatformConfig,
    component_configs: &[state::ComponentConfigRecord],
    package_id: &str,
    process_name: &str,
    arg0: u64,
    uid: u64,
    job_control: Option<u64>,
    require_job_target: bool,
    instance_id: Option<&str>,
    command: Option<&command_runtime::CommandLaunch>,
    initial_incoming_bindings: Vec<BoundCapability>,
    lazy_idle_timeout_ms: u32,
    internal_shell: bool,
    container: Option<&command_runtime::ContainerLaunch>,
) -> lifecycle::AppLifecycleStatus {
    log(&alloc::format!(
        "appd: launch request package={package_id} uid={uid}\n"
    ));
    let Ok(record) = registry.record(package_id).cloned() else {
        return lifecycle::AppLifecycleStatus::NotFound;
    };
    if registry
        .mark_lifecycle(&record.package_key(), LifecycleState::Launching)
        .is_err()
    {
        log(&alloc::format!(
            "appd: lifecycle mark launching failed package={package_id} key={}\n",
            record.package_key()
        ));
        return lifecycle::AppLifecycleStatus::Storage;
    }
    let mut pending_locale =
        match locale::prepare(registry, services, vfsd, component_configs, package_id, uid) {
            Ok(locale) => locale,
            Err(error) => {
                log(&format!("appd: locale startup failed {error}\n"));
                mark_launch_failed(registry, &record);
                return lifecycle::AppLifecycleStatus::LaunchFailed;
            }
        };
    let archive = services
        .iter()
        .find(|s| s.package == package_id && s.archive != 0);
    let directory = if let Some(selected) = archive {
        let manager = services
            .iter()
            .find(|s| s.package == "bexos.driver.storage.archivefs")
            .map(|s| Channel(s.manager));
        manager
            .ok_or(fs_fidl::FsStatus::NotFound)
            .and_then(|manager| {
                fs::mount_archive_memory(manager, selected.archive, selected.archive_len)
            })
    } else {
        vfs::get_package_directory(vfsd, &record.archive_id())
    };
    let archive_root = match directory {
        Ok(root) => root,
        Err(error) => {
            log(&alloc::format!(
                "appd: package directory failed package={package_id} key={} error={error:?}\n",
                record.package_key()
            ));
            return lifecycle::AppLifecycleStatus::Storage;
        }
    };
    let mut manifest = match Manifest::decode(&record.manifest_bytes) {
        Ok(manifest) => manifest,
        Err(_) => {
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::InvalidArgs;
        }
    };
    if let Some(container) = container {
        let Some(process) = manifest
            .processes
            .iter_mut()
            .find(|process| process.name == process_name)
        else {
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::NotFound;
        };
        process.runner = "nix".into();
        process.runner_options = Some(crate::ProcessRunnerOptions::Nix(container.options.clone()));
        process.service = false;
    }
    if let Some(command) = command {
        if let Some(process) = manifest
            .processes
            .iter_mut()
            .find(|p| p.name == process_name)
        {
            if let Some(crate::ProcessRunnerOptions::Wasm(options)) = &mut process.runner_options {
                options.arguments = command.options.arguments.clone();
                options.environment = command
                    .options
                    .environment
                    .iter()
                    .map(|(name, value)| bexos_wasm_abi::Environment {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect();
            }
        }
    }
    let Some(process) = manifest
        .processes
        .iter()
        .find(|process| process.name == process_name)
    else {
        let _ = Memory::close(archive_root.0);
        return lifecycle::AppLifecycleStatus::NotFound;
    };
    if process.shell_role != crate::manifest::ShellRole::None && !internal_shell {
        let _ = Memory::close(archive_root.0);
        return lifecycle::AppLifecycleStatus::AccessDenied;
    }
    if require_job_target
        && !manifest
            .jobs
            .iter()
            .any(|job| job.target_component == process.name)
    {
        let _ = Memory::close(archive_root.0);
        return lifecycle::AppLifecycleStatus::AccessDenied;
    }
    if uid != SYSTEM_UID && !user_is_unlocked(users, uid) {
        let _ = Memory::close(archive_root.0);
        return lifecycle::AppLifecycleStatus::AccessDenied;
    }
    if uid != SYSTEM_UID {
        match crate::permission_persistence::load_user(vfsd, uid, registry, permissions) {
            Ok(()) => {}
            Err(crate::permission_persistence::PermissionPersistenceError::Locked) => {
                let _ = Memory::close(archive_root.0);
                return lifecycle::AppLifecycleStatus::AccessDenied;
            }
            Err(_) => {
                let _ = Memory::close(archive_root.0);
                return lifecycle::AppLifecycleStatus::Storage;
            }
        }
    }
    let declarations = effective_permission_declarations(&manifest, process);
    if let Err(error) = crate::permission_persistence::auto_grant_and_sync(
        vfsd,
        uid,
        &manifest.package_name,
        &declarations,
        permissions,
    ) {
        let _ = Memory::close(archive_root.0);
        return match error {
            crate::permission_persistence::PermissionPersistenceError::Store(_) => {
                lifecycle::AppLifecycleStatus::AccessDenied
            }
            _ => lifecycle::AppLifecycleStatus::Storage,
        };
    }
    let client = client_context_from_grants(&manifest.package_name, uid, permissions);
    let requested_network_domain = process
        .network_domain
        .as_deref()
        .unwrap_or("system_default");
    if config
        .network_policy
        .authorize_domain(&manifest.package_name, requested_network_domain)
        .is_none()
    {
        log(&alloc::format!(
            "appd: network domain denied package={package_id} process={process_name} domain={requested_network_domain}\n"
        ));
        let _ = Memory::close(archive_root.0);
        return lifecycle::AppLifecycleStatus::AccessDenied;
    }
    let mut bound_capabilities = Vec::new();
    for consumed in &manifest.services_consumed {
        let mut scoped = consumed.clone();
        if let Some(instance_id) = instance_id {
            if matches!(
                scoped.name.as_str(),
                "bexos.net.NetstackBackend"
                    | "bexos.net.StackBackend"
                    | "bexos.net.StackController"
            ) {
                let expected_filter = alloc::format!("network.instance == '{instance_id}'");
                if scoped
                    .filter
                    .as_deref()
                    .is_some_and(|filter| filter != expected_filter)
                {
                    close_bound_capabilities(&bound_capabilities);
                    let _ = Memory::close(archive_root.0);
                    return lifecycle::AppLifecycleStatus::AccessDenied;
                }
                scoped.filter = Some(expected_filter);
            }
        }
        if matches!(
            scoped.name.as_str(),
            "bexos.net.SocketProvider" | "bexos.net.Netstack"
        ) {
            let domain = if scoped.name == "bexos.net.Netstack" {
                if requested_network_domain != "system_default" {
                    close_bound_capabilities(&bound_capabilities);
                    let _ = Memory::close(archive_root.0);
                    return lifecycle::AppLifecycleStatus::AccessDenied;
                }
                "system_default"
            } else {
                requested_network_domain
            };
            let expected_filter = alloc::format!("network.domain == '{domain}'");
            if scoped
                .filter
                .as_deref()
                .is_some_and(|filter| filter != expected_filter)
            {
                close_bound_capabilities(&bound_capabilities);
                let _ = Memory::close(archive_root.0);
                return lifecycle::AppLifecycleStatus::AccessDenied;
            }
            scoped.filter = Some(expected_filter);
        }
        match broker.bind_consumed_service(&client, &scoped, kernel) {
            Ok(mut bindings) => bound_capabilities.append(&mut bindings),
            Err(_) if consumed.link_type == LinkType::Optional => {}
            Err(error) => {
                log(&alloc::format!(
                    "appd: consumed service bind denied package={package_id} process={process_name} service={} error={error:?}\n",
                    consumed.name
                ));
                close_bound_capabilities(&bound_capabilities);
                let _ = Memory::close(archive_root.0);
                return lifecycle::AppLifecycleStatus::AccessDenied;
            }
        }
    }
    for binding in &mut bound_capabilities {
        preferences::bind_identity(binding, &record);
        if binding.service_name == "bexos.net.SocketProvider" {
            binding
                .permission_values
                .push(alloc::format!("network.domain={requested_network_domain}"));
        } else if binding.service_name == "bexos.net.Netstack" {
            binding
                .permission_values
                .push("network.domain=system_default".to_string());
        }
        if binding.service_name == "bexos.ui.scened.FlatlandSession" {
            crate::shell::stamp_grant(
                &mut binding.permission_values,
                process.shell_role,
                internal_shell.then_some(arg0),
            );
        }
    }
    let lazy_status = activate_lazy_bindings(
        registry,
        launches,
        services,
        vfsd,
        users,
        kernel,
        broker,
        permission_routes,
        permissions,
        opener_bindings,
        version_manager_bindings,
        app_manager_bindings,
        worker_launcher_bindings,
        service_directory_bindings,
        lazy,
        domain_associations,
        config,
        component_configs,
        &bound_capabilities,
    );
    if lazy_status != lifecycle::AppLifecycleStatus::Ok {
        close_bound_capabilities(&bound_capabilities);
        let _ = Memory::close(archive_root.0);
        return lazy_status;
    }
    let dependency_roots = match resolve_library_dependencies(vfsd, registry, &manifest) {
        Ok(dependencies) => dependencies,
        Err(_) => {
            log(&alloc::format!(
                "appd: dependency resolution failed package={package_id} key={}\n",
                record.package_key()
            ));
            close_bound_capabilities(&bound_capabilities);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::Storage;
        }
    };
    let shared_vault_roots = match resolve_shared_vaults(
        vfsd,
        registry,
        domain_associations,
        &record.package_id,
        uid,
        &manifest,
    ) {
        Ok(vaults) => vaults,
        Err(_) => {
            close_bound_capabilities(&bound_capabilities);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::AccessDenied;
        }
    };
    let sandboxed = matches!(
        process.runner_options,
        Some(crate::ProcessRunnerOptions::Wasm(_) | crate::ProcessRunnerOptions::Nix(_))
    );
    let signer = if sandboxed {
        match &record.verified_signer {
            Some(signer) => signer.root_anchor_id.as_str(),
            None => {
                close_bound_capabilities(&bound_capabilities);
                close_shared_vault_roots(shared_vault_roots);
                close_dependency_roots(dependency_roots);
                let _ = Memory::close(archive_root.0);
                return lifecycle::AppLifecycleStatus::AccessDenied;
            }
        }
    } else {
        "bexos_official_platform_v1"
    };
    let trust_tier = if sandboxed {
        PackageTrustTier::StandardConsumer
    } else {
        PackageTrustTier::PlatformCore
    };
    let disk_resolver = match resolver::Resolver::disk_with_dependencies(
        archive_root,
        &manifest,
        &dependency_roots,
        vfsd,
        registry,
        container.map(|container| (process_name, Channel(container.rootfs))),
    ) {
        Ok(resolver) => resolver,
        Err(error) => {
            log(&alloc::format!(
                "appd: package image resolution failed package={package_id} process={process_name} error={error:?}\n"
            ));
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::Storage;
        }
    };
    let mut network_resource_job = match create_network_instance_resource_group(
        config,
        services,
        kernel,
        package_id,
        process_name,
        instance_id,
    ) {
        Ok(job) => job,
        Err(status) => {
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return status;
        }
    };
    let resource_group_id = container.map_or_else(
        || network_resource_job.as_ref().map_or(1, |job| job.id),
        |container| container.resource_group_id,
    );
    let launched = RunnerRegistry::new().launch(
        &LaunchRequest {
            manifest: &manifest,
            process,
            trust_tier,
            identity: PackageIdentity {
                package_id: &manifest.package_name,
                signer,
                trust_tier,
                is_driver: false,
            },
            runner_policy: Some(&config.runner_policy),
            hardware_access: HardwareAccessTier::None,
            realtime_scheduling: crate::runner::realtime_scheduling_for(
                &manifest,
                process,
                PackageIdentity {
                    package_id: &manifest.package_name,
                    signer,
                    trust_tier,
                    is_driver: false,
                },
                Some(&config.runner_policy),
            ),
            resource_group_id,
        },
        kernel,
        &disk_resolver,
    );
    drop(disk_resolver);
    let launched = match launched {
        Ok(launched) => launched,
        Err(error) => {
            log(&alloc::format!(
                "appd: launch failed package={package_id} process={process_name} error={error:?}\n"
            ));
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
    };
    let mut launch_migration = match launch_migration::LaunchMigration::new(
        process.service && (sandboxed || uid == SYSTEM_UID),
    ) {
        Ok(migration) => migration,
        Err(_) => {
            let _ =
                crate::runner::KernelOps::terminate_process(kernel, launched.process_handle, -1);
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
    };
    let mut startup_guard = crate::runner::PendingWasmLaunch::new(kernel, Some(launched));
    let control = Channel(launched.service_manager_handle.raw);
    let pkg_handle = match Memory::duplicate(archive_root.0, 1 | 2 | 4 | 32) {
        Ok(handle) => handle,
        Err(error) => {
            log(&alloc::format!(
                "appd: pkg handle duplicate failed package={package_id} process={process_name} error={error:?}\n"
            ));
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
    };
    let data_package = match record.multi_version_policy {
        crate::MultiVersionPolicy::ParallelExecution => record.package_key(),
        crate::MultiVersionPolicy::SharedStorageMulti => {
            alloc::format!("{}:shared", record.package_id)
        }
        _ => record.package_id.clone(),
    };
    let selected_data_root = if let Some(container) = container {
        Memory::object_info(container.rootfs)
            .and_then(|(_, rights)| Memory::duplicate(container.rootfs, rights))
            .map(Channel)
            .map_err(|_| fs_fidl::FsStatus::AccessDenied)
    } else if uid == SYSTEM_UID {
        vfs::get_system_data_directory(vfsd, &data_package)
    } else {
        vfs::get_user_data_directory(vfsd, uid, &data_package)
    };
    let data_root = match selected_data_root {
        Ok(root) => root,
        Err(error) => {
            log(&alloc::format!(
                "appd: data directory failed package={package_id} process={process_name} error={error:?}\n"
            ));
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            let _ = Memory::close(pkg_handle);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
    };
    let tmp_root = match vfs::get_tmp_directory(vfsd, uid, &record.package_id, process_name) {
        Ok(root) => root,
        Err(error) => {
            log(&alloc::format!(
                "appd: tmp directory failed package={package_id} process={process_name} error={error:?}\n"
            ));
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            let _ = Memory::close(pkg_handle);
            let _ = Memory::close(data_root.0);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
    };
    let namespace_shared_vaults = shared_vault_roots
        .iter()
        .map(|vault| SharedVaultNamespaceEntry {
            name: vault.name.as_str(),
            directory: KernelHandle {
                raw: vault.directory.0,
            },
        })
        .collect::<Vec<_>>();
    let namespace_dependencies = dependency_roots
        .iter()
        .map(|dependency| crate::DependencyNamespaceEntry {
            package_name: dependency.package_name.as_str(),
            mount_alias: dependency.mount_alias.as_deref(),
            directory: KernelHandle {
                raw: dependency.directory.0,
            },
        })
        .collect::<Vec<_>>();
    let namespace = match app_storage_namespace_with_shared_vaults_and_dependencies(
        KernelHandle { raw: pkg_handle },
        KernelHandle { raw: data_root.0 },
        KernelHandle { raw: tmp_root.0 },
        &namespace_shared_vaults,
        &namespace_dependencies,
    ) {
        Ok(namespace) => namespace,
        Err(error) => {
            log(&alloc::format!(
                "appd: namespace failed package={package_id} process={process_name} error={error:?}\n"
            ));
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            let _ = Memory::close(pkg_handle);
            let _ = Memory::close(data_root.0);
            let _ = Memory::close(tmp_root.0);
            close_shared_vault_roots(shared_vault_roots);
            close_dependency_roots(dependency_roots);
            let _ = Memory::close(archive_root.0);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
    };
    let mut namespace_entries = startup_namespace_entries(&namespace);
    if let Some(command) = command
        && let Some(cwd) = command.cwd
    {
        match Memory::object_info(cwd).and_then(|(_, rights)| Memory::duplicate(cwd, rights)) {
            Ok(directory) => namespace_entries.push(bexos_userspace::NamespaceEntry {
                path: "/cwd".into(),
                directory,
            }),
            Err(_) => {
                close_bound_capabilities(&bound_capabilities);
                return lifecycle::AppLifecycleStatus::AccessDenied;
            }
        }
    }
    let mut resources = Vec::new();
    let service_grants = startup_service_grants(&bound_capabilities, &manifest.package_name, uid);
    let incoming_service_grants = incoming_service_grants(&initial_incoming_bindings);
    if let Err(error) = notify_bound_providers(&bound_capabilities) {
        log(&alloc::format!(
            "appd: provider notify failed package={package_id} process={process_name} error={error:?}\n"
        ));
        mark_launch_failed(registry, &record);
        close_bound_capabilities(&bound_capabilities);
        close_shared_vault_roots(shared_vault_roots);
        return lifecycle::AppLifecycleStatus::LaunchFailed;
    }
    if package_id == "bexos.platform.storage_verify" && arg0 == 0xbeef {
        let Some(block) = services
            .iter()
            .find(|s| s.package == "bexos.driver.storage.nvme")
            .map(|s| s.manager)
        else {
            log("appd: storage verifier progress block missing nvme manager\n");
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            return lifecycle::AppLifecycleStatus::Storage;
        };
        let Ok(block) = Memory::duplicate(block, 1 | 2 | 4 | 32) else {
            log("appd: storage verifier progress block duplicate failed\n");
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        };
        resources.push(block);
    }
    let mut command_bytes_len = 0;
    if let Some(command) = command {
        let result = (|| -> Result<(), kernel_fidl::Status> {
            if !resources.is_empty() {
                return Err(kernel_fidl::Status::ErrInvalidArgs);
            }
            for raw in command.stdio {
                let (_, rights) = Memory::object_info(raw)?;
                resources.push(Memory::duplicate(raw, rights)?);
            }
            let bytes = command
                .options
                .encode()
                .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
            let vmo = Memory::from_bytes(&bytes)?;
            resources.push(vmo);
            command_bytes_len = bytes.len() as u64;
            Ok(())
        })();
        if result.is_err() {
            close_handles(&resources);
            close_bound_capabilities(&bound_capabilities);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
    }
    if let Some(job_control) = job_control {
        resources.push(job_control);
    }
    let (mut config_vmo, mut config_endpoint) =
        match preferences::launch(services, archive_root, &record, uid, component_configs) {
            Ok(config) => config,
            Err(error) => {
                log(&alloc::format!(
                    "appd: preference resolution failed {package_id}: {error:?}\n"
                ));
                close_handles(&resources);
                close_bound_capabilities(&bound_capabilities);
                for entry in &namespace_entries {
                    let _ = Memory::close(entry.directory);
                }
                let _ = Memory::close(archive_root.0);
                mark_launch_failed(registry, &record);
                return lifecycle::AppLifecycleStatus::LaunchFailed;
            }
        };
    if package_id == "bexos.service.networkd" {
        let Some(instance_id) = instance_id else {
            log("appd: networkd launch denied: missing isolation instance\n");
            close_bound_capabilities(&bound_capabilities);
            return lifecycle::AppLifecycleStatus::AccessDenied;
        };
        let Some(bytes) = networkd_config_snapshot(&manifest, config, instance_id) else {
            log(&alloc::format!(
                "appd: networkd launch denied: config snapshot failed instance={instance_id}\n"
            ));
            close_bound_capabilities(&bound_capabilities);
            return lifecycle::AppLifecycleStatus::AccessDenied;
        };
        if let Some((handle, _)) = config_vmo.take() {
            let _ = Memory::close(handle);
        }
        if let Some(endpoint) = config_endpoint.take() {
            let _ = Memory::close(endpoint.0);
        }
        let Ok(handle) = Memory::from_bytes(&bytes) else {
            close_bound_capabilities(&bound_capabilities);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        };
        config_vmo = Some((handle, bytes.len() as u64));
    }
    let linker_data = launched
        .runtime_linker_data
        .map(|(handle, len)| (handle.raw, len));
    let trace_producer = trace_registry::allocate_trace_producer(
        launched.process_handle.raw,
        launched.main_thread_handle.raw,
    )
    .ok();
    let mut pending_trace = trace_producer.as_ref().map(|producer| {
        trace_registry::PendingTraceProducer::from_allocation(package_id, producer)
    });
    if let Err(error) = Startup::send_lazy_provider_startup(
        control,
        &resources,
        if command.is_some() {
            command_bytes_len
        } else {
            arg0
        },
        if command.is_some() {
            bexos_userspace::command::STARTUP_MAGIC
        } else {
            0
        },
        &namespace_entries,
        launch_migration.server(),
        0,
        &service_grants,
        &incoming_service_grants,
        lazy_idle_timeout_ms,
        lazy.generation(package_id, process_name, uid),
        config_vmo,
        config_endpoint,
        linker_data,
        trace_producer.map(|producer| producer.startup),
        pending_locale.0.as_ref(),
    ) {
        log(&alloc::format!(
            "appd: startup send failed package={package_id} process={process_name} error={error:?}\n"
        ));
        for handle in resources {
            let _ = Memory::close(handle);
        }
        for entry in namespace_entries {
            let _ = Memory::close(entry.directory);
        }
        if let Some((config_vmo, _)) = config_vmo {
            let _ = Memory::close(config_vmo);
        }
        if let Some(endpoint) = config_endpoint {
            let _ = Memory::close(endpoint.0);
        }
        if let Some(pending_trace) = pending_trace.take() {
            let _ = Memory::close(pending_trace.buffer);
        }
        close_bound_capabilities(&bound_capabilities);
        let _ = Memory::close(archive_root.0);
        mark_launch_failed(registry, &record);
        return lifecycle::AppLifecycleStatus::LaunchFailed;
    }
    startup_guard.startup_sent();
    pending_locale.sent();
    launch_migration.sent();
    if let Some(pending_trace) = pending_trace.take() {
        trace_registry::register_now(
            services
                .iter()
                .find(|service| service.package == "bexos.service.traced")
                .map(|service| Channel(service.manager)),
            pending_trace,
        );
    }
    if let Err(error) = Startup::wait_ready(control) {
        log(&alloc::format!(
            "appd: startup ready failed package={package_id} process={process_name} error={error:?}\n"
        ));
        for handle in resources {
            let _ = Memory::close(handle);
        }
        for entry in namespace_entries {
            let _ = Memory::close(entry.directory);
        }
        if let Some((config_vmo, _)) = config_vmo {
            let _ = Memory::close(config_vmo);
        }
        close_bound_capabilities(&bound_capabilities);
        let _ = Memory::close(archive_root.0);
        mark_launch_failed(registry, &record);
        return lifecycle::AppLifecycleStatus::LaunchFailed;
    }
    register_opener_bindings(
        opener_bindings,
        &bound_capabilities,
        &manifest.package_name,
        uid,
    );
    register_version_manager_bindings(version_manager_bindings, &bound_capabilities);
    register_app_manager_bindings(
        app_manager_bindings,
        &bound_capabilities,
        &manifest.package_name,
        uid,
    );
    register_worker_launcher_bindings(
        worker_launcher_bindings,
        &bound_capabilities,
        &manifest.package_name,
        uid,
    );
    register_service_directory_bindings(
        service_directory_bindings,
        &bound_capabilities,
        &manifest.package_name,
        uid,
        internal_shell && process.shell_role != crate::manifest::ShellRole::None,
    );
    if process.service {
        if let Err(_) = publish_manifest_services_for_manager(
            broker,
            &manifest,
            control.0,
            instance_id,
            Some(config),
        ) {
            mark_launch_failed(registry, &record);
            close_bound_capabilities(&bound_capabilities);
            return lifecycle::AppLifecycleStatus::LaunchFailed;
        }
        if manifest.process_has_lazy_exposures(process_name) {
            let queued = lazy.running(package_id, process_name, uid);
            if deliver_bound_capabilities_to_manager(queued, control.0).is_err() {
                mark_launch_failed(registry, &record);
                close_bound_capabilities(&bound_capabilities);
                return lifecycle::AppLifecycleStatus::LaunchFailed;
            }
        }
        replay_permission_routes_for_provider(permission_routes, &manifest, control.0);
        if let Some(migration) = launch_migration.commit() {
            services.push(state::ManagedService {
                package: package_id.into(),
                process: process_name.into(),
                instance_id: instance_id.unwrap_or_default().into(),
                process_handle: launched.process_handle.raw,
                space_handle: launched.address_space_handle.raw,
                thread_handle: launched.main_thread_handle.raw,
                manager: control.0,
                migration: migration.0,
                hardware: 0,
                generation: 0,
                archive: 0,
                archive_len: 0,
                resource_group_id,
                resource_job: network_resource_job.take().map_or(0, |job| job.retain()),
            });
            log(&format!("appd: process ready package={package_id}\n"));
        }
    }
    let progress = if package_id == "bexos.platform.storage_verify" && arg0 == 0xbeef {
        control
            .recv()
            .ok()
            .and_then(|m| m.handles.first().copied())
            .unwrap_or(0)
    } else {
        0
    };
    let launched_record = state::LaunchRecord {
        package: package_id.into(),
        process: process_name.into(),
        instance_id: instance_id.unwrap_or_default().into(),
        process_handle: launched.process_handle.raw,
        space_handle: launched.address_space_handle.raw,
        thread_handle: launched.main_thread_handle.raw,
        manager: control.0,
        progress,
        uid,
        job_token: if job_control.is_some() || command.is_some() {
            arg0
        } else {
            0
        },
    };
    if job_control.is_none()
        && command.is_none()
        && process.service
        && record.protected
        && record.multi_version_policy == crate::MultiVersionPolicy::SingleActiveOnly
    {
        let now_ns = monotonic_ns().unwrap_or_else(|| {
            bexos_userspace::syscall::ticks().saturating_mul(1_000_000_000)
                / bexos_userspace::syscall::frequency().max(1)
        });
        if !launches.iter().any(|launch| {
            launch.package == launched_record.package && launch.process == launched_record.process
        }) {
            let _ = registry.mark_pin_health(&record.package_id, HealthCheckStatus::Probation);
        }
        // The service-loop watchdog owns promotion and crash counters after launch.
        let _ = now_ns;
    }
    startup_guard.ready();
    launches.push(launched_record);
    let _ = registry.mark_lifecycle(&record.package_key(), LifecycleState::Running);
    let _ = Memory::close(archive_root.0);
    lifecycle::AppLifecycleStatus::Ok
}

struct NetworkResourceJob {
    id: u32,
    handle: Option<u64>,
}

impl NetworkResourceJob {
    fn retain(mut self) -> u64 {
        self.handle.take().unwrap_or(0)
    }
}

impl Drop for NetworkResourceJob {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = Memory::close(handle);
        }
    }
}

fn create_network_instance_resource_group(
    config: &PlatformConfig,
    services: &[state::ManagedService],
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    package: &str,
    process: &str,
    instance_id: Option<&str>,
) -> Result<Option<NetworkResourceJob>, lifecycle::AppLifecycleStatus> {
    let Some(instance_id) = instance_id else {
        return Ok(None);
    };
    let Some(group) = config.network_policy.isolation_groups.iter().find(|group| {
        group.name == instance_id
            && ((group.networkd_package == package && group.networkd_process == process)
                || (group.netstackd_package == package && group.netstackd_process == process))
    }) else {
        log(&alloc::format!(
            "appd: network resource policy denied instance={instance_id} package={package} process={process}: isolation group missing\n"
        ));
        return Err(lifecycle::AppLifecycleStatus::AccessDenied);
    };
    let Some(template) = config
        .network_policy
        .resource_templates
        .iter()
        .find(|template| template.name == group.resource_template)
    else {
        log(&alloc::format!(
            "appd: network resource policy denied instance={instance_id} process={process}: template missing\n"
        ));
        return Err(lifecycle::AppLifecycleStatus::AccessDenied);
    };
    let active = services
        .iter()
        .filter(|service| {
            config
                .network_policy
                .isolation_groups
                .iter()
                .find(|candidate| candidate.name == service.instance_id)
                .is_some_and(|candidate| candidate.resource_template == template.name)
        })
        .count();
    if active >= template.max_instances as usize {
        return Err(lifecycle::AppLifecycleStatus::LaunchFailed);
    }
    let parent = crate::runner::KernelOps::open_resource_group(kernel, "system").map_err(|error| {
        log(&alloc::format!(
            "appd: network resource parent open failed instance={instance_id} process={process} error={error:?}\n"
        ));
        lifecycle::AppLifecycleStatus::LaunchFailed
    })?;
    // Kernel resource-group names are limited to 24 bytes. Network instance
    // and process names are signed policy identifiers and may be longer, so
    // derive a deterministic bounded name from both instead of truncating and
    // risking collisions between networkd and netstackd.
    let mut identity_hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in instance_id
        .bytes()
        .chain(core::iter::once(0))
        .chain(process.bytes())
    {
        identity_hash ^= u64::from(byte);
        identity_hash = identity_hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let resource_name = alloc::format!("net-{identity_hash:016x}");
    let created = crate::runner::KernelOps::create_resource_group_v2(
        kernel,
        &resource_name,
        parent.handle,
        crate::runner::ResourceGroupLimits {
            cpu_weight: template.cpu_weight,
            max_cpu_utilization_permille: 0,
            allow_realtime: false,
            memory_low_watermark_bytes: 0,
            memory_high_watermark_bytes: template.memory_high_bytes,
            max_render_budget_percent: 0,
            max_vram_bytes: 0,
        },
    );
    let _ = Memory::close(parent.handle.raw);
    let created = created.map_err(|error| {
        log(&alloc::format!(
            "appd: network resource create failed instance={instance_id} process={process} name={resource_name} error={error:?}\n"
        ));
        lifecycle::AppLifecycleStatus::LaunchFailed
    })?;
    Ok(Some(NetworkResourceJob {
        id: created.id,
        handle: Some(created.handle.raw),
    }))
}

fn networkd_config_snapshot(
    manifest: &Manifest,
    config: &PlatformConfig,
    instance_id: &str,
) -> Option<Vec<u8>> {
    use bexos_migration::codec::Encoder;
    let group = config
        .network_policy
        .isolation_groups
        .iter()
        .find(|group| group.name == instance_id)?;
    let mut policy = Encoder::new();
    policy.word(0x4e45_5450_4f4c_3031);
    policy.word(2);
    policy.text(instance_id);
    policy.word(config.network_policy.max_dynamic_providers as u64);
    let domains = config
        .network_policy
        .domains
        .iter()
        .filter(|domain| domain.isolation_group == instance_id)
        .collect::<Vec<_>>();
    policy.word(domains.len() as u64);
    for domain in domains {
        policy.text(&domain.name);
        policy.word(domain.table_id as u64);
        policy.word(domain.system_default as u64);
    }
    policy.word(group.table_ids.len() as u64);
    for table in &group.table_ids {
        policy.word(*table as u64);
    }
    let ports = config
        .network_policy
        .virtual_ports
        .iter()
        .filter(|port| port.isolation_group == instance_id)
        .collect::<Vec<_>>();
    policy.word(ports.len() as u64);
    for port in ports {
        policy.word(port.port_id);
        policy.word(port.table_id as u64);
        policy.text(&port.physical_selector);
        policy.bytes(&port.source_mac);
        policy.word(port.vlan_id as u64);
        policy.word(port.tagged as u64);
        policy.word(port.rx_queue_depth as u64);
        policy.word(port.tx_queue_depth as u64);
    }
    let routes = config
        .network_policy
        .routes
        .iter()
        .filter(|route| route.isolation_group == instance_id)
        .collect::<Vec<_>>();
    policy.word(routes.len() as u64);
    for route in routes {
        policy.word(route.table_id as u64);
        policy.bytes(&route.destination.address);
        policy.word(route.destination.prefix_len as u64);
        policy.bytes(&route.gateway);
        policy.word(route.interface_id);
        policy.word(route.metric as u64);
    }
    let upstreams = config
        .network_policy
        .dns_upstreams
        .iter()
        .filter(|upstream| upstream.isolation_group == instance_id)
        .collect::<Vec<_>>();
    policy.word(upstreams.len() as u64);
    for upstream in upstreams {
        policy.word(upstream.table_id as u64);
        policy.text(&upstream.provider);
        policy.text(&upstream.domain_suffix);
        policy.bytes(&upstream.bootstrap_address);
        policy.word(upstream.port as u64);
        policy.word(match upstream.transport {
            crate::platform_config::NetworkDnsTransport::Udp53 => 1,
            crate::platform_config::NetworkDnsTransport::Dot => 2,
            crate::platform_config::NetworkDnsTransport::Doh => 3,
            crate::platform_config::NetworkDnsTransport::Unspecified => return None,
        });
        policy.text(&upstream.tls_server_name);
        policy.text(&upstream.doh_path);
        policy.word(upstream.priority as u64);
    }
    let routed = config
        .network_policy
        .routed_interfaces
        .iter()
        .filter(|interface| {
            group.table_ids.contains(&interface.table_id) && interface.routing_enabled
        })
        .collect::<Vec<_>>();
    policy.word(routed.len() as u64);
    for interface in routed {
        policy.word(interface.interface_id);
        policy.word(interface.physical_interface);
        policy.word(interface.virtual_port);
        policy.word(interface.table_id as u64);
        policy.word(interface.bridge_domain as u64);
        policy.word(interface.vlan_id as u64);
        policy.bytes(&interface.mac);
        policy.word(interface.mtu as u64);
        policy.word(interface.security_zone as u64);
        policy.word(interface.addresses.len() as u64);
        for address in &interface.addresses {
            policy.bytes(&address.address);
            policy.word(address.prefix_len as u64);
        }
    }
    let switch_routes = config
        .network_policy
        .switch_routes
        .iter()
        .filter(|route| group.table_ids.contains(&route.table_id))
        .collect::<Vec<_>>();
    policy.word(switch_routes.len() as u64);
    for route in switch_routes {
        policy.word(route.table_id as u64);
        policy.bytes(&route.destination.address);
        policy.word(route.destination.prefix_len as u64);
        policy.bytes(&route.gateway);
        policy.word(route.interface_id);
        policy.word(route.metric as u64);
    }
    policy.bytes(&firewall_extension_config(&config.network_policy.firewall)?);
    encode_extension_artifact(
        &mut policy,
        &config.network_policy.firewall.desired_artifact,
    );
    policy.word(config.network_policy.nat.enabled as u64);
    policy.bytes(&nat_extension_config(&config.network_policy.nat)?);
    encode_extension_artifact(&mut policy, &config.network_policy.nat.desired_artifact);
    let mut values = manifest
        .config_schema
        .resolve(
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
        .ok()?;
    values.insert(
        "instance_id".into(),
        crate::ComponentConfigValue::String(instance_id.into()),
    );
    values.insert(
        "domains".into(),
        crate::ComponentConfigValue::String(
            config
                .network_policy
                .domains
                .iter()
                .filter(|domain| domain.isolation_group == instance_id)
                .map(|domain| alloc::format!("{}:{}", domain.name, domain.table_id))
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    values.insert(
        "max_dynamic_providers".into(),
        crate::ComponentConfigValue::Uint32(config.network_policy.max_dynamic_providers),
    );
    values.insert(
        "boot_policy".into(),
        crate::ComponentConfigValue::Bytes(policy.finish()),
    );
    manifest.config_schema.encode_table(&values, 0).ok()
}

fn encode_extension_artifact(
    out: &mut bexos_migration::codec::Encoder,
    artifact: &crate::platform_config::NetworkExtensionArtifact,
) {
    out.text(&artifact.registry_host);
    out.text(&artifact.repository);
    out.text(&artifact.tag);
    out.bytes(&artifact.expected_digest);
    out.text(&artifact.media_type);
    out.text(&artifact.abi);
}

fn firewall_extension_config(
    firewall: &crate::platform_config::NetworkFirewallPolicy,
) -> Option<Vec<u8>> {
    if firewall.rules.len() > 128 {
        return None;
    }
    let mut bytes = Vec::with_capacity(8 + firewall.rules.len() * 56);
    bytes.extend_from_slice(b"NFW1");
    bytes.extend_from_slice(&(firewall.rules.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&[0; 2]);
    for rule in &firewall.rules {
        let mut encoded = [0u8; 56];
        encoded[0] = u8::from(rule.allow);
        encoded[1] = match rule.direction.as_str() {
            "physical_ingress" | "inbound" => 1,
            "virtual_ingress" | "outbound" => 2,
            _ => 0,
        };
        encoded[2] = rule.protocol;
        encoded[3] = match rule.connection_state.as_str() {
            "new" => 1,
            "established" | "related" => 2,
            _ => 0,
        };
        let source_zone = rule.source_zone.parse::<u16>().unwrap_or(0);
        let destination_zone = rule.destination_zone.parse::<u16>().unwrap_or(0);
        encoded[4..6].copy_from_slice(&source_zone.to_le_bytes());
        encoded[6..8].copy_from_slice(&destination_zone.to_le_bytes());
        encoded[8] = rule.source.prefix_len;
        encoded[9] = rule.destination.prefix_len;
        copy_ip(&mut encoded[12..28], &rule.source.address)?;
        copy_ip(&mut encoded[28..44], &rule.destination.address)?;
        encoded[44..46].copy_from_slice(&rule.source_port_start.to_le_bytes());
        encoded[46..48].copy_from_slice(
            &rule
                .source_port_end
                .max(rule.source_port_start)
                .to_le_bytes(),
        );
        encoded[48..50].copy_from_slice(&rule.destination_port_start.to_le_bytes());
        encoded[50..52].copy_from_slice(
            &rule
                .destination_port_end
                .max(rule.destination_port_start)
                .to_le_bytes(),
        );
        encoded[52] = match rule.icmp_type {
            Some(value) => u8::try_from(value).ok()?,
            None => u8::MAX,
        };
        encoded[53] = match rule.icmp_code {
            Some(value) => u8::try_from(value).ok()?,
            None => u8::MAX,
        };
        bytes.extend_from_slice(&encoded);
    }
    Some(bytes)
}

fn nat_extension_config(nat: &crate::platform_config::NetworkNatPolicy) -> Option<Vec<u8>> {
    if !nat.enabled {
        return Some(Vec::new());
    }
    let external: [u8; 4] = nat.external_ipv4_pool.first()?.as_slice().try_into().ok()?;
    if nat.port_forwards.len() > 128 {
        return None;
    }
    let mut bytes = vec![0u8; 48 + nat.port_forwards.len() * 16];
    bytes[..4].copy_from_slice(b"NAT1");
    bytes[4..8].copy_from_slice(&external);
    bytes[8..10].copy_from_slice(&nat.ephemeral_port_start.to_le_bytes());
    bytes[10..12].copy_from_slice(&nat.ephemeral_port_end.to_le_bytes());
    bytes[12] = nat.nptv6_internal.prefix_len;
    bytes[14..16].copy_from_slice(&(nat.port_forwards.len() as u16).to_le_bytes());
    copy_ip(&mut bytes[16..32], &nat.nptv6_internal.address)?;
    copy_ip(&mut bytes[32..48], &nat.nptv6_external.address)?;
    for (index, forward) in nat.port_forwards.iter().enumerate() {
        let start = 48 + index * 16;
        let external: [u8; 4] = forward.external_address.as_slice().try_into().ok()?;
        let internal: [u8; 4] = forward.internal_address.as_slice().try_into().ok()?;
        bytes[start..start + 4].copy_from_slice(&external);
        bytes[start + 4..start + 6].copy_from_slice(&forward.external_port.to_le_bytes());
        bytes[start + 6..start + 10].copy_from_slice(&internal);
        bytes[start + 10..start + 12].copy_from_slice(&forward.internal_port.to_le_bytes());
        bytes[start + 12] = forward.protocol;
    }
    Some(bytes)
}

fn copy_ip(out: &mut [u8], address: &[u8]) -> Option<()> {
    match address.len() {
        0 => out.fill(0),
        4 => out[..4].copy_from_slice(address),
        16 => out.copy_from_slice(address),
        _ => return None,
    }
    Some(())
}

fn mark_launch_failed(registry: &mut MemoryAppRegistry, record: &bexos_app_registry::AppRecord) {
    let _ = registry.mark_pin_health(&record.package_id, HealthCheckStatus::CrashLoop);
    if record.protected {
        let _ = registry.rollback_to_previous(&record.package_id);
    }
}

fn register_version_manager_bindings(
    version_manager_bindings: &mut Vec<u64>,
    bindings: &[BoundCapability],
) {
    for binding in bindings
        .iter()
        .filter(|binding| is_appd_version_manager_binding(binding))
    {
        version_manager_bindings.push(binding.provider_endpoint.object_id);
    }
}

fn register_app_manager_bindings(
    app_manager_bindings: &mut Vec<crate::AppManagerBinding>,
    bindings: &[BoundCapability],
    package: &str,
    uid: u64,
) {
    for binding in bindings
        .iter()
        .filter(|binding| is_appd_manager_binding(binding))
    {
        app_manager_bindings.push(crate::AppManagerBinding {
            channel: binding.provider_endpoint.object_id,
            package: package.to_string(),
            uid,
            system: uid == SYSTEM_UID,
        });
    }
}

fn register_worker_launcher_bindings(
    worker_launcher_bindings: &mut Vec<crate::AppManagerBinding>,
    bindings: &[BoundCapability],
    package: &str,
    uid: u64,
) {
    for binding in bindings
        .iter()
        .filter(|binding| is_appd_worker_launcher_binding(binding))
    {
        worker_launcher_bindings.push(crate::AppManagerBinding {
            channel: binding.provider_endpoint.object_id,
            package: package.to_string(),
            uid,
            system: uid == SYSTEM_UID,
        });
    }
}

fn register_service_directory_bindings(
    service_directory_bindings: &mut Vec<crate::ServiceDirectoryBinding>,
    bindings: &[BoundCapability],
    package: &str,
    uid: u64,
    shell: bool,
) {
    for binding in bindings
        .iter()
        .filter(|binding| is_appd_service_directory_binding(binding))
    {
        service_directory_bindings.push(crate::ServiceDirectoryBinding {
            channel: binding.provider_endpoint.object_id,
            package: package.to_string(),
            uid,
            system: uid == SYSTEM_UID,
            shell,
        });
    }
}

fn handle_worker_launcher_message(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &crate::AppManagerBinding,
    channel: u64,
    bytes: &[u8],
    handles: &[u64],
) {
    let (ordinal, req) = envelope(bytes);
    let hs = handles
        .iter()
        .map(|raw| app_worker::HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    match ordinal {
        1 => {
            let status = match app_worker::WorkerLauncherSpawnWorkerRequest::decode(req, &hs) {
                Ok(request) => match request.job_control.raw {
                    0 => app_worker::WorkerLaunchStatus::InvalidArgs,
                    job_control => lifecycle_to_worker_status(launch_lifecycle_app(
                        &mut state.registry,
                        &mut state.launches,
                        &mut state.services,
                        state.vfsd,
                        state.users,
                        kernel,
                        &mut state.broker,
                        &mut state.permission_routes,
                        &mut state.permissions,
                        &mut state.opener_bindings,
                        &mut state.version_manager_bindings,
                        &mut state.app_manager_bindings,
                        &mut state.worker_launcher_bindings,
                        &mut state.service_directory_bindings,
                        &mut state.lazy,
                        &state.domain_associations,
                        &state.config,
                        &state.component_configs,
                        request.package_id,
                        request.process_name,
                        request.job_token,
                        request.uid,
                        Some(job_control),
                        true,
                    )),
                },
                Err(_) => app_worker::WorkerLaunchStatus::InvalidArgs,
            };
            worker_reply(
                channel,
                &app_worker::WorkerLauncherSpawnWorkerResponse { status },
            );
        }
        2 => {
            let (status, instance_id, owned_declarations) =
                match app_worker::WorkerLauncherGetJobDeclarationsRequest::decode(req, &hs) {
                    Ok(request) => job_declarations(&state.registry, request.package_id),
                    Err(_) => (app_worker::WorkerLaunchStatus::InvalidArgs, 0, Vec::new()),
                };
            let declarations = owned_declarations
                .iter()
                .map(|job| app_worker::JobDeclaration {
                    job_id: &job.job_id,
                    target_component: &job.target_component,
                    initial_delay_seconds: job.initial_delay_seconds,
                    interval_seconds: job.interval_seconds,
                    flex_window_seconds: job.flex_window_seconds,
                    network: job.network,
                    requires_charging: job.requires_charging,
                    requires_device_idle: job.requires_device_idle,
                    requires_battery_not_low: job.requires_battery_not_low,
                    persist_across_reboots: job.persist_across_reboots,
                    max_execution_seconds: job.max_execution_seconds,
                })
                .collect::<Vec<_>>();
            worker_reply(
                channel,
                &app_worker::WorkerLauncherGetJobDeclarationsResponse {
                    status,
                    package_instance_id: instance_id,
                    declarations: app_worker::WireVector::from_slice(&declarations),
                },
            );
        }
        3 => {
            let status = match app_worker::WorkerLauncherStopWorkerRequest::decode(req, &hs) {
                Ok(request) if binding.package == "bexos.service.jobd" => stop_worker(
                    state,
                    kernel,
                    request.package_id,
                    request.uid,
                    request.job_token,
                    request.exit_code,
                ),
                Ok(_) => app_worker::WorkerLaunchStatus::AccessDenied,
                Err(_) => app_worker::WorkerLaunchStatus::InvalidArgs,
            };
            worker_reply(
                channel,
                &app_worker::WorkerLauncherStopWorkerResponse { status },
            );
        }
        4 => {
            let status = match app_worker::WorkerLauncherWatchPackagePolicyRequest::decode(req, &hs)
            {
                Ok(request) if request.watcher.raw != 0 => {
                    state.worker_policy_watchers.push(request.watcher.raw);
                    app_worker::WorkerLaunchStatus::Ok
                }
                Ok(_) => app_worker::WorkerLaunchStatus::InvalidArgs,
                Err(_) => app_worker::WorkerLaunchStatus::InvalidArgs,
            };
            worker_reply(
                channel,
                &app_worker::WorkerLauncherWatchPackagePolicyResponse { status },
            );
        }
        _ => {
            for handle in handles {
                let _ = Memory::close(*handle);
            }
        }
    }
}

fn handle_service_directory_message(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &crate::ServiceDirectoryBinding,
    channel: u64,
    bytes: &[u8],
    handles: &[u64],
) {
    let (ordinal, req) = envelope(bytes);
    let hs = handles
        .iter()
        .map(|raw| service_directory::HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    match ordinal {
        3 => {
            let status = match service_directory::ServiceDirectoryBindShellSessionRequest::decode(
                req, &hs,
            ) {
                Ok(q)
                    if handles.len() == 1
                        && binding.shell
                        && state.shell.authorized(&binding.package, binding.uid)
                        && state.shell.clients.len() < 16 =>
                {
                    state.shell.clients.push(crate::shell::Client {
                        callback: 0,
                        channel: q.endpoint.raw,
                        package: binding.package.clone(),
                        uid: binding.uid,
                        epoch: state.shell.epoch,
                    });
                    service_directory::ServiceDirectoryStatus::Ok
                }
                _ => {
                    close_handles(handles);
                    service_directory::ServiceDirectoryStatus::AccessDenied
                }
            };
            service_directory_reply(
                Channel(channel),
                &service_directory::ServiceDirectoryBindShellSessionResponse { status },
            );
        }

        1 => {
            let request = service_directory::ServiceDirectoryDiscoverRequest::decode(req, &hs);
            match request {
                Ok(request) => {
                    let client = client_context_from_grants(
                        &binding.package,
                        binding.uid,
                        &state.permissions,
                    );
                    let query = crate::InterfaceQuery {
                        protocol: (!request.protocol.is_empty()).then_some(request.protocol),
                        metadata: &[],
                    };
                    let interfaces = state.broker.get_interfaces(&client, query);
                    let entries = interfaces
                        .iter()
                        .map(|interface| service_directory::ServiceDirectoryEntry {
                            service: &interface.name,
                            protocol: &interface.protocol,
                            provider_package: &interface.provider_package,
                            lifecycle: match interface.lifecycle {
                                Lifecycle::Singleton => 1,
                                Lifecycle::UserScopedSingleton => 2,
                                Lifecycle::MultipleInstance => 3,
                                Lifecycle::Unspecified => 0,
                            },
                            lazy: interface.activation == crate::ServiceActivation::Lazy,
                        })
                        .collect::<Vec<_>>();
                    service_directory_reply(
                        Channel(channel),
                        &service_directory::ServiceDirectoryDiscoverResponse {
                            status: service_directory::ServiceDirectoryStatus::Ok,
                            entries: service_directory::WireVector::from_slice(&entries),
                        },
                    );
                }
                Err(_) => service_directory_reply(
                    Channel(channel),
                    &service_directory::ServiceDirectoryDiscoverResponse {
                        status: service_directory::ServiceDirectoryStatus::InvalidArgs,
                        entries: service_directory::WireVector::from_slice(&[]),
                    },
                ),
            }
        }
        2 => {
            let status = match service_directory::ServiceDirectoryConnectRequest::decode(req, &hs) {
                Ok(request) => service_directory_connect(state, kernel, binding, request),
                Err(_) => {
                    close_handles(handles);
                    service_directory::ServiceDirectoryStatus::InvalidArgs
                }
            };
            service_directory_reply(
                Channel(channel),
                &service_directory::ServiceDirectoryConnectResponse { status },
            );
        }
        _ => close_handles(handles),
    }
}

fn service_directory_connect(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &crate::ServiceDirectoryBinding,
    request: service_directory::ServiceDirectoryConnectRequest<'_>,
) -> service_directory::ServiceDirectoryStatus {
    if request.service.is_empty() || request.endpoint.raw == 0 {
        if request.endpoint.raw != 0 {
            let _ = Memory::close(request.endpoint.raw);
        }
        return service_directory::ServiceDirectoryStatus::InvalidArgs;
    }
    let capability = if request.capability.is_empty() {
        "Public"
    } else {
        request.capability
    };
    let manifest_bytes = match state.registry.record(&binding.package) {
        Ok(record) => record.manifest_bytes.clone(),
        Err(_) => {
            let _ = Memory::close(request.endpoint.raw);
            return service_directory::ServiceDirectoryStatus::AccessDenied;
        }
    };
    let manifest = match Manifest::decode(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(_) => {
            let _ = Memory::close(request.endpoint.raw);
            return service_directory::ServiceDirectoryStatus::AccessDenied;
        }
    };
    let Some(consumed) = manifest
        .services_consumed
        .iter()
        .find(|consumed| consumed.name == request.service)
    else {
        let _ = Memory::close(request.endpoint.raw);
        return service_directory::ServiceDirectoryStatus::AccessDenied;
    };
    let client = client_context_from_grants(&binding.package, binding.uid, &state.permissions);
    let bound = match state.broker.bind_runtime_capability(
        &client,
        consumed,
        capability,
        bexos_kernel_core::ipc::Capability {
            object_id: request.endpoint.raw,
            rights: crate::broker::SERVICE_ENDPOINT_RIGHTS,
        },
    ) {
        Ok(bound) => bound,
        Err(crate::BindError::NotFound) => {
            let _ = Memory::close(request.endpoint.raw);
            return service_directory::ServiceDirectoryStatus::NotFound;
        }
        Err(_) => {
            let _ = Memory::close(request.endpoint.raw);
            return service_directory::ServiceDirectoryStatus::AccessDenied;
        }
    };
    if bound.activation == crate::ServiceActivation::Lazy {
        let status = activate_lazy_bindings(
            &mut state.registry,
            &mut state.launches,
            &mut state.services,
            state.vfsd,
            state.users,
            kernel,
            &mut state.broker,
            &mut state.permission_routes,
            &mut state.permissions,
            &mut state.opener_bindings,
            &mut state.version_manager_bindings,
            &mut state.app_manager_bindings,
            &mut state.worker_launcher_bindings,
            &mut state.service_directory_bindings,
            &mut state.lazy,
            &state.domain_associations,
            &state.config,
            &state.component_configs,
            core::slice::from_ref(&bound),
        );
        return crate::service_directory::status_from_lifecycle(status);
    }
    let metadata = provider_binding_metadata(&bound);
    if let Err(error) = deliver_provider_endpoint(&bound, &metadata) {
        log(&format!(
            "appd: service directory endpoint delivery failed: {error:?}\n"
        ));
        let _ = Memory::close(request.endpoint.raw);
        return service_directory::ServiceDirectoryStatus::LaunchFailed;
    }
    // Delivery transfers a duplicate to the provider. This runtime request does
    // not retain a binding record, so release the broker's original endpoint.
    let _ = Memory::close(request.endpoint.raw);
    service_directory::ServiceDirectoryStatus::Ok
}

fn job_declarations<'a>(
    registry: &MemoryAppRegistry,
    package_id: &str,
) -> (
    app_worker::WorkerLaunchStatus,
    u64,
    Vec<OwnedJobDeclaration>,
) {
    let Ok(record) = registry.record(package_id) else {
        return (app_worker::WorkerLaunchStatus::NotFound, 0, Vec::new());
    };
    let Ok(manifest) = Manifest::decode(&record.manifest_bytes) else {
        return (app_worker::WorkerLaunchStatus::InvalidArgs, 0, Vec::new());
    };
    let declarations = manifest
        .jobs
        .iter()
        .map(|job| OwnedJobDeclaration {
            job_id: job.job_id.clone(),
            target_component: job.target_component.clone(),
            initial_delay_seconds: job.initial_delay_seconds,
            interval_seconds: job.interval_seconds,
            flex_window_seconds: job.flex_window_seconds,
            network: match job.network {
                crate::manifest::JobNetworkConstraint::None => {
                    app_worker::JobNetworkConstraint::None
                }
                crate::manifest::JobNetworkConstraint::UnmeteredOnly => {
                    app_worker::JobNetworkConstraint::UnmeteredOnly
                }
                _ => app_worker::JobNetworkConstraint::Any,
            },
            requires_charging: job.requires_charging,
            requires_device_idle: job.requires_device_idle,
            requires_battery_not_low: job.requires_battery_not_low,
            persist_across_reboots: job.persist_across_reboots,
            max_execution_seconds: job.max_execution_seconds,
        })
        .collect();
    (
        app_worker::WorkerLaunchStatus::Ok,
        record.package_instance_id,
        declarations,
    )
}

struct OwnedJobDeclaration {
    job_id: String,
    target_component: String,
    initial_delay_seconds: u64,
    interval_seconds: u64,
    flex_window_seconds: u32,
    network: app_worker::JobNetworkConstraint,
    requires_charging: bool,
    requires_device_idle: bool,
    requires_battery_not_low: bool,
    persist_across_reboots: bool,
    max_execution_seconds: u32,
}

fn stop_worker(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    package_id: &str,
    uid: u64,
    job_token: u64,
    exit_code: i32,
) -> app_worker::WorkerLaunchStatus {
    let Some(index) = state.launches.iter().position(|launch| {
        launch.package == package_id && launch.uid == uid && launch.job_token == job_token
    }) else {
        return app_worker::WorkerLaunchStatus::NotFound;
    };
    let launch = state.launches.remove(index);
    let _ = crate::runner::KernelOps::terminate_process(
        kernel,
        KernelHandle {
            raw: launch.process_handle,
        },
        exit_code,
    );
    for handle in launch.handles() {
        if handle != 0 {
            let _ = Memory::close(handle);
        }
    }
    app_worker::WorkerLaunchStatus::Ok
}

fn notify_package_policy_watchers(
    state: &mut state::AppdState,
    package_id: &str,
    package_instance_id: u64,
    kind: app_worker::PackagePolicyEventKind,
) {
    let generation = state.generation.saturating_add(1);
    state.worker_policy_watchers.retain(|channel| {
        let mut client = app_worker::PackagePolicyWatcherPublicClient::new(Rpc(Channel(*channel)));
        let mut request_bytes = [0; 256];
        let mut request_handles = [app_worker::HandleRef { raw: 0 }; 1];
        client
            .on_package_policy_changed(
                &app_worker::PackagePolicyWatcherOnPackagePolicyChangedRequest {
                    generation,
                    package_id,
                    package_instance_id,
                    kind,
                },
                &mut request_bytes,
                &mut request_handles,
            )
            .is_ok()
    });
}

fn lifecycle_to_worker_status(
    status: lifecycle::AppLifecycleStatus,
) -> app_worker::WorkerLaunchStatus {
    match status {
        lifecycle::AppLifecycleStatus::Ok => app_worker::WorkerLaunchStatus::Ok,
        lifecycle::AppLifecycleStatus::NotFound => app_worker::WorkerLaunchStatus::NotFound,
        lifecycle::AppLifecycleStatus::InvalidArgs => app_worker::WorkerLaunchStatus::InvalidArgs,
        lifecycle::AppLifecycleStatus::AccessDenied => app_worker::WorkerLaunchStatus::AccessDenied,
        lifecycle::AppLifecycleStatus::Storage => app_worker::WorkerLaunchStatus::Storage,
        lifecycle::AppLifecycleStatus::VerifyFailed
        | lifecycle::AppLifecycleStatus::LaunchFailed => {
            app_worker::WorkerLaunchStatus::LaunchFailed
        }
    }
}

fn worker_reply<T: WorkerEncode>(channel: u64, response: &T) {
    let mut bytes = [0; 512];
    let mut handles = [app_worker::HandleRef { raw: 0 }; 8];
    if let Ok(encoded) = response.encode(&mut bytes, &mut handles) {
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = Channel(channel).send(&bytes[..encoded.bytes], &raw_handles);
    }
}

fn service_directory_reply<Q: ServiceDirectoryEncode>(channel: Channel, response: &Q) {
    let mut bytes = [0; 4096];
    let mut handles = [service_directory::HandleRef { raw: 0 }; 8];
    if let Ok(encoded) = response.encode(&mut bytes, &mut handles) {
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = channel.send(&bytes[..encoded.bytes], &raw_handles);
    }
}

fn register_opener_bindings(
    opener_bindings: &mut Vec<OpenerBinding>,
    bindings: &[BoundCapability],
    package: &str,
    uid: u64,
) {
    for binding in bindings
        .iter()
        .filter(|binding| is_appd_opener_binding(binding))
    {
        opener_bindings.push(OpenerBinding {
            channel: binding.provider_endpoint.object_id,
            package: package.to_string(),
            uid,
            system: uid == SYSTEM_UID,
        });
    }
}

fn handle_opener_message(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &OpenerBinding,
    bytes: &[u8],
    handles: &[u64],
) {
    if bytes.len() < 8 {
        close_handles(handles);
        return;
    }
    let (ordinal, req) = envelope(bytes);
    let hs = opener_refs(handles);
    match ordinal {
        6 => {
            let status = match opener_fidl::OpenerBindCommandLauncherRequest::decode(req, &hs) {
                Ok(q)
                    if handles.len() == 1
                        && q.launcher.raw != 0
                        && state.commands.bindings.len() < crate::command_state::LIMIT =>
                {
                    state.commands.bindings.push(OpenerBinding {
                        channel: q.launcher.raw,
                        ..binding.clone()
                    });
                    opener_fidl::OpenerStatus::Ok
                }
                _ => {
                    close_handles(handles);
                    opener_fidl::OpenerStatus::InvalidArgs
                }
            };
            opener_reply(
                binding.channel,
                &opener_fidl::OpenerBindCommandLauncherResponse { status },
            );
        }

        1 => {
            let response = match opener_fidl::OpenerOpenUrlRequest::decode(req, &hs) {
                Ok(request) => {
                    let process = selected_process(&request.target);
                    let (status, result) = resolve_and_open(
                        state,
                        kernel,
                        binding,
                        OpenKind::Url(request.url),
                        process,
                        OpenerPayload::Url(request.url),
                    );
                    opener_fidl::OpenerOpenUrlResponse { status, result }
                }
                Err(_) => opener_response(
                    opener_fidl::OpenerStatus::InvalidArgs,
                    opener_fidl::OpenResult::AccessDenied,
                ),
            };
            opener_reply(binding.channel, &response);
        }
        2 => {
            let response = match opener_fidl::OpenerOpenFileRequest::decode(req, &hs) {
                Ok(request) => {
                    let process = selected_process(&request.target);
                    let (status, result) = resolve_and_open(
                        state,
                        kernel,
                        binding,
                        OpenKind::Mime(request.mime_type),
                        process,
                        OpenerPayload::File {
                            file: request.file.raw,
                            mime_type: request.mime_type,
                        },
                    );
                    if result != opener_fidl::OpenResult::Success {
                        let _ = Memory::close(request.file.raw);
                    }
                    opener_fidl::OpenerOpenFileResponse { status, result }
                }
                Err(_) => {
                    close_handles(handles);
                    opener_fidl::OpenerOpenFileResponse {
                        status: opener_fidl::OpenerStatus::InvalidArgs,
                        result: opener_fidl::OpenResult::AccessDenied,
                    }
                }
            };
            opener_reply(binding.channel, &response);
        }
        3 => {
            let response = match opener_fidl::OpenerOpenAppRequest::decode(req, &hs) {
                Ok(request) => {
                    let process = selected_process(&request.target);
                    match decode_opener_arguments(request.arguments) {
                        Ok(arguments) => open_app_direct(
                            state,
                            kernel,
                            binding,
                            request.package_name,
                            process,
                            arguments,
                        ),
                        Err(_) => opener_fidl::OpenerOpenAppResponse {
                            status: opener_fidl::OpenerStatus::InvalidArgs,
                            result: opener_fidl::OpenResult::AccessDenied,
                        },
                    }
                }
                Err(_) => opener_fidl::OpenerOpenAppResponse {
                    status: opener_fidl::OpenerStatus::InvalidArgs,
                    result: opener_fidl::OpenResult::AccessDenied,
                },
            };
            opener_reply(binding.channel, &response);
        }
        4 => {
            let selected_package_storage;
            let response = match opener_fidl::OpenerGetPreferredInterfaceRequest::decode(req, &hs) {
                Ok(request) => {
                    let fallback = state
                        .config
                        .interface_defaults
                        .iter()
                        .find(|d| d.interface_name == request.interface_name)
                        .map(|d| {
                            crate::HandlerId::new(d.package_name.clone(), d.process_name.clone())
                        });
                    let outcome = state.openers.resolve_with_default(
                        opener_scope(binding),
                        OpenKind::Interface(request.interface_name),
                        fallback.as_ref(),
                    );
                    match outcome {
                        ResolveOutcome::Selected(handler) => {
                            selected_package_storage = handler.package.clone();
                            let payload = OpenerPayload::Interface {
                                name: request.interface_name,
                                server_channel: request.server_channel.raw,
                            };
                            match open_resolved_handler(
                                state, kernel, binding, handler, None, payload,
                            ) {
                                opener_fidl::OpenResult::Success => {
                                    opener_fidl::OpenerGetPreferredInterfaceResponse {
                                        status: opener_fidl::OpenerStatus::Ok,
                                        selected_package: &selected_package_storage,
                                    }
                                }
                                _ => {
                                    let _ = Memory::close(request.server_channel.raw);
                                    opener_fidl::OpenerGetPreferredInterfaceResponse {
                                        status: opener_fidl::OpenerStatus::LaunchFailed,
                                        selected_package: "",
                                    }
                                }
                            }
                        }
                        ResolveOutcome::PromptPendingUser(_) => {
                            let _ = Memory::close(request.server_channel.raw);
                            opener_fidl::OpenerGetPreferredInterfaceResponse {
                                status: opener_fidl::OpenerStatus::AccessDenied,
                                selected_package: "",
                            }
                        }
                        ResolveOutcome::NoHandler => {
                            let _ = Memory::close(request.server_channel.raw);
                            opener_fidl::OpenerGetPreferredInterfaceResponse {
                                status: opener_fidl::OpenerStatus::NotFound,
                                selected_package: "",
                            }
                        }
                        ResolveOutcome::AccessDenied => {
                            let _ = Memory::close(request.server_channel.raw);
                            opener_fidl::OpenerGetPreferredInterfaceResponse {
                                status: opener_fidl::OpenerStatus::AccessDenied,
                                selected_package: "",
                            }
                        }
                    }
                }
                Err(_) => {
                    close_handles(handles);
                    opener_fidl::OpenerGetPreferredInterfaceResponse {
                        status: opener_fidl::OpenerStatus::InvalidArgs,
                        selected_package: "",
                    }
                }
            };
            opener_reply(binding.channel, &response);
        }
        5 => {
            let response = match opener_fidl::OpenerSetUserDefaultRequest::decode(req, &hs) {
                Ok(request) if !binding.system => match default_handler_for_package(
                    &state.openers,
                    binding.uid,
                    request.target_key,
                    request.package_name,
                ) {
                    Some(handler) => match state.openers.set_user_default(
                        binding.uid,
                        request.target_key.to_string(),
                        handler,
                    ) {
                        Ok(()) => opener_fidl::OpenerSetUserDefaultResponse {
                            status: opener_fidl::OpenerStatus::Ok,
                        },
                        Err(_) => opener_fidl::OpenerSetUserDefaultResponse {
                            status: opener_fidl::OpenerStatus::InvalidArgs,
                        },
                    },
                    None => opener_fidl::OpenerSetUserDefaultResponse {
                        status: opener_fidl::OpenerStatus::AccessDenied,
                    },
                },
                Ok(_) => opener_fidl::OpenerSetUserDefaultResponse {
                    status: opener_fidl::OpenerStatus::AccessDenied,
                },
                Err(_) => opener_fidl::OpenerSetUserDefaultResponse {
                    status: opener_fidl::OpenerStatus::InvalidArgs,
                },
            };
            opener_reply(binding.channel, &response);
        }
        _ => close_handles(handles),
    }
}

enum OpenerPayload<'a> {
    Url(&'a str),
    File { file: u64, mime_type: &'a str },
    App { arguments: Vec<String> },
    Interface { name: &'a str, server_channel: u64 },
}

fn resolve_and_open(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &OpenerBinding,
    kind: OpenKind<'_>,
    process: Option<&str>,
    payload: OpenerPayload<'_>,
) -> (opener_fidl::OpenerStatus, opener_fidl::OpenResult) {
    match state.openers.resolve(opener_scope(binding), kind) {
        ResolveOutcome::Selected(handler) => (
            opener_fidl::OpenerStatus::Ok,
            open_resolved_handler(state, kernel, binding, handler, process, payload),
        ),
        ResolveOutcome::PromptPendingUser(_) => (
            opener_fidl::OpenerStatus::Ok,
            opener_fidl::OpenResult::PromptPendingUser,
        ),
        ResolveOutcome::NoHandler => (
            opener_fidl::OpenerStatus::Ok,
            opener_fidl::OpenResult::NoHandlerRegistered,
        ),
        ResolveOutcome::AccessDenied => (
            opener_fidl::OpenerStatus::AccessDenied,
            opener_fidl::OpenResult::AccessDenied,
        ),
    }
}

fn open_resolved_handler(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &OpenerBinding,
    mut handler: ResolvedHandler,
    process: Option<&str>,
    payload: OpenerPayload<'_>,
) -> opener_fidl::OpenResult {
    if let Some(process) = process {
        handler.process = process.to_string();
    }
    match ensure_handler_running(
        state,
        kernel,
        binding.uid,
        &handler.package,
        &handler.process,
    ) {
        Some(manager) if deliver_opener_payload(manager, payload).is_ok() => {
            opener_fidl::OpenResult::Success
        }
        _ => opener_fidl::OpenResult::AccessDenied,
    }
}

fn open_app_direct(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    binding: &OpenerBinding,
    package: &str,
    process: Option<&str>,
    arguments: Vec<String>,
) -> opener_fidl::OpenerOpenAppResponse {
    let Some(process) = process
        .map(ToString::to_string)
        .or_else(|| default_process_name(&state.registry, package))
    else {
        return opener_fidl::OpenerOpenAppResponse {
            status: opener_fidl::OpenerStatus::AccessDenied,
            result: opener_fidl::OpenResult::NoHandlerRegistered,
        };
    };
    let payload = OpenerPayload::App { arguments };
    let result = open_resolved_handler(
        state,
        kernel,
        binding,
        ResolvedHandler {
            package: package.to_string(),
            process,
        },
        None,
        payload,
    );
    opener_fidl::OpenerOpenAppResponse {
        status: opener_fidl::OpenerStatus::Ok,
        result,
    }
}

fn ensure_handler_running(
    state: &mut state::AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    uid: u64,
    package: &str,
    process: &str,
) -> Option<u64> {
    if let Some(launch) =
        state.launches.iter().rev().find(|launch| {
            launch.package == package && launch.process == process && launch.uid == uid
        })
    {
        return Some(launch.manager);
    }
    let status = launch_lifecycle_app(
        &mut state.registry,
        &mut state.launches,
        &mut state.services,
        state.vfsd,
        state.users,
        kernel,
        &mut state.broker,
        &mut state.permission_routes,
        &mut state.permissions,
        &mut state.opener_bindings,
        &mut state.version_manager_bindings,
        &mut state.app_manager_bindings,
        &mut state.worker_launcher_bindings,
        &mut state.service_directory_bindings,
        &mut state.lazy,
        &state.domain_associations,
        &state.config,
        &state.component_configs,
        package,
        process,
        0,
        uid,
        None,
        false,
    );
    if status != lifecycle::AppLifecycleStatus::Ok {
        return None;
    }
    state
        .launches
        .iter()
        .rev()
        .find(|launch| launch.package == package && launch.process == process && launch.uid == uid)
        .map(|launch| launch.manager)
}

fn deliver_opener_payload(
    manager: u64,
    payload: OpenerPayload<'_>,
) -> Result<(), kernel_fidl::Status> {
    match payload {
        OpenerPayload::Url(url) => {
            let mut bytes = b"bexos.opener.url.v1\n".to_vec();
            bytes.extend_from_slice(url.as_bytes());
            Channel(manager).send(&bytes, &[])
        }
        OpenerPayload::File { file, mime_type } => {
            let mut bytes = b"bexos.opener.file.v1\n".to_vec();
            bytes.extend_from_slice(mime_type.as_bytes());
            Channel(manager).send(&bytes, &[file])
        }
        OpenerPayload::App { arguments } => {
            let mut bytes = b"bexos.opener.app.v1\n".to_vec();
            for (index, argument) in arguments.iter().enumerate() {
                if index != 0 {
                    bytes.push(0);
                }
                bytes.extend_from_slice(argument.as_bytes());
            }
            Channel(manager).send(&bytes, &[])
        }
        OpenerPayload::Interface {
            name,
            server_channel,
        } => {
            let mut bytes = b"bexos.opener.interface.v1\n".to_vec();
            bytes.extend_from_slice(name.as_bytes());
            Channel(manager).send(&bytes, &[server_channel])
        }
    }
}

fn default_process_name(registry: &MemoryAppRegistry, package: &str) -> Option<String> {
    let record = registry.record(package).ok()?;
    let manifest = Manifest::decode(&record.manifest_bytes).ok()?;
    manifest
        .processes
        .first()
        .map(|process| process.name.clone())
}

fn default_handler_for_package(
    openers: &MemoryOpenerRegistry,
    uid: u64,
    key: &str,
    package: &str,
) -> Option<HandlerId> {
    for entry in openers.user_handlers() {
        if entry.uid == uid
            && entry.registration.package == package
            && handler_key_matches(&entry.registration, key)
        {
            return Some(HandlerId::new(
                entry.registration.package,
                entry.registration.process,
            ));
        }
    }
    for entry in openers.system_handlers() {
        if entry.package == package && handler_key_matches(entry, key) {
            return Some(HandlerId::new(&entry.package, &entry.process));
        }
    }
    None
}

fn handler_key_matches(handler: &crate::HandlerRegistration, key: &str) -> bool {
    if let Some(mime) = key.strip_prefix("mime:") {
        handler.mime_types.iter().any(|candidate| candidate == mime)
    } else if let Some(scheme) = key.strip_prefix("scheme:") {
        handler.schemes.iter().any(|candidate| candidate == scheme)
    } else if let Some(interface) = key.strip_prefix("interface:") {
        handler
            .interfaces
            .iter()
            .any(|candidate| candidate == interface)
    } else {
        false
    }
}

fn selected_process<'a>(target: &'a opener_fidl::OpenTarget<'a>) -> Option<&'a str> {
    if target.specific_process.is_empty() || target.default_process {
        None
    } else {
        Some(target.specific_process)
    }
}

fn decode_opener_arguments(
    arguments: opener_fidl::WireStringVector<'_>,
) -> Result<Vec<String>, ()> {
    let mut out = Vec::new();
    for index in 0..arguments.len() {
        let argument = arguments.get(index).map_err(|_| ())?;
        if argument.contains('\0') || argument.len() > 256 {
            return Err(());
        }
        out.push(argument.to_string());
    }
    Ok(out)
}

fn opener_scope(binding: &OpenerBinding) -> OpenerScope {
    if binding.system {
        OpenerScope::System
    } else {
        OpenerScope::User(binding.uid)
    }
}

fn opener_response(
    status: opener_fidl::OpenerStatus,
    result: opener_fidl::OpenResult,
) -> opener_fidl::OpenerOpenUrlResponse {
    opener_fidl::OpenerOpenUrlResponse { status, result }
}

fn opener_refs(hs: &[u64]) -> Vec<opener_fidl::HandleRef> {
    hs.iter()
        .map(|h| opener_fidl::HandleRef { raw: *h })
        .collect()
}

fn opener_reply<Q: OpenerEncode>(channel: u64, q: &Q) {
    let mut bytes = alloc::vec![0; 65500];
    let mut hs = [opener_fidl::HandleRef { raw: 0 }; 16];
    let e = q.encode(&mut bytes, &mut hs).expect("app opener encode");
    let _ = Channel(channel).send(
        &bytes[..e.bytes],
        &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
    );
}

fn close_handles(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}

fn request_optional_permission(
    registry: &MemoryAppRegistry,
    launches: &[state::LaunchRecord],
    vfsd: Channel,
    users: Channel,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    broker: &AppdBroker,
    permission_routes: &mut crate::PermissionRouteTable,
    permissions: &mut MemoryPermissionStore,
    package_id: &str,
    process_name: &str,
    uid: u64,
    permission_name: &str,
    service_name: &str,
    capability_name: &str,
    requested_values: lifecycle::WireStringVector<'_>,
) -> (
    lifecycle::AppLifecycleStatus,
    lifecycle::PermissionState,
    Vec<String>,
    u64,
) {
    let Ok(record) = registry.record(package_id) else {
        return (
            lifecycle::AppLifecycleStatus::NotFound,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    };
    let Ok(manifest) = Manifest::decode(&record.manifest_bytes) else {
        return (
            lifecycle::AppLifecycleStatus::InvalidArgs,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    };
    let Some(process) = manifest
        .processes
        .iter()
        .find(|process| process.name == process_name)
    else {
        return (
            lifecycle::AppLifecycleStatus::NotFound,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    };
    if !launches.iter().any(|launch| {
        launch.package == package_id && launch.process == process_name && launch.uid == uid
    }) {
        return (
            lifecycle::AppLifecycleStatus::AccessDenied,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    }
    if uid != SYSTEM_UID && !user_is_unlocked(users, uid) {
        return (
            lifecycle::AppLifecycleStatus::AccessDenied,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    }
    if uid != SYSTEM_UID {
        match crate::permission_persistence::load_user(vfsd, uid, registry, permissions) {
            Ok(()) => {}
            Err(crate::permission_persistence::PermissionPersistenceError::Locked) => {
                return (
                    lifecycle::AppLifecycleStatus::AccessDenied,
                    lifecycle::PermissionState::Denied,
                    Vec::new(),
                    0,
                );
            }
            Err(_) => {
                return (
                    lifecycle::AppLifecycleStatus::Storage,
                    lifecycle::PermissionState::Denied,
                    Vec::new(),
                    0,
                );
            }
        }
    }
    let declarations = effective_permission_declarations(&manifest, process);
    let Some(declaration) = declarations.iter().find(|declaration| {
        declaration.name == permission_name
            && declaration.requirement == PermissionRequirement::Optional
    }) else {
        return (
            lifecycle::AppLifecycleStatus::AccessDenied,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    };
    let Ok(requested) = decode_permission_values(requested_values) else {
        return (
            lifecycle::AppLifecycleStatus::InvalidArgs,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    };
    let consumed = manifest
        .services_consumed
        .iter()
        .filter(|consumed| consumed.name == service_name)
        .collect::<Vec<_>>();
    if consumed.len() != 1 {
        return (
            lifecycle::AppLifecycleStatus::InvalidArgs,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    }
    let saved_user_records = permissions.user_records(uid);
    let candidate = permissions
        .grant_declared(uid, package_id, declaration, &requested)
        .ok();
    let Some(_candidate) = candidate else {
        return (
            lifecycle::AppLifecycleStatus::AccessDenied,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    };
    let client = client_context_from_grants(package_id, uid, permissions);
    let mut binding =
        match broker.bind_consumed_capability(&client, consumed[0], capability_name, kernel) {
            Ok(binding) => binding,
            Err(crate::BindError::Ambiguous) => {
                let _ = permissions.replace_user_records(uid, saved_user_records.clone());
                return (
                    lifecycle::AppLifecycleStatus::InvalidArgs,
                    lifecycle::PermissionState::Denied,
                    Vec::new(),
                    0,
                );
            }
            Err(crate::BindError::NotFound) => {
                let _ = permissions.replace_user_records(uid, saved_user_records.clone());
                return (
                    lifecycle::AppLifecycleStatus::NotFound,
                    lifecycle::PermissionState::Denied,
                    Vec::new(),
                    0,
                );
            }
            Err(_) => {
                let _ = permissions.replace_user_records(uid, saved_user_records.clone());
                return (
                    lifecycle::AppLifecycleStatus::AccessDenied,
                    lifecycle::PermissionState::Denied,
                    Vec::new(),
                    0,
                );
            }
        };
    if crate::policy::permission_scope_name(binding.permission.as_deref()) != Some(permission_name)
    {
        close_bound_capabilities(core::slice::from_ref(&binding));
        let _ = permissions.replace_user_records(uid, saved_user_records);
        return (
            lifecycle::AppLifecycleStatus::AccessDenied,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    }
    let persisted = crate::permission_persistence::grant_optional_and_sync(
        vfsd,
        uid,
        package_id,
        declaration,
        &requested,
        permissions,
    );
    let Ok(persisted) = persisted else {
        close_bound_capabilities(core::slice::from_ref(&binding));
        return (
            lifecycle::AppLifecycleStatus::Storage,
            lifecycle::PermissionState::Denied,
            Vec::new(),
            0,
        );
    };
    preferences::bind_identity(&mut binding, &record);
    let metadata = provider_binding_metadata(&binding);
    if deliver_provider_endpoint(&binding, &metadata).is_err() {
        close_bound_capabilities(core::slice::from_ref(&binding));
        return (
            lifecycle::AppLifecycleStatus::LaunchFailed,
            lifecycle::PermissionState::Granted,
            persisted.granted_values,
            0,
        );
    }
    let handle = binding.client_endpoint.object_id;
    permission_routes.retain(crate::PermissionRoute::from_binding(
        process_name,
        &binding,
        metadata,
    ));
    (
        lifecycle::AppLifecycleStatus::Ok,
        lifecycle::PermissionState::Granted,
        persisted.granted_values,
        handle,
    )
}

fn decode_permission_values(values: lifecycle::WireStringVector<'_>) -> Result<Vec<String>, ()> {
    let mut out = Vec::new();
    for index in 0..values.len() {
        let value = values.get(index).map_err(|_| ())?;
        if value.is_empty() || value.contains('|') || value.contains(';') {
            return Err(());
        }
        out.push(value.to_string());
    }
    Ok(out)
}

fn effective_permission_declarations(
    manifest: &Manifest,
    process: &crate::Process,
) -> Vec<PermissionDeclaration> {
    let mut declarations = manifest.permissions.clone();
    declarations.extend(process.permissions.iter().cloned());
    declarations
}

fn client_context_from_grants(
    package_name: &str,
    uid: u64,
    permissions: &MemoryPermissionStore,
) -> ClientContext {
    let grants = permissions.user_grants(uid, package_name);
    ClientContext {
        package_name: package_name.into(),
        permissions: grants
            .iter()
            .map(|grant| grant.permission_name.clone())
            .collect(),
        permission_values: grants
            .into_iter()
            .map(|grant| PermissionValueGrant {
                permission: grant.permission_name,
                values: grant.granted_values,
            })
            .collect(),
        is_foreground: true,
        user_id: Some(uid),
    }
}

fn prior_generation_seed(package_name: &str) -> u64 {
    package_name.len() as u64
}

fn user_is_unlocked(users: Channel, uid: u64) -> bool {
    let mut request_bytes = alloc::vec![0; 64];
    let request = user_manager::UserManagerGetUserRequest { uid };
    let encoded = match request.encode(&mut request_bytes, &mut []) {
        Ok(encoded) => encoded,
        Err(_) => return false,
    };
    let response = match Rpc(users).call_raw(2, &request_bytes[..encoded.bytes], &[], true) {
        Ok(response) => response,
        Err(_) => return false,
    };
    let handles = response
        .handles
        .iter()
        .map(|handle| user_manager::HandleRef { raw: *handle })
        .collect::<Vec<_>>();
    let Ok(response) = user_manager::UserManagerGetUserResponse::decode(&response.bytes, &handles)
    else {
        return false;
    };
    response.status == user_manager::UserStatus::Ok
        && response.user.uid == uid
        && response.user.unlocked
        && !response.user.disabled
}

fn ensure_user_state_watcher(state: &mut state::AppdState) {
    if state.user_watcher.0 != 0 || state.users.0 == 0 {
        return;
    }
    let Ok((watcher_client, watcher_server)) = Channel::pair() else {
        return;
    };
    let mut request_bytes = alloc::vec![0; 64];
    let mut request_handles = [user_manager::HandleRef { raw: 0 }; 1];
    let request = user_manager::UserManagerWatchUserEventsRequest {
        watcher: user_manager::HandleRef {
            raw: watcher_client.0,
        },
    };
    let Ok(encoded) = request.encode(&mut request_bytes, &mut request_handles) else {
        let _ = Memory::close(watcher_client.0);
        let _ = Memory::close(watcher_server.0);
        return;
    };
    let raw_handles = request_handles[..encoded.handles]
        .iter()
        .map(|handle| handle.raw)
        .collect::<Vec<_>>();
    let Ok(response) =
        Rpc(state.users).call_raw(8, &request_bytes[..encoded.bytes], &raw_handles, true)
    else {
        let _ = Memory::close(watcher_client.0);
        let _ = Memory::close(watcher_server.0);
        return;
    };
    let handles = response
        .handles
        .iter()
        .map(|handle| user_manager::HandleRef { raw: *handle })
        .collect::<Vec<_>>();
    let Ok(response) =
        user_manager::UserManagerWatchUserEventsResponse::decode(&response.bytes, &handles)
    else {
        let _ = Memory::close(watcher_client.0);
        let _ = Memory::close(watcher_server.0);
        return;
    };
    if response.status == user_manager::UserStatus::Ok {
        let _ = Memory::close(watcher_client.0);
        state.user_watcher = watcher_server;
    } else {
        let _ = Memory::close(watcher_client.0);
        let _ = Memory::close(watcher_server.0);
    }
}

fn poll_user_state_watcher(state: &mut state::AppdState) {
    if state.user_watcher.0 == 0 {
        ensure_user_state_watcher(state);
        return;
    }
    while let Ok(message) = state.user_watcher.try_recv() {
        let (ordinal, req) = envelope(&message.bytes);
        if ordinal != 1 {
            continue;
        }
        let handles = message
            .handles
            .iter()
            .map(|handle| user_manager::HandleRef { raw: *handle })
            .collect::<Vec<_>>();
        let Ok(event) =
            user_manager::UserStateWatcherOnUserStateChangedRequest::decode(req, &handles)
        else {
            continue;
        };
        match event.kind {
            user_manager::UserEventKind::Unlocked => {
                let _ = crate::permission_persistence::load_user(
                    state.vfsd,
                    event.uid,
                    &state.registry,
                    &mut state.permissions,
                );
            }
            user_manager::UserEventKind::Locked | user_manager::UserEventKind::Deleted => {
                shell::user_locked(state, event.uid);
                state.permissions.remove_user(event.uid);
                let removed = state.permission_routes.remove_uid(event.uid);
                close_permission_routes(removed);
            }
            _ => {}
        }
    }
}

fn lifecycle_info(record: &bexos_app_registry::AppRecord) -> lifecycle::AppInfo<'_> {
    lifecycle::AppInfo {
        package_id: &record.package_id,
        name: &record.display_name,
        state: if record.validate_architecture().is_err() {
            lifecycle::AppLifecycleState::Unavailable
        } else {
            match record.lifecycle_state {
                LifecycleState::Installed => lifecycle::AppLifecycleState::Installed,
                LifecycleState::Launching => lifecycle::AppLifecycleState::Launching,
                LifecycleState::Running => lifecycle::AppLifecycleState::Running,
                LifecycleState::Stopped => lifecycle::AppLifecycleState::Stopped,
            }
        },
        source: match record.install_source {
            InstallSource::Bootfs => lifecycle::AppInstallSource::Bootfs,
            InstallSource::SystemImage => lifecycle::AppInstallSource::SystemImage,
            InstallSource::Debugd => lifecycle::AppInstallSource::Debugd,
            InstallSource::Oci => lifecycle::AppInstallSource::Oci,
        },
        protected: record.protected,
    }
}

fn lifecycle_refs(hs: &[u64]) -> Vec<lifecycle::HandleRef> {
    hs.iter()
        .map(|h| lifecycle::HandleRef { raw: *h })
        .collect()
}

fn version_manager_refs(hs: &[u64]) -> Vec<version_manager::HandleRef> {
    hs.iter()
        .map(|h| version_manager::HandleRef { raw: *h })
        .collect()
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn lifecycle_reply<Q: LifecycleEncode>(channel: Channel, q: &Q) {
    let mut bytes = alloc::vec![0; 65500];
    let mut hs = [lifecycle::HandleRef { raw: 0 }; 16];
    let e = q.encode(&mut bytes, &mut hs).expect("app lifecycle encode");
    channel
        .send(
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .expect("app lifecycle reply");
}

fn version_manager_reply<Q: VersionEncode>(channel: Channel, q: &Q) {
    let mut bytes = alloc::vec![0; 65500];
    let mut hs = [version_manager::HandleRef { raw: 0 }; 16];
    let e = q
        .encode(&mut bytes, &mut hs)
        .expect("app version manager encode");
    channel
        .send(
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .expect("app version manager reply");
}
