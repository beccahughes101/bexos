mod command_tests;
mod deferred_tests;
mod firmware_policy_tests;
mod hardware_resource_tests;
mod launch_permission_tests;
mod lazy_tests;
mod migration_count_tests;
mod nix_tests;
mod wasm_tests;

use app_debug_fidl::{AppDebugControlListProcessesRequest, AppDebugControlPublicServer};
use app_lifecycle_fidl::{
    AppLifecycleControlRequestPermissionRequest, AppLifecycleControlRequestPermissionResponse,
    AppLifecycleStatus, FidlDecode, FidlEncode, HandleRef, PermissionState, WireStringVector,
};
use bexos_appd::{
    AppdBroker, AppdSnapshot, AppdWaveOrchestrator, BindBusType, BindCondition, BindError,
    BindProperty, BindRule, BoundCapability, BusType, CapabilityMetadata, ClientContext,
    ConsumedCapability, ConsumedService, DeviceNodeInfo, DeviceNodeState, DeviceProperty,
    DeviceRegistry, DeviceRegistryError, DriverExclusions, DriverIndex, DriverInfo,
    DriverRecoveryBudget, ElfError, ExposedService, FakeKernelOps, FidlCapability,
    HardwareAccessTier, HardwareResourceKind, HardwareResourceLease, ImmediateReadiness,
    InterfaceQuery, KernelHandle, KernelOperation, LaunchError, LaunchRequest, Lifecycle, LinkType,
    Manifest, Metadata, MethodDependency, MultiVersionPolicy, NetworkDomain, NetworkPolicy,
    PackageIdentity, PackageImage, PackageImageError, PackageImageResolver, PackageKind,
    PackageLibrary, PackageLibraryDependency, PackageLibraryKind, PackageTrustTier, ParsedElf,
    PermissionDecision, PermissionDeclaration, PermissionRequirement, PermissionRoute,
    PermissionRouteTable, PermissionValueGrant, PlatformConfig, ProcessRunnerOptions,
    ReadinessError, ReadinessGate, RecoveryDecision, RegisteredDeviceNode, RegistryError,
    ResourceGroup, RunnerPolicyDecision, RunnerRegistry, SYSTEM_PRIVILEGED_PERMISSION, SemVer,
    ServiceActivation, ServiceContract, SnapshotError, StartupClass, StartupError, Visibility,
    allowed_capabilities, publish_kernel_services,
};
use bexos_kernel_core::ipc::Capability;
use bexos_userspace::live_migration::State;

#[test]
fn lifecycle_callers_keep_distinct_reply_queues_across_handover() {
    use bexos_appd::guest::state::AppdState;
    use bexos_userspace::{Channel, live_migration::Resource};
    let mut source = AppdState::empty();
    source.lifecycle = Channel(71);
    source.updated_lifecycle = Channel(72);
    let bytes = source.encode_record(0).unwrap().unwrap();
    let mut target = AppdState::empty();
    target.adopt_record(0, Some(&bytes)).unwrap();
    assert_eq!(target.lifecycle.0, 71);
    assert_eq!(target.updated_lifecycle.0, 72);
    assert!(
        target
            .resources()
            .iter()
            .any(|r| matches!(r, Resource::Handle(71)))
    );
    assert!(
        target
            .resources()
            .iter()
            .any(|r| matches!(r, Resource::Handle(72)))
    );
}

const TEST_IMAGE_BASE: u64 = bexos_boot::USER_IMAGE_BASE;
const TEST_IMAGE_ENTRY: u64 = TEST_IMAGE_BASE + 0x1000;
const TEST_TLS_PHDR_VADDR: u64 = TEST_IMAGE_BASE + 0x0100_0000;
const TEST_TLS_BASE: u64 = 0xbe00_0000;
const TEST_LIBRARY_BASE: u64 = 0xc000_0000;

struct FailingBundleFetcher;

#[test]
fn network_domain_authorization_is_fail_closed() {
    let policy = NetworkPolicy {
        domains: vec![
            NetworkDomain {
                name: "system_default".into(),
                isolation_group: "system_default".into(),
                table_id: 0,
                system_default: true,
                authorized_packages: Vec::new(),
            },
            NetworkDomain {
                name: "corp".into(),
                isolation_group: "isolated".into(),
                table_id: 7,
                system_default: false,
                authorized_packages: vec!["bexos.app.allowed".into()],
            },
        ],
        ..NetworkPolicy::default()
    };

    assert!(
        policy
            .authorize_domain("bexos.app.any", "system_default")
            .is_some()
    );
    assert!(
        policy
            .authorize_domain("bexos.app.allowed", "corp")
            .is_some()
    );
    assert!(
        policy
            .authorize_domain("bexos.app.denied", "corp")
            .is_none()
    );
    assert!(
        policy
            .authorize_domain("bexos.app.allowed", "unknown")
            .is_none()
    );
}

impl bexos_appd::WellKnownFetcher for FailingBundleFetcher {
    fn fetch_well_known(&mut self, _domain: &str) -> Result<Vec<u8>, bexos_appd::WebInstallError> {
        Err(bexos_appd::WebInstallError::Network)
    }
}

impl bexos_appd::AppBundleFetcher for FailingBundleFetcher {
    fn fetch_app_bundle(&mut self, _url: &str) -> Result<Vec<u8>, bexos_appd::WebInstallError> {
        Err(bexos_appd::WebInstallError::Network)
    }
}

#[test]
fn appd_identity_is_canonical() {
    assert_eq!(
        bexos_boot::APPD_PATH,
        "/boot/pkg/bexos.platform.appd/bin/appd"
    );
    assert_eq!(
        bexos_appd::guest::state::APPD_PACKAGE,
        "bexos.platform.appd"
    );
}

#[test]
fn platform_config_decodes_lifecycle_policy_defaults_and_explicit_values() {
    let default_config = PlatformConfig::decode(&[]).unwrap();
    assert_eq!(
        default_config.app_lifecycle_policy.probation_window_seconds,
        60
    );
    assert_eq!(default_config.app_lifecycle_policy.crash_window_seconds, 60);
    assert_eq!(default_config.app_lifecycle_policy.crash_threshold, 3);
    assert_eq!(default_config.app_lifecycle_policy.restart_delay_seconds, 1);

    let lifecycle_policy = message(&[
        varint_field(1, 11),
        varint_field(2, 12),
        varint_field(3, 4),
        varint_field(4, 2),
    ]);
    let config = PlatformConfig::decode(&message(&[message_field(6, &lifecycle_policy)])).unwrap();

    assert_eq!(config.app_lifecycle_policy.probation_window_seconds, 11);
    assert_eq!(config.app_lifecycle_policy.crash_window_seconds, 12);
    assert_eq!(config.app_lifecycle_policy.crash_threshold, 4);
    assert_eq!(config.app_lifecycle_policy.restart_delay_seconds, 2);
}

#[test]
fn watchdog_does_not_restart_commands_or_session_managed_shells() {
    use bexos_appd::manifest::ShellRole;
    let mut manifest = bexos_appd::Manifest::default();
    manifest.processes.push(bexos_appd::Process {
        name: "main".into(),
        ..Default::default()
    });
    assert!(!bexos_appd::watchdog::automatic_restart(&manifest, "main"));
    manifest.processes[0].service = true;
    assert!(bexos_appd::watchdog::automatic_restart(&manifest, "main"));
    assert!(!bexos_appd::watchdog::automatic_restart(
        &manifest, "missing"
    ));
    for role in [ShellRole::System, ShellRole::User] {
        manifest.processes[0].shell_role = role;
        assert!(!bexos_appd::watchdog::automatic_restart(&manifest, "main"));
    }
}

#[test]
fn watchdog_promotes_after_probation_survival() {
    let policy = bexos_appd::AppLifecyclePolicy {
        probation_window_seconds: 60,
        crash_window_seconds: 60,
        crash_threshold: 3,
        restart_delay_seconds: 1,
    };
    let mut record = bexos_appd::watchdog::WatchdogRecord::new(
        "com.example:demo".into(),
        "main".into(),
        String::new(),
        1,
        semver(1, 0, 0),
        0,
    );

    assert_eq!(record.ready(59_000_000_000, policy), None);
    assert_eq!(
        record.ready(60_000_000_000, policy),
        Some(bexos_appd::watchdog::WatchdogAction::PromoteHealthy {
            package_id: "com.example:demo".into()
        })
    );
}

#[test]
fn watchdog_rolls_back_on_probation_exit_or_third_healthy_exit() {
    let policy = bexos_appd::AppLifecyclePolicy::default();
    let mut probation = bexos_appd::watchdog::WatchdogRecord::new(
        "com.example:demo".into(),
        "main".into(),
        String::new(),
        1,
        semver(1, 1, 0),
        0,
    );

    assert!(matches!(
        probation.exit(1_000_000_000, policy, true),
        bexos_appd::watchdog::WatchdogAction::RollbackAndRestart { .. }
    ));

    let mut healthy = bexos_appd::watchdog::WatchdogRecord::new(
        "com.example:demo".into(),
        "main".into(),
        String::new(),
        1,
        semver(1, 1, 0),
        0,
    );
    let _ = healthy.ready(60_000_000_000, policy);
    assert!(matches!(
        healthy.exit(61_000_000_000, policy, true),
        bexos_appd::watchdog::WatchdogAction::Restart { .. }
    ));
    assert!(matches!(
        healthy.exit(62_000_000_000, policy, true),
        bexos_appd::watchdog::WatchdogAction::Restart { .. }
    ));
    assert!(matches!(
        healthy.exit(63_000_000_000, policy, true),
        bexos_appd::watchdog::WatchdogAction::RollbackAndRestart { .. }
    ));
}

#[test]
fn watchdog_resets_counter_after_full_crash_free_window_and_checkpoints() {
    let policy = bexos_appd::AppLifecyclePolicy::default();
    let mut record = bexos_appd::watchdog::WatchdogRecord::new(
        "com.example:demo".into(),
        "main".into(),
        String::new(),
        1,
        semver(1, 1, 0),
        0,
    );
    let _ = record.ready(60_000_000_000, policy);
    let _ = record.exit(61_000_000_000, policy, false);
    assert!(matches!(
        record.exit(122_000_000_000, policy, false),
        bexos_appd::watchdog::WatchdogAction::Restart { .. }
    ));
    assert_eq!(record.crash_count, 1);

    let restored =
        bexos_appd::watchdog::WatchdogRecord::from_checkpoint(&record.checkpoint()).unwrap();
    assert_eq!(restored, record);
}

#[test]
fn appd_guest_entry_is_tokio_future() {
    let _future = bexos_appd::guest::main(0);
}

#[test]
fn appd_state_syncs_without_serialized_store_handles() {
    let state = bexos_appd::guest::state::AppdState::empty();

    state.sync_stores().expect("missing live stores is a no-op");
}

#[test]
fn app_lifecycle_permission_values_round_trip_as_string_vectors() {
    let requested = ["front", "back"];
    let request = AppLifecycleControlRequestPermissionRequest {
        package_id: "com.example.camera",
        process_name: "main",
        uid: 1000,
        permission_name: "CAMERA",
        service_name: "bexos.hardware.Camera",
        capability: "Camera",
        requested_values: WireStringVector::from_slice(&requested),
    };
    let mut bytes = [0u8; 512];
    let mut handles = [];
    let encoded = request.encode(&mut bytes, &mut handles).unwrap();
    let decoded =
        AppLifecycleControlRequestPermissionRequest::decode(&bytes[..encoded.bytes], &[]).unwrap();
    assert_eq!(decoded.requested_values.len(), 2);
    assert_eq!(decoded.service_name, "bexos.hardware.Camera");
    assert_eq!(decoded.capability, "Camera");
    assert_eq!(decoded.requested_values.get(0).unwrap(), "front");
    assert_eq!(decoded.requested_values.get(1).unwrap(), "back");

    let granted = ["front"];
    let response = AppLifecycleControlRequestPermissionResponse {
        status: AppLifecycleStatus::Ok,
        state: PermissionState::Granted,
        granted_values: WireStringVector::from_slice(&granted),
        granted_handle: HandleRef { raw: 99 },
    };
    let mut response_handles = [HandleRef { raw: 0 }; 1];
    let encoded = response.encode(&mut bytes, &mut response_handles).unwrap();
    let decoded = AppLifecycleControlRequestPermissionResponse::decode(
        &bytes[..encoded.bytes],
        &response_handles[..encoded.handles],
    )
    .unwrap();
    assert_eq!(decoded.granted_values.len(), 1);
    assert_eq!(decoded.granted_values.get(0).unwrap(), "front");
    assert_eq!(decoded.granted_handle.raw, 99);
}

#[test]
fn manifest_lifecycle_defaults_and_explicit_opt_in_are_checked() {
    use bexos_appd::{ManifestError, UpdateStrategy};
    let decode = |fields: Vec<u8>| {
        Manifest::decode(&message(&[message_field(
            3,
            &message(&[message_field(9, &fields)]),
        )]))
    };
    let default = decode(Vec::new()).unwrap();
    assert_eq!(
        default.processes[0].lifecycle.update_strategy,
        UpdateStrategy::Restart
    );
    assert_eq!(default.processes[0].lifecycle.migration_timeout_ms, 150);
    assert_eq!(
        default.processes[0].lifecycle.preparation_timeout_ms,
        30_000
    );
    let live = decode(message(&[
        varint_field(1, 1),
        varint_field(2, 250),
        varint_field(3, 12_000),
    ]))
    .unwrap();
    assert_eq!(
        live.processes[0].lifecycle.update_strategy,
        UpdateStrategy::HeartTransplant
    );
    assert_eq!(live.processes[0].lifecycle.migration_timeout_ms, 250);
    assert_eq!(live.processes[0].lifecycle.preparation_timeout_ms, 12_000);
    for fields in [
        message(&[varint_field(1, 99)]),
        message(&[varint_field(2, 0)]),
        message(&[varint_field(3, 1u64 << 32)]),
    ] {
        assert_eq!(decode(fields), Err(ManifestError::InvalidLifecycle));
    }
}

#[test]
fn manifest_decoder_reads_package_kind_and_library_dependencies() {
    let dependency = message(&[
        string_field(1, "bexos.lib.crypto"),
        string_field(2, "1"),
        string_field(3, "crypto"),
        varint_field(4, 1),
    ]);
    let export = message(&[
        string_field(1, "crypto"),
        string_field(2, "/pkg/lib/libbexos_crypto.so"),
        string_field(3, "bexos_crypto_"),
        varint_field(4, 1),
        string_field(5, "libbexos_crypto.so"),
    ]);
    let bytes = message(&[
        string_field(1, "bexos.service.keychaind"),
        varint_field(11, 0),
        message_field(12, &dependency),
        message_field(16, &export),
    ]);

    let manifest = Manifest::decode(&bytes).expect("manifest should decode");

    assert_eq!(manifest.package_kind, PackageKind::Application);
    assert_eq!(manifest.library_dependencies.len(), 1);
    assert_eq!(
        manifest.library_dependencies[0].package_name,
        "bexos.lib.crypto"
    );
    assert_eq!(
        manifest.library_dependencies[0]
            .version_requirement
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        manifest.library_dependencies[0].mount_alias.as_deref(),
        Some("crypto")
    );
    assert_eq!(manifest.library_dependencies[0].abi_version, 1);
    assert_eq!(manifest.library_exports.len(), 1);
    assert_eq!(manifest.library_exports[0].name, "crypto");
    assert_eq!(
        manifest.library_exports[0].path,
        "/pkg/lib/libbexos_crypto.so"
    );
    assert_eq!(manifest.library_exports[0].symbol_prefix, "bexos_crypto_");
    assert_eq!(manifest.library_exports[0].abi_version, 1);
    assert_eq!(manifest.library_exports[0].soname, "libbexos_crypto.so");
}

#[test]
fn manifest_decoder_reads_structured_version_metadata() {
    let semver = message(&[
        varint_field(1, 1),
        varint_field(2, 2),
        varint_field(3, 3),
        varint_field(4, 104),
        string_field(5, "rc.1"),
    ]);
    let bytes = message(&[
        string_field(1, "com.example:demo"),
        message_field(13, &semver),
        varint_field(14, 1),
        varint_field(15, 7),
    ]);

    let manifest = Manifest::decode(&bytes).expect("manifest should decode");

    assert_eq!(manifest.package_version.major, 1);
    assert_eq!(manifest.package_version.minor, 2);
    assert_eq!(manifest.package_version.patch, 3);
    assert_eq!(manifest.package_version.build, 104);
    assert_eq!(manifest.package_version.prerelease, "rc.1");
    assert_eq!(
        manifest.multi_version_policy,
        MultiVersionPolicy::ParallelExecution
    );
    assert_eq!(manifest.min_bexos_abi_version, 7);
}

#[test]
fn manifest_decoder_defaults_old_manifests_to_application() {
    let manifest =
        Manifest::decode(&message(&[string_field(1, "legacy")])).expect("manifest should decode");

    assert_eq!(manifest.package_kind, PackageKind::Application);
    assert!(manifest.library_dependencies.is_empty());
}

#[test]
fn app_storage_namespace_orders_pkg_then_data() {
    let namespace = bexos_appd::app_storage_namespace(
        KernelHandle { raw: 11 },
        KernelHandle { raw: 12 },
        KernelHandle { raw: 13 },
    )
    .expect("namespace should assemble");

    assert_eq!(namespace.paths(), "/pkg;/data;/tmp");
    assert_eq!(namespace.handles(), vec![11, 12, 13]);
    assert_eq!(namespace.entries()[0].path, "/pkg");
    assert_eq!(namespace.entries()[1].path, "/data");
    assert_eq!(namespace.entries()[2].path, "/tmp");
}

#[test]
fn app_storage_namespace_mounts_dependencies_after_data() {
    let dependencies = [
        bexos_appd::DependencyNamespaceEntry {
            package_name: "bexos.lib.crypto",
            mount_alias: None,
            directory: KernelHandle { raw: 13 },
        },
        bexos_appd::DependencyNamespaceEntry {
            package_name: "bexos.lib.ui",
            mount_alias: Some("ui"),
            directory: KernelHandle { raw: 14 },
        },
    ];
    let namespace = bexos_appd::app_storage_namespace_with_dependencies(
        KernelHandle { raw: 11 },
        KernelHandle { raw: 12 },
        KernelHandle { raw: 15 },
        &dependencies,
    )
    .expect("namespace should assemble");

    assert_eq!(
        namespace.paths(),
        "/pkg;/data;/tmp;/deps/bexos.lib.crypto;/deps/ui"
    );
    assert_eq!(namespace.handles(), vec![11, 12, 15, 13, 14]);
}

#[test]
fn app_storage_namespace_mounts_shared_vaults_before_dependencies() {
    let shared_vaults = [bexos_appd::SharedVaultNamespaceEntry {
        name: "google_shared_vault",
        directory: KernelHandle { raw: 14 },
    }];
    let dependencies = [bexos_appd::DependencyNamespaceEntry {
        package_name: "bexos.lib.crypto",
        mount_alias: None,
        directory: KernelHandle { raw: 15 },
    }];
    let namespace = bexos_appd::app_storage_namespace_with_shared_vaults_and_dependencies(
        KernelHandle { raw: 11 },
        KernelHandle { raw: 12 },
        KernelHandle { raw: 13 },
        &shared_vaults,
        &dependencies,
    )
    .expect("namespace should assemble");

    assert_eq!(
        namespace.paths(),
        "/pkg;/data;/tmp;/shared/google_shared_vault;/deps/bexos.lib.crypto"
    );
    assert_eq!(namespace.handles(), vec![11, 12, 13, 14, 15]);
}

#[test]
fn app_storage_namespace_rejects_path_like_shared_vault_names() {
    let shared_vaults = [bexos_appd::SharedVaultNamespaceEntry {
        name: "../escape",
        directory: KernelHandle { raw: 13 },
    }];

    assert_eq!(
        bexos_appd::app_storage_namespace_with_shared_vaults_and_dependencies(
            KernelHandle { raw: 11 },
            KernelHandle { raw: 12 },
            KernelHandle { raw: 13 },
            &shared_vaults,
            &[],
        ),
        Err(bexos_appd::NamespaceError::InvalidMountName(
            "../escape".to_string()
        ))
    );
}

#[test]
fn startup_namespace_rejects_missing_and_duplicate_roots() {
    let mut namespace = bexos_appd::StartupNamespace::new();
    assert_eq!(
        namespace.push("/pkg", KernelHandle::none()),
        Err(bexos_appd::NamespaceError::MissingDirectory(
            "/pkg".to_string()
        ))
    );

    namespace
        .push("/pkg", KernelHandle { raw: 1 })
        .expect("first path should be accepted");
    assert_eq!(
        namespace.push("/pkg", KernelHandle { raw: 2 }),
        Err(bexos_appd::NamespaceError::DuplicatePath(
            "/pkg".to_string()
        ))
    );
}

#[test]
fn manifest_decoder_reads_shared_vaults() {
    let vault = message(&[
        string_field(1, "google_shared_vault"),
        varint_field(2, 1),
        string_field(3, "com.google"),
        string_field(3, "com.waymo"),
    ]);
    let manifest = Manifest::decode(&message(&[message_field(18, &vault)]))
        .expect("shared vault manifest should decode");

    assert_eq!(manifest.shared_vaults.len(), 1);
    assert_eq!(manifest.shared_vaults[0].name, "google_shared_vault");
    assert_eq!(
        manifest.shared_vaults[0].access,
        bexos_appd::SharedVaultAccess::ReadWrite
    );
    assert_eq!(
        manifest.shared_vaults[0].allowed_peer_package_prefixes,
        ["com.google", "com.waymo"]
    );
}

#[test]
fn shared_vault_policy_rejects_system_launch_with_user_peer() {
    let mut manifest = launch_manifest();
    manifest.package_name = "system:settings".to_string();
    manifest.shared_vaults = vec![bexos_appd::SharedVault {
        name: "settings_bridge".to_string(),
        access: bexos_appd::SharedVaultAccess::ReadWrite,
        allowed_peer_package_prefixes: vec!["user:settings".to_string()],
    }];
    let registry = bexos_appd::MemoryAppRegistry::new();

    assert_eq!(
        bexos_appd::guest::shared_vault_allowed_for_runtime(
            &registry,
            "system:settings",
            &manifest,
            bexos_appd::SYSTEM_UID,
            &manifest.shared_vaults[0],
        ),
        Err(fs_fidl::FsStatus::AccessDenied)
    );
}

#[test]
fn shared_vault_policy_allows_user_scoped_system_user_bridge() {
    let system_manifest =
        shared_vault_manifest_bytes("system:settings", "settings_bridge", &["user:settings"]);
    let user_manifest =
        shared_vault_manifest_bytes("user:settings", "settings_bridge", &["system:settings"]);
    let mut registry = bexos_appd::MemoryAppRegistry::new();
    registry
        .import_manifest_cache(
            &system_manifest,
            bexos_appd::InstallSource::Debugd,
            false,
            "pkg/system-settings.bex",
        )
        .expect("system manifest import");
    registry
        .import_manifest_cache(
            &user_manifest,
            bexos_appd::InstallSource::Debugd,
            false,
            "pkg/user-settings.bex",
        )
        .expect("user manifest import");
    let manifest = Manifest::decode(&system_manifest).expect("manifest decode");

    assert_eq!(
        bexos_appd::guest::shared_vault_allowed_for_runtime(
            &registry,
            "system:settings",
            &manifest,
            1000,
            &manifest.shared_vaults[0],
        ),
        Ok(())
    );
}

#[test]
fn shared_vault_domain_storage_requires_bidirectional_peer_trust() {
    let mut cache = bexos_appd::MemoryDomainAssociationCache::new();
    cache
        .upsert(
            "google.com",
            bexos_appd::DomainPolicyRecord {
                origin: "https://google.com".to_string(),
                allowed_package_prefixes: vec!["com.google".to_string()],
                trusted_distribution_origins: Vec::new(),
                trusted_peer_domains: vec!["https://waymo.com".to_string()],
                signing_certificate_fingerprints: vec![
                    "SHA256:00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
                        .to_string(),
                ],
                fetched_timestamp: 1,
                ttl_seconds: 60,
            },
        )
        .expect("google policy");

    assert_eq!(
        bexos_appd::guest::shared_vault_storage_domain(
            &cache,
            "com.google:maps",
            &["com.waymo".to_string()],
        ),
        Err(fs_fidl::FsStatus::AccessDenied)
    );

    cache
        .upsert(
            "waymo.com",
            bexos_appd::DomainPolicyRecord {
                origin: "https://waymo.com".to_string(),
                allowed_package_prefixes: vec!["com.waymo".to_string()],
                trusted_distribution_origins: Vec::new(),
                trusted_peer_domains: vec!["https://google.com".to_string()],
                signing_certificate_fingerprints: vec![
                    "SHA256:ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100"
                        .to_string(),
                ],
                fetched_timestamp: 1,
                ttl_seconds: 60,
            },
        )
        .expect("waymo policy");

    assert_eq!(
        bexos_appd::guest::shared_vault_storage_domain(
            &cache,
            "com.waymo:rider",
            &["com.google".to_string()],
        ),
        Ok(Some("google.com".to_string()))
    );
}

#[test]
fn manifest_decoder_reads_binary_protobuf_manifest() {
    let service = message(&[
        string_field(1, "bexos.hardware.Camera"),
        string_field(2, "CameraController"),
        varint_field(3, 1),
        varint_field(4, 1),
        string_field(5, "CAMERA"),
        message_field(
            6,
            &message(&[string_field(1, "supports_capture"), string_field(2, "true")]),
        ),
        message_field(
            7,
            &message(&[
                string_field(1, "Camera"),
                string_field(2, "CAMERA"),
                varint_field(3, 99),
            ]),
        ),
    ]);
    let process = message(&[
        string_field(1, "camera_service"),
        string_field(2, "elf"),
        message_field(3, &permission("CAMERA", &["front"], 0)),
        varint_field(4, 1),
        string_field(10, "foreground"),
    ]);
    let resource_group = message(&[
        string_field(1, "camera_foreground"),
        varint_field(2, 768),
        varint_field(3, 4096),
    ]);
    let bytes = message(&[
        string_field(1, "bexos.hardware:camera"),
        string_field(2, "Camera Provider"),
        message_field(3, &process),
        message_field(4, &permission("CAMERA", &["front"], 0)),
        message_field(5, &service),
        message_field(7, &resource_group),
    ]);

    let manifest = Manifest::decode(&bytes).expect("manifest should decode");

    assert_eq!(manifest.package_name, "bexos.hardware:camera");
    assert_eq!(manifest.processes[0].name, "camera_service");
    assert!(manifest.processes[0].service);
    assert_eq!(manifest.processes[0].wave, None);
    assert_eq!(
        manifest.processes[0].resource_group,
        Some("foreground".to_string())
    );
    assert_eq!(
        manifest.permissions,
        [PermissionDeclaration {
            name: "CAMERA".to_string(),
            values: vec!["front".to_string()],
            requirement: PermissionRequirement::Required,
            usage_description: String::new(),
        }]
    );
    assert_eq!(
        manifest.resource_groups[0],
        ResourceGroup {
            name: "camera_foreground".to_string(),
            cpu_shares: 768,
            memory_limit_pages: 4096,
            ..Default::default()
        }
    );
    assert_eq!(manifest.services_exposed[0].lifecycle, Lifecycle::Singleton);
    assert_eq!(manifest.services_exposed[0].visibility, Visibility::Public);
    assert_eq!(
        manifest.services_exposed[0].metadata[0],
        Metadata {
            key: "supports_capture".to_string(),
            value: "true".to_string()
        }
    );
    assert_eq!(
        manifest.services_exposed[0].capabilities[0],
        CapabilityMetadata {
            capability: "Camera".to_string(),
            permission: Some("CAMERA".to_string()),
            method_ordinals: vec![99],
        }
    );
}

#[test]
fn manifest_decoder_reads_process_intent_filters() {
    let intent_a = message(&[
        string_field(1, "mailto"),
        string_field(2, "example.com"),
        string_field(3, "application/pdf"),
        string_field(4, "bexos.ui.WebViewEngine"),
    ]);
    let intent_b = message(&[string_field(1, "bexos-demo"), string_field(3, "image/*")]);
    let process = message(&[
        string_field(1, "viewer"),
        string_field(2, "elf"),
        message_field(11, &intent_a),
        message_field(11, &intent_b),
    ]);
    let bytes = message(&[
        string_field(1, "com.example:viewer"),
        string_field(2, "Viewer"),
        message_field(3, &process),
    ]);

    let manifest = Manifest::decode(&bytes).expect("intent filters should decode");

    assert_eq!(manifest.processes[0].handles.len(), 2);
    assert_eq!(manifest.processes[0].handles[0].schemes, ["mailto"]);
    assert_eq!(manifest.processes[0].handles[0].domains, ["example.com"]);
    assert_eq!(
        manifest.processes[0].handles[0].mime_types,
        ["application/pdf"]
    );
    assert_eq!(
        manifest.processes[0].handles[0].provides_interfaces,
        ["bexos.ui.WebViewEngine"]
    );
    assert_eq!(manifest.processes[0].handles[1].mime_types, ["image/*"]);
}

#[test]
fn register_manifest_openers_resolves_manifest_handlers() {
    let mut manifest = launch_manifest();
    manifest.package_name = "com.example:viewer".to_string();
    manifest.processes[0].name = "viewer".to_string();
    manifest.processes[0].handles = vec![bexos_appd::IntentFilter {
        schemes: vec!["mailto".to_string()],
        domains: vec!["example.com".to_string()],
        mime_types: vec!["application/pdf".to_string(), "image/*".to_string()],
        provides_interfaces: vec!["bexos.ui.WebViewEngine".to_string()],
    }];
    let mut openers = bexos_appd::MemoryOpenerRegistry::new();

    bexos_appd::register_manifest_openers(
        &mut openers,
        bexos_appd::OpenerScope::System,
        &manifest,
        false,
    );

    assert!(matches!(
        openers.resolve(
            bexos_appd::OpenerScope::System,
            bexos_appd::OpenKind::Url("mailto:becca@example.com")
        ),
        bexos_appd::ResolveOutcome::Selected(_)
    ));
    assert!(matches!(
        openers.resolve(
            bexos_appd::OpenerScope::System,
            bexos_appd::OpenKind::Mime("image/png")
        ),
        bexos_appd::ResolveOutcome::Selected(_)
    ));
    assert_eq!(
        openers.resolve(
            bexos_appd::OpenerScope::System,
            bexos_appd::OpenKind::Url("https://example.com/doc")
        ),
        bexos_appd::ResolveOutcome::NoHandler
    );
}

#[test]
fn manifest_decoder_reads_structured_component_config_schema() {
    let default_channel = message(&[string_field(4, "stable")]);
    let default_retry = message(&[varint_field(2, 3)]);
    let default_bytes = message(&[bytes_field(5, b"seed")]);
    let schema = message(&[
        message_field(
            1,
            &message(&[
                string_field(1, "channel"),
                varint_field(2, 4),
                varint_field(3, 1),
                message_field(4, &default_channel),
                varint_field(5, 16),
            ]),
        ),
        message_field(
            1,
            &message(&[
                string_field(1, "retry_limit"),
                varint_field(2, 2),
                varint_field(3, 1),
                message_field(4, &default_retry),
            ]),
        ),
        message_field(
            1,
            &message(&[
                string_field(1, "seed"),
                varint_field(2, 5),
                message_field(4, &default_bytes),
                varint_field(5, 8),
            ]),
        ),
    ]);
    let bytes = message(&[
        string_field(1, "bexos.platform.storage_verify"),
        message_field(10, &schema),
    ]);

    let manifest = Manifest::decode(&bytes).expect("manifest should decode config schema");

    assert_eq!(manifest.config_schema.fields.len(), 3);
    assert_eq!(manifest.config_schema.fields[0].name, "channel");
    assert_eq!(
        manifest.config_schema.fields[0].config_type,
        bexos_appd::ComponentConfigType::String
    );
    assert_eq!(manifest.config_schema.fields[0].max_size, 16);
    assert_eq!(
        manifest.config_schema.fields[0].default_value,
        Some(bexos_appd::ComponentConfigValue::String(
            "stable".to_string()
        ))
    );
    assert_eq!(
        manifest.config_schema.fields[1].default_value,
        Some(bexos_appd::ComponentConfigValue::Uint32(3))
    );
    assert_eq!(
        manifest.config_schema.fields[2].default_value,
        Some(bexos_appd::ComponentConfigValue::Bytes(b"seed".to_vec()))
    );
}

#[test]
fn manifest_decoder_reads_driver_info_and_bind_rules() {
    let driver_info = message(&[
        string_field(1, "nvme_d1_driver"),
        string_field(2, "bexos.driver.storage.nvme"),
        string_field(3, "1.2.0"),
    ]);
    let bind_condition = message(&[
        varint_field(1, 1),
        message_field(
            2,
            &message(&[string_field(1, "pci.class"), varint_field(2, 0x01)]),
        ),
        message_field(
            2,
            &message(&[string_field(1, "pci.subclass"), varint_field(2, 0x08)]),
        ),
        message_field(
            2,
            &message(&[string_field(1, "pci.prog_if"), varint_field(2, 0x02)]),
        ),
    ]);
    let bytes = message(&[
        string_field(1, "bexos.driver.storage.nvme"),
        message_field(8, &driver_info),
        message_field(
            9,
            &message(&[message_field(1, &bind_condition), varint_field(2, 7)]),
        ),
    ]);

    let manifest = Manifest::decode(&bytes).expect("driver manifest should decode");

    assert_eq!(
        manifest.driver_info,
        Some(DriverInfo {
            name: "nvme_d1_driver".to_string(),
            package_id: "bexos.driver.storage.nvme".to_string(),
            version: "1.2.0".to_string(),
            ..Default::default()
        })
    );
    assert_eq!(manifest.bind_rules.len(), 1);
    assert_eq!(manifest.bind_rules[0].priority, 7);
    assert_eq!(manifest.bind_rules[0].conditions[0].bus, BindBusType::Pci);
    assert_eq!(
        manifest.bind_rules[0].conditions[0].properties[2],
        BindProperty {
            key: "pci.prog_if".to_string(),
            value: 0x02,
        }
    );
}

#[test]
fn manifest_decoder_accepts_packed_capability_ordinals() {
    let service = message(&[
        string_field(1, "bexos.power.PowerManager"),
        string_field(2, "PowerManager"),
        varint_field(3, 1),
        varint_field(4, 1),
        message_field(
            7,
            &message(&[string_field(1, "Public"), bytes_field(3, &[1, 2])]),
        ),
    ]);
    let bytes = message(&[
        string_field(1, "bexos.service.powerd"),
        message_field(5, &service),
    ]);

    let manifest = Manifest::decode(&bytes).expect("packed ordinals should decode");

    assert_eq!(
        manifest.services_exposed[0].capabilities[0].method_ordinals,
        [1, 2]
    );
}

#[test]
fn manifest_decoder_reads_consumed_capability_scope() {
    let consumed = message(&[
        string_field(1, "bexos.hardware.Camera"),
        varint_field(2, 1),
        message_field(
            4,
            &message(&[
                string_field(1, "Camera"),
                message_field(2, &message(&[varint_field(1, 2), varint_field(2, 1)])),
                message_field(2, &message(&[varint_field(1, 3), varint_field(2, 2)])),
            ]),
        ),
    ]);
    let bytes = message(&[
        string_field(1, "com.example.camera"),
        message_field(6, &consumed),
    ]);

    let manifest = Manifest::decode(&bytes).expect("manifest should decode consumed scope");

    assert_eq!(manifest.services_consumed.len(), 1);
    assert_eq!(manifest.services_consumed[0].capabilities.len(), 1);
    assert_eq!(
        manifest.services_consumed[0].capabilities[0].capability,
        "Camera"
    );
    assert_eq!(
        manifest.services_consumed[0].capabilities[0].methods,
        [
            MethodDependency {
                ordinal: 2,
                link_type: LinkType::Required,
            },
            MethodDependency {
                ordinal: 3,
                link_type: LinkType::Optional,
            }
        ]
    );
}

#[test]
fn broker_intersects_declared_consumed_capability_scope() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            ExposedService {
                capabilities: vec![CapabilityMetadata {
                    capability: "Camera".to_string(),
                    permission: Some("CAMERA".to_string()),
                    method_ordinals: vec![2, 3, 4],
                }],
                ..camera_service(None, Lifecycle::Singleton)
            },
            Capability {
                object_id: 44,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let mut kernel = FakeKernelOps::new();
    let bindings = broker
        .bind_consumed_service(
            &client(&["CAMERA"], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "Camera".to_string(),
                    methods: vec![
                        MethodDependency {
                            ordinal: 2,
                            link_type: LinkType::Required,
                        },
                        MethodDependency {
                            ordinal: 4,
                            link_type: LinkType::Required,
                        },
                    ],
                }],
            },
            &mut kernel,
        )
        .expect("scoped camera capability should bind");

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].capability, "Camera");
    assert_eq!(bindings[0].method_ordinals, vec![2, 4]);
}

#[test]
fn broker_binds_exact_consumed_capability_selector() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            ExposedService {
                capabilities: vec![CapabilityMetadata {
                    capability: "Camera".to_string(),
                    permission: Some("CAMERA".to_string()),
                    method_ordinals: vec![2, 3, 4],
                }],
                ..camera_service(None, Lifecycle::Singleton)
            },
            Capability {
                object_id: 44,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let mut kernel = FakeKernelOps::new();
    let binding = broker
        .bind_consumed_capability(
            &client(&["CAMERA"], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "Camera".to_string(),
                    methods: vec![MethodDependency {
                        ordinal: 2,
                        link_type: LinkType::Required,
                    }],
                }],
            },
            "Camera",
            &mut kernel,
        )
        .expect("exact camera selector should bind");

    assert_eq!(binding.service_name, "bexos.hardware.Camera");
    assert_eq!(binding.capability, "Camera");
    assert_eq!(binding.permission.as_deref(), Some("CAMERA"));
    assert_eq!(binding.method_ordinals, vec![2]);
    assert_ne!(binding.client_endpoint.object_id, 0);
}

#[test]
fn broker_reports_ambiguous_exact_consumed_capability_selector() {
    let mut broker = AppdBroker::new();
    for provider in ["bexos.hardware:camera0", "bexos.hardware:camera1"] {
        broker
            .publish_interface(
                provider,
                ExposedService {
                    capabilities: vec![CapabilityMetadata {
                        capability: "Camera".to_string(),
                        permission: Some("CAMERA".to_string()),
                        method_ordinals: vec![2],
                    }],
                    ..camera_service(None, Lifecycle::MultipleInstance)
                },
                Capability {
                    object_id: if provider.ends_with('0') { 44 } else { 45 },
                    rights: 0b11,
                },
            )
            .expect("publish should succeed");
    }

    let mut kernel = FakeKernelOps::new();
    let err = broker
        .bind_consumed_capability(
            &client(&["CAMERA"], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "Camera".to_string(),
                    methods: vec![MethodDependency {
                        ordinal: 2,
                        link_type: LinkType::Required,
                    }],
                }],
            },
            "Camera",
            &mut kernel,
        )
        .unwrap_err();

    assert_eq!(err, BindError::Ambiguous);
}

#[test]
fn permission_routes_replay_only_compatible_provider_replacements() {
    let mut table = PermissionRouteTable::new();
    table.retain(PermissionRoute {
        caller_package: "com.example:client".to_string(),
        caller_process: "default".to_string(),
        caller_uid: 1000,
        provider_package: "bexos.hardware:camera".to_string(),
        provider_instance_id: None,
        service_name: "bexos.hardware.Camera".to_string(),
        protocol: "CameraController".to_string(),
        capability: "Camera".to_string(),
        permission: Some("CAMERA".to_string()),
        method_ordinals: vec![2],
        granted_values: vec!["front".to_string()],
        metadata: b"metadata".to_vec(),
        retained_endpoint: Capability {
            object_id: 10,
            rights: 0b11,
        },
        client_endpoint: Capability {
            object_id: 11,
            rights: 0b11,
        },
        provider_manager: Capability {
            object_id: 44,
            rights: 0b11,
        },
    });

    let compatible = Manifest {
        architecture: bexos_app_manifest::Architecture::current_guest(),
        package_name: "bexos.hardware:camera".to_string(),
        services_exposed: vec![ExposedService {
            name: "bexos.hardware.Camera".to_string(),
            protocol: "CameraController".to_string(),
            capabilities: vec![CapabilityMetadata {
                capability: "Camera".to_string(),
                permission: Some("CAMERA".to_string()),
                method_ordinals: vec![2, 3],
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        table
            .take_incompatible_provider("bexos.hardware:camera", &compatible)
            .is_empty()
    );
    assert_eq!(
        table
            .replayable_for_provider(
                "bexos.hardware:camera",
                Capability {
                    object_id: 45,
                    rights: 0b11,
                },
            )
            .len(),
        1
    );

    let incompatible = Manifest {
        architecture: bexos_app_manifest::Architecture::current_guest(),
        package_name: "bexos.hardware:camera".to_string(),
        services_exposed: vec![ExposedService {
            name: "bexos.hardware.Camera".to_string(),
            protocol: "CameraController".to_string(),
            capabilities: vec![CapabilityMetadata {
                capability: "Camera".to_string(),
                permission: Some("DIFFERENT".to_string()),
                method_ordinals: vec![2],
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let removed = table.take_incompatible_provider("bexos.hardware:camera", &incompatible);
    assert_eq!(removed.len(), 1);
    assert!(table.routes().is_empty());
}

#[test]
fn broker_rejects_unknown_consumed_capability_scope() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            ExposedService {
                capabilities: vec![CapabilityMetadata {
                    capability: "Camera".to_string(),
                    permission: None,
                    method_ordinals: vec![2],
                }],
                ..camera_service(None, Lifecycle::Singleton)
            },
            Capability {
                object_id: 44,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let mut kernel = FakeKernelOps::new();
    assert_eq!(
        broker.bind_consumed_service(
            &client(&[], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "Camera".to_string(),
                    methods: vec![MethodDependency {
                        ordinal: 99,
                        link_type: LinkType::Required,
                    }],
                }],
            },
            &mut kernel,
        ),
        Err(BindError::InvalidCapability)
    );
    assert!(kernel.operations.is_empty());
}

#[test]
fn broker_drops_unknown_optional_consumed_methods() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            ExposedService {
                capabilities: vec![CapabilityMetadata {
                    capability: "Camera".to_string(),
                    permission: None,
                    method_ordinals: vec![2, 4],
                }],
                ..camera_service(None, Lifecycle::Singleton)
            },
            Capability {
                object_id: 44,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let mut kernel = FakeKernelOps::new();
    let bindings = broker
        .bind_consumed_service(
            &client(&[], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "Camera".to_string(),
                    methods: vec![
                        MethodDependency {
                            ordinal: 2,
                            link_type: LinkType::Required,
                        },
                        MethodDependency {
                            ordinal: 99,
                            link_type: LinkType::Optional,
                        },
                    ],
                }],
            },
            &mut kernel,
        )
        .expect("optional unknown method should be dropped");

    assert_eq!(bindings[0].method_ordinals, vec![2]);
}

#[test]
fn broker_does_not_mint_channel_for_empty_consumed_method_scope() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            ExposedService {
                capabilities: vec![CapabilityMetadata {
                    capability: "Camera".to_string(),
                    permission: None,
                    method_ordinals: vec![2],
                }],
                ..camera_service(None, Lifecycle::Singleton)
            },
            Capability {
                object_id: 44,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let mut kernel = FakeKernelOps::new();
    assert_eq!(
        broker.bind_consumed_service(
            &client(&[], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "Camera".to_string(),
                    methods: Vec::new(),
                }],
            },
            &mut kernel,
        ),
        Err(BindError::PermissionDenied)
    );
    assert!(kernel.operations.is_empty());
}

#[test]
fn device_registry_registers_and_finds_pci_nodes() {
    let mut registry = DeviceRegistry::new();
    registry
        .register_device_node(nvme_device_node(7))
        .expect("valid node should register");

    let matches = registry.find_by_property("pci.class", 0x01);

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].info.node_id, 7);
    assert_eq!(registry.nodes().len(), 1);
}

#[test]
fn device_registry_wire_request_round_trips_nested_resource_handles() {
    use hardware_manager_fidl::{
        BusType, DeviceNodeInfo, DeviceProperty, DeviceRegistryRegisterDeviceNodeRequest,
        FidlDecode, FidlEncode, HandleRef, HardwareResource, HardwareResourceKind, WireVector,
    };

    let properties = [
        DeviceProperty {
            key: "pci.bus",
            value: 0,
        },
        DeviceProperty {
            key: "pci.device",
            value: 0,
        },
        DeviceProperty {
            key: "pci.function",
            value: 0,
        },
        DeviceProperty {
            key: "pci.class",
            value: 0x06,
        },
    ];
    let resources = [HardwareResource {
        kind: HardwareResourceKind::Mmio,
        resource_id: 7,
        base: 0x1000,
        length: 0x1000,
        flags: 0,
        resource: HandleRef { raw: 77 },
    }];
    let request = DeviceRegistryRegisterDeviceNodeRequest {
        info: DeviceNodeInfo {
            node_id: 0,
            bus: BusType::Pci,
            has_parent: false,
            parent_node_id: 0,
            topological_path: "pci/0000:00:00.0",
            properties: WireVector::from_slice(&properties),
        },
        resources: WireVector::from_slice(&resources),
    };
    let mut bytes = [0; 2048];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let encoded = request.encode(&mut bytes, &mut handles).unwrap();
    let decoded = DeviceRegistryRegisterDeviceNodeRequest::decode(
        &bytes[..encoded.bytes],
        &handles[..encoded.handles],
    )
    .unwrap();

    assert_eq!(decoded.info.node_id, 0);
    assert_eq!(decoded.info.properties.len(), properties.len());
    assert_eq!(decoded.info.properties.get(3).unwrap().value, 0x06);
    assert_eq!(encoded.handles, 1);
    assert_eq!(decoded.resources.get(0).unwrap().resource.raw, 77);
}

#[test]
fn device_registry_wire_request_accepts_i2c_and_spi_bus_types() {
    use hardware_manager_fidl::{
        BusType as WireBusType, DeviceNodeInfo as WireNodeInfo, DeviceProperty as WireProperty,
        DeviceRegistryRegisterDeviceNodeRequest, FidlDecode, FidlEncode, HandleRef,
        HardwareResource, HardwareResourceKind, WireVector,
    };

    for (wire_bus, expected_bus, key, node_id) in [
        (WireBusType::I2c, BusType::I2c, "i2c.address", 701),
        (WireBusType::Spi, BusType::Spi, "spi.chip_select", 702),
    ] {
        let properties = [WireProperty { key, value: 1 }];
        let resources = [HardwareResource {
            kind: HardwareResourceKind::BusControl,
            resource_id: 1,
            base: 1,
            length: 1,
            flags: 0,
            resource: HandleRef { raw: 99 },
        }];
        let request = DeviceRegistryRegisterDeviceNodeRequest {
            info: WireNodeInfo {
                node_id,
                bus: wire_bus,
                has_parent: false,
                parent_node_id: 0,
                topological_path: if wire_bus == WireBusType::I2c {
                    "i2c/controller-701"
                } else {
                    "spi/controller-702"
                },
                properties: WireVector::from_slice(&properties),
            },
            resources: WireVector::from_slice(&resources),
        };
        let mut bytes = [0; 512];
        let mut handles = [HandleRef { raw: 0 }; 1];
        let encoded = request.encode(&mut bytes, &mut handles).unwrap();
        let decoded = DeviceRegistryRegisterDeviceNodeRequest::decode(
            &bytes[..encoded.bytes],
            &handles[..encoded.handles],
        )
        .unwrap();
        let mut registry = DeviceRegistry::new();
        registry
            .register_device_node(RegisteredDeviceNode {
                info: DeviceNodeInfo {
                    node_id: decoded.info.node_id,
                    bus: expected_bus,
                    parent_node_id: None,
                    topological_path: decoded.info.topological_path.to_string(),
                    properties: vec![DeviceProperty {
                        key: decoded.info.properties.get(0).unwrap().key.to_string(),
                        value: decoded.info.properties.get(0).unwrap().value,
                    }],
                },
                resources: vec![HardwareResourceLease {
                    kind: bexos_appd::HardwareResourceKind::BusControl,
                    resource_id: decoded.resources.get(0).unwrap().resource_id,
                    base: decoded.resources.get(0).unwrap().base,
                    length: decoded.resources.get(0).unwrap().length,
                    flags: decoded.resources.get(0).unwrap().flags,
                    capability: Capability {
                        object_id: decoded.resources.get(0).unwrap().resource.raw,
                        rights: 1,
                    },
                }],
                mmio_vmo: None,
                irq_channel: None,
                registrar: None,
                present: true,
                state: Default::default(),
            })
            .unwrap();
        assert_eq!(registry.nodes()[0].info.bus, expected_bus);
    }
}

#[test]
fn device_registry_rejects_duplicate_node_ids() {
    let mut registry = DeviceRegistry::new();
    registry
        .register_device_node(nvme_device_node(7))
        .expect("first node should register");

    assert_eq!(
        registry.register_device_node(nvme_device_node(7)),
        Err(DeviceRegistryError::DuplicateNode(7))
    );
}

#[test]
fn device_registry_unregister_removes_node() {
    let mut registry = DeviceRegistry::new();
    registry
        .register_device_node(nvme_device_node(7))
        .expect("node should register");

    let removed = registry
        .unregister_device_node(7)
        .expect("node should unregister");

    assert_eq!(removed.info.node_id, 7);
    assert!(registry.nodes().is_empty());
    assert_eq!(
        registry.unregister_device_node(7),
        Err(DeviceRegistryError::NotFound(7))
    );
}

#[test]
fn device_registry_replace_nodes_preserves_snapshot_shape() {
    let mut registry = DeviceRegistry::new();
    registry
        .replace_nodes(vec![nvme_device_node(7), nvme_device_node(8)])
        .expect("replacement nodes should validate");

    assert_eq!(registry.nodes().len(), 2);
    assert_eq!(registry.find_by_property("pci.prog_if", 0x02).len(), 2);
}

#[test]
fn device_registry_tracks_bind_active_and_unregister_lifecycle() {
    let mut registry = DeviceRegistry::new();
    registry
        .register_device_node(nvme_device_node(7))
        .expect("node should register");

    registry
        .begin_binding(
            7,
            "bexos.driver.storage.nvme".to_string(),
            "nvme_driver".to_string(),
        )
        .expect("binding should begin");
    assert!(matches!(
        registry.nodes()[0].state,
        DeviceNodeState::Binding(_)
    ));

    registry
        .bind_active(
            7,
            Some(Capability {
                object_id: 44,
                rights: 0,
            }),
            Some(Capability {
                object_id: 45,
                rights: 0,
            }),
            None,
        )
        .expect("binding should activate");
    assert!(matches!(
        registry.nodes()[0].state,
        DeviceNodeState::Active(_)
    ));

    registry
        .begin_quiescing(7)
        .expect("active binding can quiesce");
    assert!(matches!(
        registry.nodes()[0].state,
        DeviceNodeState::Quiescing(_)
    ));
    let removed = registry
        .unregister_device_node(7)
        .expect("quiescing node should unregister");
    assert_eq!(removed.info.node_id, 7);
}

#[test]
fn driver_index_matches_nvme_and_prefers_vendor_device_specific_driver() {
    let generic = nvme_driver_manifest("bexos.driver.storage.generic_nvme", 0, false);
    let specific = nvme_driver_manifest("bexos.driver.storage.vendor_nvme", 0, true);
    let manifests = vec![generic, specific];
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let node = nvme_device_node(7);

    let candidate = DriverIndex::build(&manifests)
        .best_match(&node.info, &config.driver_policy, |manifest| {
            PackageIdentity {
                package_id: &manifest.package_name,
                signer: "bexos_official_platform_v1",
                trust_tier: PackageTrustTier::SystemHardware,
                is_driver: true,
            }
        })
        .expect("driver should match");

    assert_eq!(
        candidate.manifest.package_name,
        "bexos.driver.storage.vendor_nvme"
    );
    assert_eq!(candidate.hardware_access, HardwareAccessTier::Isolated);
}

#[test]
fn driver_index_matches_i2c_and_spi_bind_rules() {
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let i2c = peripheral_driver_manifest(
        "bexos.driver.test_i2c",
        BindBusType::I2c,
        "i2c.address",
        0x48,
    );
    let spi = peripheral_driver_manifest(
        "bexos.driver.test_spi",
        BindBusType::Spi,
        "spi.chip_select",
        1,
    );
    let manifests = vec![i2c, spi];
    let i2c_node = RegisteredDeviceNode {
        info: DeviceNodeInfo {
            node_id: 4097,
            bus: BusType::I2c,
            parent_node_id: Some(4096),
            topological_path: "i2c/controller-4096/device-4097".to_string(),
            properties: vec![DeviceProperty {
                key: "i2c.address".to_string(),
                value: 0x48,
            }],
        },
        resources: Vec::new(),
        mmio_vmo: None,
        irq_channel: None,
        registrar: None,
        present: true,
        state: Default::default(),
    };
    let spi_node = RegisteredDeviceNode {
        info: DeviceNodeInfo {
            node_id: 8194,
            bus: BusType::Spi,
            parent_node_id: Some(8192),
            topological_path: "spi/controller-8192/device-8194".to_string(),
            properties: vec![DeviceProperty {
                key: "spi.chip_select".to_string(),
                value: 1,
            }],
        },
        resources: Vec::new(),
        mmio_vmo: None,
        irq_channel: None,
        registrar: None,
        present: true,
        state: Default::default(),
    };
    assert_eq!(
        DriverIndex::build(&manifests)
            .best_match(
                &i2c_node.info,
                &config.driver_policy,
                system_driver_identity
            )
            .unwrap()
            .manifest
            .package_name,
        "bexos.driver.test_i2c"
    );
    assert_eq!(
        DriverIndex::build(&manifests)
            .best_match(
                &spi_node.info,
                &config.driver_policy,
                system_driver_identity
            )
            .unwrap()
            .manifest
            .package_name,
        "bexos.driver.test_spi"
    );
}

#[test]
fn driver_index_returns_no_candidate_for_nonmatching_properties() {
    let manifest = nvme_driver_manifest("bexos.driver.storage.nvme", 0, false);
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let node = RegisteredDeviceNode {
        info: DeviceNodeInfo {
            node_id: 9,
            bus: BusType::Usb,
            parent_node_id: None,
            topological_path: "usb/device-9".to_string(),
            properties: vec![DeviceProperty {
                key: "usb.vendor_id".to_string(),
                value: 0x0bda,
            }],
        },
        resources: Vec::new(),
        mmio_vmo: None,
        irq_channel: None,
        registrar: None,
        present: true,
        state: Default::default(),
    };

    assert!(
        DriverIndex::build(&[manifest])
            .best_match(&node.info, &config.driver_policy, |manifest| {
                PackageIdentity {
                    package_id: &manifest.package_name,
                    signer: "bexos_official_platform_v1",
                    trust_tier: PackageTrustTier::SystemHardware,
                    is_driver: true,
                }
            })
            .is_none()
    );
}

#[test]
fn device_registry_validates_topology_resources_and_authorization() {
    let mut registry = DeviceRegistry::new();
    assert_eq!(
        registry.register_device_node(child_device_node(2, 1)),
        Err(DeviceRegistryError::MissingParent(1))
    );
    registry
        .register_device_node(nvme_device_node(1))
        .expect("root node should register");
    assert_eq!(
        registry.register_device_node(RegisteredDeviceNode {
            info: DeviceNodeInfo {
                node_id: 2,
                bus: BusType::Pci,
                parent_node_id: Some(1),
                topological_path: "pci/1/2".to_string(),
                properties: vec![
                    DeviceProperty {
                        key: "pci.vendor_id".to_string(),
                        value: 1,
                    },
                    DeviceProperty {
                        key: "pci.vendor_id".to_string(),
                        value: 2,
                    },
                ],
            },
            resources: Vec::new(),
            mmio_vmo: None,
            irq_channel: None,
            registrar: None,
            present: true,
            state: Default::default(),
        }),
        Err(DeviceRegistryError::DuplicateProperty(
            "pci.vendor_id".to_string()
        ))
    );
    assert_eq!(
        registry.register_device_node(RegisteredDeviceNode {
            info: DeviceNodeInfo {
                node_id: 3,
                bus: BusType::Pci,
                parent_node_id: Some(1),
                topological_path: "pci/1/3".to_string(),
                properties: vec![DeviceProperty {
                    key: "pci.vendor_id".to_string(),
                    value: 1,
                }],
            },
            resources: vec![
                HardwareResourceLease {
                    kind: HardwareResourceKind::Mmio,
                    resource_id: 7,
                    base: 0,
                    length: 4096,
                    flags: 0,
                    capability: Capability {
                        object_id: 200,
                        rights: 0b11,
                    },
                },
                HardwareResourceLease {
                    kind: HardwareResourceKind::Mmio,
                    resource_id: 7,
                    base: 4096,
                    length: 4096,
                    flags: 0,
                    capability: Capability {
                        object_id: 201,
                        rights: 0b11,
                    },
                },
            ],
            mmio_vmo: None,
            irq_channel: None,
            registrar: None,
            present: true,
            state: Default::default(),
        }),
        Err(DeviceRegistryError::DuplicateResource(7))
    );
    assert_eq!(
        registry.register_device_node_from(
            Some(bexos_appd::DeviceRegistrar {
                package_id: "child-driver".to_string(),
                node_id: Some(9),
                system_privileged: false,
            }),
            child_device_node(4, 1),
        ),
        Err(DeviceRegistryError::AccessDenied(4))
    );
}

#[test]
fn device_registry_unregisters_descendants_before_parent() {
    let mut registry = DeviceRegistry::new();
    registry
        .register_device_node(nvme_device_node(1))
        .expect("root node");
    registry
        .register_device_node(child_device_node(2, 1))
        .expect("child node");
    let mut grandchild = child_device_node(3, 2);
    grandchild.info.topological_path = "pci/1/2/3".to_string();
    registry
        .register_device_node(grandchild)
        .expect("grandchild node");

    let removed = registry
        .unregister_device_node_post_order(1)
        .expect("post-order unregister");
    assert_eq!(
        removed
            .iter()
            .map(|node| node.info.node_id)
            .collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
    assert!(registry.nodes().is_empty());
}

#[test]
fn broker_binds_each_device_instance_with_node_metadata() {
    let mut broker = AppdBroker::new();
    let service = ExposedService {
        name: "bexos.storage.block.BlockDevice".to_string(),
        protocol: "BlockDevice".to_string(),
        lifecycle: Lifecycle::MultipleInstance,
        visibility: Visibility::Public,
        bind_permission: None,
        metadata: Vec::new(),
        capabilities: vec![CapabilityMetadata {
            capability: "Public".to_string(),
            permission: None,
            method_ordinals: vec![1, 2],
        }],
        ..Default::default()
    };
    broker
        .publish_instance_interface(
            "bexos.driver.storage.nvme",
            "7",
            service.clone(),
            Capability {
                object_id: 70,
                rights: 0b11,
            },
        )
        .expect("node 7 instance");
    broker
        .publish_instance_interface(
            "bexos.driver.storage.nvme",
            "8",
            service,
            Capability {
                object_id: 80,
                rights: 0b11,
            },
        )
        .expect("node 8 instance");

    let mut kernel = FakeKernelOps::new();
    let bindings = broker
        .bind_consumed_service(
            &client(&[], true),
            &ConsumedService {
                name: "bexos.storage.block.BlockDevice".to_string(),
                link_type: LinkType::Required,
                filter: Some("device.node_id == '8'".to_string()),
                capabilities: vec![ConsumedCapability {
                    capability: "Public".to_string(),
                    methods: vec![MethodDependency {
                        ordinal: 1,
                        link_type: LinkType::Required,
                    }],
                }],
            },
            &mut kernel,
        )
        .expect("filtered instance should bind");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].provider_manager.object_id, 80);
}

#[test]
fn driver_index_honors_exclusions_and_service_contracts() {
    let specific = nvme_driver_manifest("bexos.driver.storage.vendor_nvme", 0, true);
    let generic = nvme_driver_manifest("bexos.driver.storage.generic_nvme", 0, false);
    let manifests = vec![specific, generic];
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let node = nvme_device_node(7);
    let mut exclusions = DriverExclusions::default();
    exclusions.exclude(7, "bexos.driver.storage.vendor_nvme", "nvme_driver");
    let contract = ServiceContract::from_services(&manifests[1].services_exposed);

    let candidates = DriverIndex::build(&manifests).ranked_candidates(
        &node.info,
        &config.driver_policy,
        |manifest| PackageIdentity {
            package_id: &manifest.package_name,
            signer: "bexos_official_platform_v1",
            trust_tier: PackageTrustTier::SystemHardware,
            is_driver: true,
        },
        Some(&exclusions),
        Some(&contract),
    );
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].manifest.package_name,
        "bexos.driver.storage.generic_nvme"
    );
}

#[test]
fn recovery_budget_retries_once_then_excludes_until_stable_reset() {
    let mut recovery = DriverRecoveryBudget::new();
    let mut exclusions = DriverExclusions::default();
    assert_eq!(
        recovery.record_failure(7, "pkg", "driver", 1000, &mut exclusions),
        RecoveryDecision::RetrySame
    );
    assert_eq!(
        recovery.record_failure(7, "pkg", "driver", 2000, &mut exclusions),
        RecoveryDecision::Fallback
    );
    assert!(exclusions.contains(7, "pkg", "driver"));
    recovery.record_stable(7, 62_001, &mut exclusions);
    assert!(!exclusions.contains(7, "pkg", "driver"));
    assert_eq!(
        recovery.record_failure(7, "pkg", "driver", 62_100, &mut exclusions),
        RecoveryDecision::RetrySame
    );
}

#[test]
fn driver_index_policy_rejection_prevents_binding() {
    let manifest = nvme_driver_manifest("bexos.driver.storage.nvme", 0, false);
    let node = nvme_device_node(7);
    let mut policy = PlatformConfig::decode(&platform_config_bytes())
        .expect("config")
        .driver_policy;
    policy.tier_1_allowlist.clear();
    policy.tier_2_rules.enforce_strict_iommu = false;

    assert!(
        DriverIndex::build(&[manifest])
            .best_match(&node.info, &policy, |manifest| PackageIdentity {
                package_id: &manifest.package_name,
                signer: "unknown",
                trust_tier: PackageTrustTier::StandardConsumer,
                is_driver: true,
            })
            .is_none()
    );
}

#[test]
fn manifest_decoder_distinguishes_absent_zero_and_later_waves() {
    let absent_wave = message(&[string_field(1, "manual"), string_field(2, "elf")]);
    let wave_zero = message(&[
        string_field(1, "root_bus"),
        string_field(2, "elf"),
        varint_field(8, 0),
    ]);
    let wave_three = message(&[
        string_field(1, "shell"),
        string_field(2, "elf"),
        varint_field(8, 3),
    ]);
    let bytes = message(&[
        string_field(1, "system:startup"),
        message_field(3, &absent_wave),
        message_field(3, &wave_zero),
        message_field(3, &wave_three),
    ]);

    let manifest = Manifest::decode(&bytes).expect("manifest should decode");

    assert_eq!(manifest.processes[0].wave, None);
    assert_eq!(manifest.processes[1].wave, Some(0));
    assert_eq!(manifest.processes[2].wave, Some(3));
}

#[test]
fn manifest_decoder_reads_elf_runner_options_any() {
    let elf_options = message(&[string_field(1, "/pkg/bin/camera_service")]);
    let runner_options = message(&[
        string_field(1, "type.googleapis.com/bexos.app.ELFRunnerOptions"),
        bytes_field(2, &elf_options),
    ]);
    let process = message(&[
        string_field(1, "camera_service"),
        string_field(2, "elf"),
        message_field(7, &runner_options),
    ]);
    let bytes = message(&[
        string_field(1, "bexos.hardware:camera"),
        string_field(2, "Camera Provider"),
        message_field(3, &process),
    ]);

    let manifest = Manifest::decode(&bytes).expect("manifest should decode");

    assert_eq!(
        manifest.processes[0].runner_options,
        Some(ProcessRunnerOptions::Elf(bexos_appd::ElfRunnerOptions {
            path: "/pkg/bin/camera_service".to_string()
        }))
    );
}

#[test]
fn wave_plan_sorts_groups_and_excludes_manual_processes() {
    let manifests = vec![wave_manifest(
        "system:startup",
        &[
            ("display", Some(2)),
            ("manual_tool", None),
            ("root_bus", Some(0)),
            ("timer", Some(0)),
        ],
    )];

    let plan = AppdWaveOrchestrator::new().plan(&manifests);

    assert_eq!(plan.waves.len(), 2);
    assert_eq!(plan.waves[0].wave, 0);
    assert_eq!(
        plan.waves[0]
            .processes
            .iter()
            .map(|process_ref| process_ref.process.name.as_str())
            .collect::<Vec<_>>(),
        vec!["root_bus", "timer"]
    );
    assert_eq!(plan.waves[1].wave, 2);
    assert_eq!(plan.manual.len(), 1);
    assert_eq!(plan.manual[0].process.name, "manual_tool");
    assert_eq!(plan.manual[0].startup_class(), StartupClass::Manual);
    assert_eq!(
        plan.find_manual("system:startup", "manual_tool")
            .expect("manual process should be found")
            .process
            .name,
        "manual_tool"
    );
}

#[test]
fn storage_preinstalled_service_plan_launches_only_waved_services() {
    let netstack = wave_manifest("bexos.service.netstackd", &[("netstackd", Some(5))]);
    let timed = wave_manifest("bexos.service.timed", &[("timed", Some(6))]);
    let storage_verify = wave_manifest("bexos.platform.storage_verify", &[("verify", None)]);
    let driver = nvme_driver_manifest("bexos.driver.network.virtio_net", 4, false);

    let plan = bexos_appd::guest::preinstalled_storage_service_plan(&[
        timed,
        storage_verify,
        driver,
        netstack,
    ]);

    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].package_id, "bexos.service.netstackd");
    assert_eq!(plan[0].process_name, "netstackd");
    assert_eq!(plan[0].wave, 5);
    assert_eq!(plan[1].package_id, "bexos.service.timed");
    assert_eq!(plan[1].process_name, "timed");
    assert_eq!(plan[1].wave, 6);
}

#[test]
fn wave_orchestrator_waits_for_wave_readiness_before_next_wave() {
    let manifests = vec![wave_manifest(
        "bexos.hardware:camera",
        &[
            ("root_bus", Some(0)),
            ("timer", Some(0)),
            ("display", Some(1)),
            ("manual_tool", None),
        ],
    )];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = RecordingReadiness::default();

    let launched = AppdWaveOrchestrator::new()
        .launch_automatic(
            &manifests,
            PackageTrustTier::SystemHardware,
            1,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect("automatic waves should launch");

    assert_eq!(
        launched
            .iter()
            .map(|launch| launch.process_ref.process.name.as_str())
            .collect::<Vec<_>>(),
        vec!["root_bus", "timer", "display"]
    );
    assert_eq!(
        readiness.ready_processes,
        vec![
            "bexos.hardware:camera:root_bus".to_string(),
            "bexos.hardware:camera:timer".to_string(),
            "bexos.hardware:camera:display".to_string(),
        ]
    );
    let create_names = kernel
        .operations
        .iter()
        .filter_map(|operation| match operation {
            KernelOperation::CreateProcess { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        create_names,
        vec![
            "bexos.hardware:camera:root_bus",
            "bexos.hardware:camera:timer",
            "bexos.hardware:camera:display",
        ]
    );
}

#[test]
fn wave_orchestrator_stops_on_launch_failure() {
    let manifests = vec![wave_manifest(
        "bexos.hardware:camera",
        &[("root_bus", Some(0)), ("display", Some(1))],
    )];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = ImmediateReadiness;

    let error = AppdWaveOrchestrator::new()
        .launch_automatic(
            &manifests,
            PackageTrustTier::StandardConsumer,
            1,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect_err("consumer elf launch should fail");

    assert_eq!(
        error,
        StartupError::Launch {
            package_name: "bexos.hardware:camera".to_string(),
            process_name: "root_bus".to_string(),
            source: LaunchError::RunnerPolicyDenied,
        }
    );
    assert!(kernel.operations.is_empty());
}

#[test]
fn wave_orchestrator_stops_before_next_wave_on_readiness_failure() {
    let manifests = vec![wave_manifest(
        "bexos.hardware:camera",
        &[("root_bus", Some(0)), ("display", Some(1))],
    )];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = RecordingReadiness {
        fail_on: Some("root_bus"),
        ..RecordingReadiness::default()
    };

    let error = AppdWaveOrchestrator::new()
        .launch_automatic(
            &manifests,
            PackageTrustTier::SystemHardware,
            1,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect_err("readiness should stop startup");

    assert_eq!(
        error,
        StartupError::Readiness {
            package_name: "bexos.hardware:camera".to_string(),
            process_name: "root_bus".to_string(),
            source: ReadinessError::TimedOut,
        }
    );
    let create_names = kernel
        .operations
        .iter()
        .filter_map(|operation| match operation {
            KernelOperation::CreateProcess { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(create_names, vec!["bexos.hardware:camera:root_bus"]);
}

#[test]
fn wave_orchestrator_launches_manual_processes_through_runner_policy() {
    let manifests = vec![wave_manifest(
        "bexos.hardware:camera",
        &[("manual_tool", None), ("display", Some(1))],
    )];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();

    let launched = AppdWaveOrchestrator::new()
        .launch_manual(
            &manifests,
            "bexos.hardware:camera",
            "manual_tool",
            PackageTrustTier::SystemHardware,
            1,
            &mut kernel,
            &resolver,
        )
        .expect("manual process should launch");

    assert_eq!(launched.process_ref.process.name, "manual_tool");

    let mut denied_kernel = FakeKernelOps::new();
    assert_eq!(
        AppdWaveOrchestrator::new().launch_manual(
            &manifests,
            "bexos.hardware:camera",
            "manual_tool",
            PackageTrustTier::StandardConsumer,
            1,
            &mut denied_kernel,
            &resolver,
        ),
        Err(StartupError::Launch {
            package_name: "bexos.hardware:camera".to_string(),
            process_name: "manual_tool".to_string(),
            source: LaunchError::RunnerPolicyDenied,
        })
    );
    assert!(denied_kernel.operations.is_empty());

    assert_eq!(
        AppdWaveOrchestrator::new().launch_manual(
            &manifests,
            "bexos.hardware:camera",
            "missing",
            PackageTrustTier::SystemHardware,
            1,
            &mut kernel,
            &resolver,
        ),
        Err(StartupError::ManualProcessNotFound {
            package_name: "bexos.hardware:camera".to_string(),
            process_name: "missing".to_string(),
        })
    );
}

#[test]
fn wave_orchestrator_creates_manifest_resource_groups_before_process_launch() {
    let mut manifest = launch_manifest();
    manifest.resource_groups.push(ResourceGroup {
        name: "camera_foreground".to_string(),
        cpu_shares: 768,
        memory_limit_pages: 4096,
        ..Default::default()
    });
    manifest.processes[0].resource_group = Some("camera_foreground".to_string());
    let manifests = vec![manifest];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = ImmediateReadiness;

    AppdWaveOrchestrator::new()
        .launch_automatic(
            &manifests,
            PackageTrustTier::SystemHardware,
            1,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect("manifest resource group should launch");

    assert!(matches!(
        kernel.operations.first(),
        Some(KernelOperation::CreateResourceGroup {
            name,
            cpu_shares: 768,
            memory_limit_pages: 4096,
        }) if name == "camera_foreground"
    ));
    assert!(kernel.operations.iter().any(|operation| {
        matches!(
            operation,
            KernelOperation::CreateProcess {
                resource_group_id: 1001,
                ..
            }
        )
    }));
}

#[test]
fn wave_orchestrator_resolves_builtin_resource_group_names() {
    let mut manifest = launch_manifest();
    manifest.processes[0].resource_group = Some("background".to_string());
    let manifests = vec![manifest];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = ImmediateReadiness;

    AppdWaveOrchestrator::new()
        .launch_automatic(
            &manifests,
            PackageTrustTier::SystemHardware,
            1,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect("builtin resource group should launch");

    assert!(kernel.operations.iter().any(|operation| {
        matches!(
            operation,
            KernelOperation::CreateProcess {
                resource_group_id: 3,
                ..
            }
        )
    }));
}

#[test]
fn wave_orchestrator_rejects_unknown_resource_group_names() {
    let mut manifest = launch_manifest();
    manifest.processes[0].resource_group = Some("missing_group".to_string());
    let manifests = vec![manifest];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = ImmediateReadiness;

    let error = AppdWaveOrchestrator::new()
        .launch_automatic(
            &manifests,
            PackageTrustTier::SystemHardware,
            1,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect_err("unknown resource group should fail");

    assert_eq!(
        error,
        StartupError::Launch {
            package_name: "bexos.hardware:camera".to_string(),
            process_name: "camera_service".to_string(),
            source: LaunchError::Kernel {
                operation: "resolve_resource_group",
                source: bexos_appd::KernelError::InvalidArgs,
            },
        }
    );
    assert!(kernel.operations.is_empty());
}

#[test]
fn broker_publishes_queries_and_returns_singleton_endpoints() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            camera_service(Some("CAMERA"), Lifecycle::Singleton),
            Capability {
                object_id: 7,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let client = client(&["CAMERA"], true);
    let results = broker.get_interfaces(
        &client,
        InterfaceQuery {
            protocol: Some("CameraController"),
            metadata: &[Metadata {
                key: "supports_capture".to_string(),
                value: "true".to_string(),
            }],
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].endpoint.object_id, 7);
    assert_eq!(
        broker
            .get_singleton_interface(&client, "bexos.hardware.Camera")
            .expect("singleton should bind")
            .endpoint
            .object_id,
        7
    );
}

#[test]
fn broker_rejects_duplicate_singleton_service_names() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera-a",
            camera_service(None, Lifecycle::Singleton),
            Capability {
                object_id: 7,
                rights: 0b11,
            },
        )
        .expect("first singleton should publish");

    assert_eq!(
        broker.publish_interface(
            "bexos.hardware:camera-b",
            camera_service(None, Lifecycle::Singleton),
            Capability {
                object_id: 8,
                rights: 0b11,
            },
        ),
        Err(BindError::Registry(RegistryError::DuplicateSingleton {
            name: "bexos.hardware.Camera".to_string(),
            existing_provider_package: "bexos.hardware:camera-a".to_string(),
            provider_package: "bexos.hardware:camera-b".to_string(),
        }))
    );
}

#[test]
fn broker_allows_duplicate_multiple_instance_service_names() {
    let mut broker = AppdBroker::new();
    for (package, endpoint) in [("bexos.driver:power-a", 7), ("bexos.driver:power-b", 8)] {
        broker
            .publish_interface(
                package,
                ExposedService {
                    name: "bexos.power.DevicePowerControl".to_string(),
                    protocol: "DevicePowerControl".to_string(),
                    lifecycle: Lifecycle::MultipleInstance,
                    visibility: Visibility::Public,
                    bind_permission: None,
                    metadata: Vec::new(),
                    capabilities: Vec::new(),
                    ..Default::default()
                },
                Capability {
                    object_id: endpoint,
                    rights: 0b11,
                },
            )
            .expect("multiple-instance service should publish");
    }

    let results = broker.get_interfaces(
        &client(&[], true),
        InterfaceQuery {
            protocol: Some("DevicePowerControl"),
            metadata: &[],
        },
    );

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].endpoint.object_id, 7);
    assert_eq!(results[1].endpoint.object_id, 8);
}

#[test]
fn broker_denies_missing_permissions_and_allows_public_services() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            camera_service(Some("CAMERA"), Lifecycle::Singleton),
            Capability {
                object_id: 8,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");
    broker
        .publish_interface(
            "bexos.hardware:status",
            ExposedService {
                name: "bexos.hardware.Status".to_string(),
                protocol: "StatusController".to_string(),
                lifecycle: Lifecycle::Singleton,
                visibility: Visibility::Public,
                bind_permission: None,
                metadata: Vec::new(),
                capabilities: Vec::new(),
                ..Default::default()
            },
            Capability {
                object_id: 9,
                rights: 0b01,
            },
        )
        .expect("publish should succeed");

    let denied = client(&[], true);
    assert_eq!(
        broker.get_singleton_interface(&denied, "bexos.hardware.Camera"),
        Err(BindError::PermissionDenied)
    );
    assert_eq!(
        broker
            .get_singleton_interface(&denied, "bexos.hardware.Status")
            .expect("public service should bind")
            .endpoint
            .object_id,
        9
    );
}

#[test]
fn broker_resolves_user_scoped_singletons_only_for_user_contexts() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "com.example:vault",
            camera_service(None, Lifecycle::UserScopedSingleton),
            Capability {
                object_id: 10,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let mut no_user = client(&[], true);
    no_user.user_id = None;
    assert_eq!(
        broker.get_user_scoped_singleton(&no_user, "bexos.hardware.Camera"),
        Err(BindError::PermissionDenied)
    );

    let mut user = client(&[], true);
    user.user_id = Some(42);
    assert_eq!(
        broker
            .get_user_scoped_singleton(&user, "bexos.hardware.Camera")
            .expect("user scoped singleton should bind")
            .endpoint
            .object_id,
        10
    );
}

#[test]
fn permission_policy_handles_public_exact_cel_and_unsupported_expressions() {
    let foreground = client(&["CAMERA"], true);
    let background = client(&["CAMERA"], false);
    let missing = client(&[], true);

    assert_eq!(
        bexos_appd::policy::check_permission(None, &missing),
        PermissionDecision::Allow
    );
    assert_eq!(
        bexos_appd::policy::check_permission(Some("CAMERA"), &foreground),
        PermissionDecision::Allow
    );
    assert_eq!(
        bexos_appd::policy::check_permission(
            Some("request.permissions.contains('CAMERA') && client.is_foreground"),
            &foreground,
        ),
        PermissionDecision::Allow
    );
    assert_eq!(
        bexos_appd::policy::check_permission(
            Some("request.permissions.contains('CAMERA') && client.is_foreground"),
            &background,
        ),
        PermissionDecision::Deny
    );
    assert_eq!(
        bexos_appd::policy::check_permission(Some("device.battery_level > 10"), &foreground),
        PermissionDecision::Deny
    );
}

#[test]
fn generated_fidl_capability_metadata_can_be_filtered_for_bind_time() {
    let method_ordinals: Vec<Vec<u64>> = camera_fidl::CAPABILITY_BINDINGS
        .iter()
        .map(|binding| {
            binding
                .methods
                .iter()
                .map(|method| method.ordinal)
                .collect()
        })
        .collect();
    let capabilities: Vec<FidlCapability<'_>> = camera_fidl::CAPABILITY_BINDINGS
        .iter()
        .zip(method_ordinals.iter())
        .map(|(binding, ordinals)| FidlCapability {
            protocol: binding.protocol,
            capability: binding.capability,
            permission: binding.permission,
            method_ordinals: ordinals,
        })
        .collect();

    let client = client(&["CAMERA"], true);
    let allowed = allowed_capabilities(&capabilities, &client);

    assert!(
        allowed
            .iter()
            .any(|capability| capability.capability == "Public")
    );
    assert!(
        allowed
            .iter()
            .any(|capability| capability.capability == "Camera")
    );
    assert!(
        allowed.iter().any(|capability| capability.capability
            == "RequestPermissionsContainsCameraClientIsForeground")
    );
    assert!(
        !allowed
            .iter()
            .any(|capability| capability.capability == "AndroidPermissionCameraRecordCamera1")
    );
}

#[test]
fn broker_mints_live_endpoints_for_only_allowed_capabilities() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            ExposedService {
                capabilities: vec![
                    CapabilityMetadata {
                        capability: "Public".to_string(),
                        permission: None,
                        method_ordinals: vec![1],
                    },
                    CapabilityMetadata {
                        capability: "Camera".to_string(),
                        permission: Some("CAMERA".to_string()),
                        method_ordinals: vec![2, 3],
                    },
                    CapabilityMetadata {
                        capability: "RawSensor".to_string(),
                        permission: Some("RAW_SENSOR_TUNING".to_string()),
                        method_ordinals: vec![4],
                    },
                ],
                ..camera_service(None, Lifecycle::Singleton)
            },
            Capability {
                object_id: 44,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    let mut kernel = FakeKernelOps::new();
    let bindings = broker
        .bind_consumed_service(
            &client(&["CAMERA"], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: Some("supports_capture == 'true'".to_string()),
                capabilities: vec![
                    ConsumedCapability {
                        capability: "Public".to_string(),
                        methods: vec![MethodDependency {
                            ordinal: 1,
                            link_type: LinkType::Required,
                        }],
                    },
                    ConsumedCapability {
                        capability: "Camera".to_string(),
                        methods: vec![
                            MethodDependency {
                                ordinal: 2,
                                link_type: LinkType::Required,
                            },
                            MethodDependency {
                                ordinal: 3,
                                link_type: LinkType::Required,
                            },
                        ],
                    },
                ],
            },
            &mut kernel,
        )
        .expect("camera capability should bind");

    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].capability, "Public");
    assert_eq!(bindings[0].provider_manager.object_id, 44);
    assert_eq!(bindings[0].client_endpoint.object_id, 1);
    assert_eq!(bindings[0].provider_endpoint.object_id, 2);
    assert_eq!(
        bindings[0].client_endpoint.rights,
        bexos_appd::broker::SERVICE_ENDPOINT_RIGHTS
    );
    assert_eq!(
        bindings[0].provider_endpoint.rights,
        bexos_appd::broker::SERVICE_ENDPOINT_RIGHTS
    );
    assert_eq!(bindings[1].capability, "Camera");
    assert_eq!(bindings[1].method_ordinals, vec![2, 3]);
    assert_eq!(bindings[1].permission_values, vec!["default".to_string()]);
    assert_eq!(
        kernel.operations,
        vec![
            KernelOperation::CreateChannel,
            KernelOperation::CreateChannel
        ]
    );
}

#[test]
fn broker_reports_unavailable_or_fully_denied_consumed_services() {
    let mut broker = AppdBroker::new();
    let mut kernel = FakeKernelOps::new();
    assert_eq!(
        broker.bind_consumed_service(
            &client(&[], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: Vec::new(),
            },
            &mut kernel,
        ),
        Err(BindError::NotFound)
    );

    broker
        .publish_interface(
            "bexos.hardware:camera",
            ExposedService {
                bind_permission: None,
                capabilities: vec![CapabilityMetadata {
                    capability: "Camera".to_string(),
                    permission: Some("CAMERA".to_string()),
                    method_ordinals: vec![7],
                }],
                ..camera_service(None, Lifecycle::Singleton)
            },
            Capability {
                object_id: 45,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");

    assert_eq!(
        broker.bind_consumed_service(
            &client(&[], true),
            &ConsumedService {
                name: "bexos.hardware.Camera".to_string(),
                link_type: LinkType::Optional,
                filter: None,
                capabilities: Vec::new(),
            },
            &mut kernel,
        ),
        Err(BindError::PermissionDenied)
    );
    assert!(kernel.operations.is_empty());
}

#[test]
fn appd_freeze_restore_round_trips_published_interfaces() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.hardware:camera",
            camera_service(Some("CAMERA"), Lifecycle::Singleton),
            Capability {
                object_id: 11,
                rights: 0b11,
            },
        )
        .expect("publish should succeed");
    broker
        .publish_interface(
            "bexos.hardware:status",
            ExposedService {
                name: "bexos.hardware.Status".to_string(),
                protocol: "StatusController".to_string(),
                lifecycle: Lifecycle::Singleton,
                visibility: Visibility::Public,
                bind_permission: None,
                metadata: Vec::new(),
                capabilities: Vec::new(),
                ..Default::default()
            },
            Capability {
                object_id: 12,
                rights: 0b01,
            },
        )
        .expect("publish should succeed");

    let encoded = broker.freeze().encode().expect("snapshot should encode");
    let snapshot = AppdSnapshot::decode(&encoded).expect("snapshot should decode");
    let restored = AppdBroker::restore(snapshot).expect("snapshot should restore");
    let client = client(&["CAMERA"], true);

    assert_eq!(
        restored
            .get_singleton_interface(&client, "bexos.hardware.Camera")
            .expect("camera should bind after restore")
            .endpoint
            .object_id,
        11
    );
    assert_eq!(
        restored
            .get_singleton_interface(&client, "bexos.hardware.Status")
            .expect("status should bind after restore")
            .endpoint
            .rights,
        0b01
    );
}

#[test]
fn appd_snapshot_decoder_rejects_invalid_inputs() {
    assert_eq!(AppdSnapshot::decode(b"nope"), Err(SnapshotError::BadMagic));

    let mut encoded = AppdSnapshot::default()
        .encode()
        .expect("empty snapshot should encode");
    encoded[4] = 99;
    assert_eq!(
        AppdSnapshot::decode(&encoded),
        Err(SnapshotError::UnsupportedVersion)
    );

    let empty_snapshot = AppdSnapshot::default()
        .encode()
        .expect("empty snapshot should encode");
    let truncated = &empty_snapshot[..5];
    assert_eq!(
        AppdSnapshot::decode(truncated),
        Err(SnapshotError::UnexpectedEof)
    );
}

#[test]
fn appd_device_registry_publishes_scoped_bus_registrar_capabilities() {
    let mut broker = AppdBroker::new();
    broker
        .publish_interface(
            "bexos.platform.appd",
            ExposedService {
                name: "bexos.hardware.manager.DeviceRegistry".into(),
                protocol: "DeviceRegistry".into(),
                lifecycle: Lifecycle::Singleton,
                visibility: Visibility::Public,
                bind_permission: Some(SYSTEM_PRIVILEGED_PERMISSION.into()),
                metadata: Vec::new(),
                capabilities: vec![
                    CapabilityMetadata {
                        capability: "Public".into(),
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
                ..Default::default()
            },
            Capability {
                object_id: 0,
                rights: 0b11,
            },
        )
        .expect("publish appd device registry");

    let consumed = ConsumedService {
        name: "bexos.hardware.manager.DeviceRegistry".into(),
        link_type: LinkType::Required,
        filter: None,
        capabilities: vec![ConsumedCapability {
            capability: "I2cDescendantRegistrar".into(),
            methods: vec![MethodDependency {
                ordinal: 1,
                link_type: LinkType::Required,
            }],
        }],
    };
    let mut kernel = FakeKernelOps::new();
    assert_eq!(
        broker.bind_consumed_service(&client(&[], true), &consumed, &mut kernel),
        Err(BindError::NotFound)
    );

    let bindings = broker
        .bind_consumed_service(
            &client(&[SYSTEM_PRIVILEGED_PERMISSION], true),
            &consumed,
            &mut kernel,
        )
        .expect("privileged i2c registrar binding");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].capability, "I2cDescendantRegistrar");
    assert_eq!(bindings[0].method_ordinals, vec![1]);
}

#[test]
fn appd_publishes_kernel_services_with_privileged_gate() {
    let mut broker = AppdBroker::new();
    publish_kernel_services(&mut broker).expect("kernel services should publish");

    let unprivileged = client(&[], true);
    assert_eq!(
        broker
            .get_singleton_interface(&unprivileged, "bexos.kernel.ChannelControl")
            .expect("public channel control")
            .protocol,
        "ChannelControl"
    );
    let system = broker
        .get_singleton_interface(&unprivileged, "bexos.kernel.SystemPrivileged")
        .expect("public system protocol endpoint");
    assert_eq!(system.protocol, "SystemPrivileged");
    assert_eq!(
        broker
            .get_singleton_interface(&unprivileged, "bexos.kernel.ProfileProvider")
            .expect("public profile provider")
            .protocol,
        "ProfileProvider"
    );

    let mut kernel = FakeKernelOps::new();
    assert_eq!(
        broker.bind_consumed_service(
            &unprivileged,
            &ConsumedService {
                name: "bexos.kernel.SystemPrivileged".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "BexosSystemPrivileged".to_string(),
                    methods: vec![MethodDependency {
                        ordinal: 6711119950886114089,
                        link_type: LinkType::Required,
                    }],
                }],
            },
            &mut kernel,
        ),
        Err(BindError::PermissionDenied)
    );

    let privileged = client(&[SYSTEM_PRIVILEGED_PERMISSION], true);
    let bindings = broker
        .bind_consumed_service(
            &privileged,
            &ConsumedService {
                name: "bexos.kernel.SystemPrivileged".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "BexosSystemPrivileged".to_string(),
                    methods: vec![MethodDependency {
                        ordinal: 6711119950886114089,
                        link_type: LinkType::Required,
                    }],
                }],
            },
            &mut kernel,
        )
        .expect("privileged system service");
    assert!(
        bindings
            .iter()
            .any(|binding| binding.capability == "BexosSystemPrivileged")
    );

    let time_setter = client(&["SET_TIME"], true);
    let bindings = broker
        .bind_consumed_service(
            &time_setter,
            &ConsumedService {
                name: "bexos.kernel.SystemPrivileged".to_string(),
                link_type: LinkType::Required,
                filter: None,
                capabilities: vec![ConsumedCapability {
                    capability: "SetTime".to_string(),
                    methods: vec![MethodDependency {
                        ordinal: 6855954954352417048,
                        link_type: LinkType::Required,
                    }],
                }],
            },
            &mut kernel,
        )
        .expect("set-time system service");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].capability, "SetTime");
    assert_ne!(bindings[0].client_endpoint.object_id, 0);
}

#[test]
fn app_debug_registry_lists_recorded_processes_over_fidl() {
    let manifest = Manifest {
        architecture: bexos_app_manifest::Architecture::current_guest(),
        package_name: "bexos.driver.debugd".into(),
        name: "debugd".into(),
        processes: vec![bexos_appd::Process {
            name: "debugd".into(),
            runner: "elf".into(),
            service: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut registry = bexos_appd::AppDebugProcessRegistry::new();
    registry.record_created(42, &manifest, &manifest.processes[0]);

    let response = registry
        .list_processes(AppDebugControlListProcessesRequest {})
        .expect("app debug FIDL response");
    assert_eq!(response.status, app_debug_fidl::AppDebugStatus::Ok);
    assert_eq!(response.processes.len(), 1);
    let process = response.processes.get(0).unwrap();
    assert_eq!(process.pid, 42);
    assert_eq!(process.process_name, "debugd");
    assert_eq!(process.package_id, "bexos.driver.debugd");
    assert_eq!(process.state, app_debug_fidl::AppProcessState::Created);
}

#[test]
fn generated_kernel_fidl_metadata_can_be_passed_through_policy_filter() {
    let method_ordinals: Vec<Vec<u64>> = kernel_fidl::CAPABILITY_BINDINGS
        .iter()
        .map(|binding| {
            binding
                .methods
                .iter()
                .map(|method| method.ordinal)
                .collect()
        })
        .collect();
    let capabilities: Vec<FidlCapability<'_>> = kernel_fidl::CAPABILITY_BINDINGS
        .iter()
        .zip(method_ordinals.iter())
        .map(|(binding, ordinals)| FidlCapability {
            protocol: binding.protocol,
            capability: binding.capability,
            permission: binding.permission,
            method_ordinals: ordinals,
        })
        .collect();

    let allowed = allowed_capabilities(&capabilities, &client(&[], true));
    assert!(
        allowed
            .iter()
            .any(|capability| capability.protocol == "ChannelControl")
    );
    assert!(
        !allowed
            .iter()
            .any(|capability| capability.protocol == "SystemPrivileged")
    );

    let privileged = allowed_capabilities(
        &capabilities,
        &client(&[SYSTEM_PRIVILEGED_PERMISSION], true),
    );
    assert!(
        privileged
            .iter()
            .any(|capability| capability.protocol == "SystemPrivileged"
                && capability.capability == "BexosSystemPrivileged")
    );
}

#[test]
fn elf_runner_policy_allows_only_system_tiers() {
    let manifest = launch_manifest();
    let process = &manifest.processes[0];
    let registry = RunnerRegistry::new();
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);

    let mut consumer_kernel = FakeKernelOps::new();
    assert_eq!(
        registry.launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::StandardConsumer,
                identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut consumer_kernel,
            &resolver,
        ),
        Err(LaunchError::RunnerPolicyDenied)
    );

    let mut system_kernel = FakeKernelOps::new();
    let launched = registry
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::PlatformCore,
                identity: package_identity(&manifest, PackageTrustTier::PlatformCore, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut system_kernel,
            &resolver,
        )
        .expect("system elf should launch");

    assert_ne!(launched.main_thread_handle.raw, 0);
}

#[test]
fn runner_rejects_library_packages_as_launch_targets() {
    let mut manifest = launch_manifest();
    manifest.package_kind = PackageKind::Library;
    let process = &manifest.processes[0];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();

    assert_eq!(
        RunnerRegistry::new().launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::PlatformCore,
                identity: package_identity(&manifest, PackageTrustTier::PlatformCore, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        ),
        Err(LaunchError::LibraryPackageNotLaunchable)
    );
}

#[test]
fn platform_config_decoder_reads_aarch64_runner_tee_and_driver_policy() {
    let bytes = platform_config_bytes();
    let config = PlatformConfig::decode(&bytes).expect("platform config should decode");

    assert_eq!(
        config.metadata.target_board,
        "//device/virtual/qemu/base/aarch64"
    );
    assert_eq!(
        config.metadata.architecture,
        bexos_appd::Architecture::Aarch64
    );
    assert!(config.runner_policy.allow_microvm_runner);
    assert!(!config.runner_policy.allow_native_elf_runner);
    assert_eq!(config.tee_policy.allowed_trusted_apps.len(), 6);
    assert!(config.driver_policy.tier_2_rules.enforce_strict_iommu);
    assert_eq!(
        config.update_policy.metadata_base_url,
        "https://repo.example/metadata"
    );
    assert_eq!(config.update_policy.kernel_target_name, "kernel.img");

    let direct = config.driver_policy.evaluate_driver(PackageIdentity {
        package_id: "bexos.driver.storage.nvme",
        signer: "bexos_official_platform_v1",
        trust_tier: PackageTrustTier::SystemHardware,
        is_driver: true,
    });
    assert_eq!(
        direct,
        bexos_appd::DriverPolicyDecision::Allow {
            hardware_access: HardwareAccessTier::Direct
        }
    );

    let isolated = config.driver_policy.evaluate_driver(PackageIdentity {
        package_id: "bexos.driver.net.usb",
        signer: "third_party",
        trust_tier: PackageTrustTier::StandardConsumer,
        is_driver: true,
    });
    assert_eq!(
        isolated,
        bexos_appd::DriverPolicyDecision::Allow {
            hardware_access: HardwareAccessTier::Isolated
        }
    );
}

#[test]
fn install_app_from_url_rejects_non_https_and_reports_fetch_failure() {
    let mut state = bexos_appd::guest::state::AppdState::empty();
    let mut fetcher = FailingBundleFetcher;
    let (status, _, package_id) =
        bexos_appd::install_app_from_url(&mut state, &mut fetcher, "http://repo.example/app.bex");
    assert_eq!(status, app_manager_fidl::AppManagerStatus::InvalidArgs);
    assert!(package_id.is_empty());

    let (status, association, package_id) =
        bexos_appd::install_app_from_url(&mut state, &mut fetcher, "https://repo.example/app.bex");
    assert_eq!(status, app_manager_fidl::AppManagerStatus::Network);
    assert_eq!(
        association,
        app_manager_fidl::AssociationStatus::DomainUnreachable
    );
    assert!(package_id.is_empty());
}

#[test]
fn platform_runner_policy_denies_consumer_elf_and_routes_microvm() {
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let consumer = PackageIdentity {
        package_id: "com.example.native",
        signer: "third_party",
        trust_tier: PackageTrustTier::StandardConsumer,
        is_driver: false,
    };

    assert_eq!(
        config.runner_policy.evaluate_runner("elf", consumer),
        RunnerPolicyDecision::Deny
    );
    assert_eq!(
        config.runner_policy.evaluate_runner("nix", consumer),
        RunnerPolicyDecision::RouteToMicrovm
    );
    assert_eq!(
        config.runner_policy.evaluate_runner("wasm", consumer),
        RunnerPolicyDecision::Allow
    );
}

#[test]
fn platform_runner_policy_decodes_starnix_opt_in() {
    let runner_policy = message(&[varint_field(7, 1)]);
    let config =
        PlatformConfig::decode(&message(&[message_field(2, &runner_policy)])).expect("config");
    let consumer = PackageIdentity {
        package_id: "com.example.linux",
        signer: "third_party",
        trust_tier: PackageTrustTier::StandardConsumer,
        is_driver: false,
    };
    assert!(config.runner_policy.allow_starnix_runner);
    assert_eq!(
        config.runner_policy.evaluate_runner("nix", consumer),
        RunnerPolicyDecision::Allow
    );
}

#[test]
fn policy_launches_tier_one_driver_with_direct_hardware_access() {
    let manifest = launch_manifest();
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = ImmediateReadiness;

    AppdWaveOrchestrator::new()
        .launch_automatic_with_policy(
            &[manifest],
            &config.runner_policy,
            &config.driver_policy,
            |_| PackageIdentity {
                package_id: "bexos.driver.storage.nvme",
                signer: "bexos_official_platform_v1",
                trust_tier: PackageTrustTier::SystemHardware,
                is_driver: true,
            },
            4,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect("tier one driver should launch");

    assert!(kernel.operations.iter().any(|operation| {
        matches!(
            operation,
            KernelOperation::CreateProcess {
                package_id,
                hardware_access: HardwareAccessTier::Direct,
                resource_group_id: 4,
                ..
            } if package_id == "bexos.driver.storage.nvme"
        )
    }));
}

#[test]
fn policy_launches_platform_service_without_driver_hardware_grant() {
    let manifest = wave_manifest("bexos.service.teed", &[("teed", Some(1))]);
    let manifests = [manifest];
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = ImmediateReadiness;

    let launched = AppdWaveOrchestrator::new()
        .launch_automatic_with_policy(
            &manifests,
            &config.runner_policy,
            &config.driver_policy,
            |_| PackageIdentity {
                package_id: "bexos.service.teed",
                signer: "bexos_official_platform_v1",
                trust_tier: PackageTrustTier::SystemHardware,
                is_driver: false,
            },
            4,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect("platform service should not require a driver hardware grant");

    assert_eq!(launched[0].hardware_access, HardwareAccessTier::None);

    assert!(kernel.operations.iter().any(|operation| {
        matches!(
            operation,
            KernelOperation::CreateProcess {
                package_id,
                hardware_access: HardwareAccessTier::None,
                ..
            } if package_id == "bexos.service.teed"
        )
    }));
}

#[test]
fn policy_launches_bound_driver_after_wave_zero_device_registration() {
    let pci = wave_manifest("bexos.driver.pci_root", &[("pci_root_bus", Some(0))]);
    let nvme = nvme_driver_manifest("bexos.driver.storage.nvme", 1, false);
    let manifests = vec![pci, nvme];
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = RecordingReadiness {
        register_on_ready: Some("bexos.driver.pci_root"),
        ..Default::default()
    };

    let launched = AppdWaveOrchestrator::new()
        .launch_automatic_with_policy(
            &manifests,
            &config.runner_policy,
            &config.driver_policy,
            |manifest| PackageIdentity {
                package_id: &manifest.package_name,
                signer: "bexos_official_platform_v1",
                trust_tier: PackageTrustTier::SystemHardware,
                is_driver: manifest.package_name.starts_with("bexos.driver."),
            },
            4,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect("matching driver should launch");

    assert_eq!(launched.len(), 2);
    assert_eq!(launched[1].hardware_access, HardwareAccessTier::Direct);
    assert_eq!(
        readiness.ready_processes,
        [
            "bexos.driver.pci_root:pci_root_bus",
            "bexos.driver.storage.nvme:nvme_driver",
        ]
    );
    assert!(matches!(
        readiness.registry.nodes()[0].state,
        DeviceNodeState::Active(_)
    ));
    assert!(kernel.operations.iter().any(|operation| matches!(
        operation,
        KernelOperation::CreateProcess {
            package_id,
            hardware_access: HardwareAccessTier::Direct,
            ..
        } if package_id == "bexos.driver.storage.nvme"
    )));
}

#[test]
fn policy_does_not_launch_bound_driver_without_registered_device() {
    let pci = wave_manifest("bexos.driver.pci_root", &[("pci_root_bus", Some(0))]);
    let nvme = nvme_driver_manifest("bexos.driver.storage.nvme", 1, false);
    let manifests = vec![pci, nvme];
    let config = PlatformConfig::decode(&platform_config_bytes()).expect("config");
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = RecordingReadiness::default();

    let launched = AppdWaveOrchestrator::new()
        .launch_automatic_with_policy(
            &manifests,
            &config.runner_policy,
            &config.driver_policy,
            |manifest| PackageIdentity {
                package_id: &manifest.package_name,
                signer: "bexos_official_platform_v1",
                trust_tier: PackageTrustTier::SystemHardware,
                is_driver: manifest.package_name.starts_with("bexos.driver."),
            },
            4,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .expect("unmatched bind-rule driver should stay stopped");

    assert_eq!(launched.len(), 1);
    assert!(!kernel.operations.iter().any(|operation| matches!(
        operation,
        KernelOperation::CreateProcess { package_id, .. }
            if package_id == "bexos.driver.storage.nvme"
    )));
}

#[test]
fn elf_parser_accepts_valid_selected_machine_elf() {
    let parsed = ParsedElf::parse(&valid_elf()).expect("valid elf should parse");

    assert_eq!(parsed.entry_vaddr, TEST_IMAGE_ENTRY);
    assert_eq!(parsed.mappings.len(), 1);
    assert_eq!(parsed.mappings[0].file_offset, 0);
    assert_eq!(parsed.mappings[0].file_size, 0x2000);
    assert_eq!(parsed.mappings[0].vaddr, TEST_IMAGE_BASE);
    assert_eq!(parsed.tls, None);
}

#[test]
fn elf_parser_accepts_pt_tls_template() {
    let parsed = ParsedElf::parse(&valid_elf_with_tls()).expect("valid TLS elf should parse");

    assert_eq!(
        parsed.tls,
        Some(bexos_appd::runner::TlsSegment {
            file_offset: 0x2100,
            file_size: 4,
            mem_size: 16,
            align: 16,
        })
    );
}

#[test]
fn elf_parser_rejects_invalid_inputs() {
    let mut bad_magic = valid_elf();
    bad_magic[0] = 0;
    assert_eq!(ParsedElf::parse(&bad_magic), Err(ElfError::BadMagic));

    let mut bad_class = valid_elf();
    bad_class[4] = 1;
    assert_eq!(
        ParsedElf::parse(&bad_class),
        Err(ElfError::UnsupportedClass)
    );

    let mut bad_endian = valid_elf();
    bad_endian[5] = 2;
    assert_eq!(
        ParsedElf::parse(&bad_endian),
        Err(ElfError::UnsupportedEndian)
    );

    let mut bad_machine = valid_elf();
    put_u16(
        &mut bad_machine,
        18,
        if bexos_app_manifest::Architecture::current_guest()
            == bexos_app_manifest::Architecture::Aarch64
        {
            62
        } else {
            183
        },
    );
    assert_eq!(
        ParsedElf::parse(&bad_machine),
        Err(ElfError::UnsupportedMachine)
    );

    let mut wx = valid_elf();
    put_u32(&mut wx, 68, 0x7);
    assert_eq!(ParsedElf::parse(&wx), Err(ElfError::WriteExecuteSegment));

    let mut bounds = valid_elf();
    put_u64(&mut bounds, 72, 0xffff);
    put_u64(&mut bounds, 112, 0);
    assert_eq!(ParsedElf::parse(&bounds), Err(ElfError::SegmentOutOfBounds));

    let mut bad_entry = valid_elf();
    put_u64(&mut bad_entry, 24, TEST_IMAGE_BASE + 0x0100_0000);
    assert_eq!(
        ParsedElf::parse(&bad_entry),
        Err(ElfError::EntryOutsideExecutableSegment)
    );
}

#[test]
fn elf_runner_maps_segments_stack_channel_and_starts_thread() {
    let manifest = launch_manifest();
    let process = &manifest.processes[0];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();

    let result = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect("elf should launch");

    assert_eq!(result.process_handle, KernelHandle { raw: 1 });
    assert_eq!(result.address_space_handle, KernelHandle { raw: 2 });
    assert_eq!(result.service_manager_handle, KernelHandle { raw: 9 });
    assert!(
        !kernel
            .operations
            .iter()
            .any(|operation| matches!(operation, KernelOperation::MapInVmSpace { .. }))
    );
    assert!(matches!(
        kernel.operations.first(),
        Some(KernelOperation::CreateProcess {
            name,
            resource_group_id: 1,
            package_id,
            hardware_access: HardwareAccessTier::None,
            realtime_scheduling: false,
        }) if name == "bexos.hardware:camera:camera_service"
            && package_id == "bexos.hardware:camera"
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::CreateSubVmar {
            parent_vmar: KernelHandle { raw: 3 },
            offset,
            size_bytes: 0x2000,
            flags,
        } if *offset == TEST_IMAGE_BASE - bexos_boot::USER_START
            && *flags & 0x0000_0008 != 0)
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::MapVmoInVmar {
            vmo: KernelHandle { raw: 70 },
            vmo_offset: 0,
            size_bytes: 0x2000,
            flags,
            ..
        } if *flags == (0x0000_0001 | 0x0000_0004))
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::CreateSubVmar {
            offset,
            size_bytes,
            flags: 0x0000_0009,
            ..
        } if *offset == bexos_appd::runner::DEFAULT_STACK_TOP
                - bexos_appd::runner::DEFAULT_STACK_SIZE
                - bexos_appd::runner::PAGE_SIZE
                - bexos_boot::USER_START
            && *size_bytes == bexos_appd::runner::PAGE_SIZE)
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::StartThreadInProcess {
            process: KernelHandle { raw: 1 },
            vm_space: KernelHandle { raw: 2 },
            entry_vaddr: TEST_IMAGE_ENTRY,
            stack_top_vaddr,
            thread_pointer_vaddr: 0,
            arg_handle: Some(KernelHandle { raw: 10 }),
        } if *stack_top_vaddr == bexos_appd::runner::DEFAULT_STACK_TOP)
    ));
    assert!(kernel.operations.iter().any(|operation| matches!(
        operation,
        KernelOperation::CloseHandle {
            handle: KernelHandle { raw: 3 }
        }
    )));
}

#[test]
fn repeated_driver_launches_borrow_code_and_keep_writable_pages_private() {
    let manifest = launch_manifest();
    let mut image = valid_elf();
    image.resize(0x3000, 0x5a);
    put_u16(&mut image, 56, 2);
    put_u32(&mut image, 120, 1);
    put_u32(&mut image, 124, 6);
    put_u64(&mut image, 128, 0x2000);
    put_u64(&mut image, 136, TEST_IMAGE_BASE + 0x2000);
    put_u64(&mut image, 144, TEST_IMAGE_BASE + 0x2000);
    put_u64(&mut image, 152, 0x1000);
    put_u64(&mut image, 160, 0x1000);
    put_u64(&mut image, 168, 0x1000);
    let borrowed = 0x100000;
    let resolver = StaticPackageImageResolver::new(image, borrowed);
    let mut kernel = FakeKernelOps::new();
    let runner = RunnerRegistry::new();
    let request = LaunchRequest {
        manifest: &manifest,
        process: &manifest.processes[0],
        trust_tier: PackageTrustTier::SystemHardware,
        identity: package_identity(&manifest, PackageTrustTier::SystemHardware, true),
        runner_policy: None,
        hardware_access: HardwareAccessTier::None,
        realtime_scheduling: false,
        resource_group_id: 1,
    };
    let first = runner.launch(&request, &mut kernel, &resolver).unwrap();
    let second = runner.launch(&request, &mut kernel, &resolver).unwrap();
    assert_ne!(first.process_handle, second.process_handle);
    let writable: Vec<_> = kernel
        .operations
        .iter()
        .filter_map(|operation| match operation {
            KernelOperation::MapVmoInVmar {
                vmo,
                flags,
                size_bytes: 0x1000,
                ..
            } if flags & 2 != 0 => Some(vmo.raw),
            _ => None,
        })
        .collect();
    assert_eq!(writable.len(), 2);
    assert_ne!(writable[0], writable[1]);
    assert!(!writable.contains(&borrowed));
    assert_eq!(
        kernel
            .operations
            .iter()
            .filter(|operation| matches!(operation,
                KernelOperation::MapVmoInVmar { vmo, flags: 5, .. } if vmo.raw == borrowed
            ))
            .count(),
        2
    );
}

#[test]
fn elf_runner_maps_tls_and_starts_thread_with_thread_pointer() {
    let manifest = launch_manifest();
    let process = &manifest.processes[0];
    let resolver = StaticPackageImageResolver::new(valid_elf_with_tls(), 70);
    let mut kernel = FakeKernelOps::new();

    RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect("TLS elf should launch");

    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::CreateVmoFromBytes {
                size_bytes
            } if *size_bytes == bexos_appd::runner::PAGE_SIZE * 2)
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::CreateSubVmar {
                offset,
                size_bytes,
                flags,
                ..
            } if *offset == TEST_TLS_BASE - bexos_boot::USER_START
                && *size_bytes == bexos_appd::runner::PAGE_SIZE * 2
                && *flags == (0x0000_0001 | 0x0000_0002 | 0x0000_0008))
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::MapVmoInVmar {
                flags,
                ..
            } if *flags == (0x0000_0001 | 0x0000_0002))
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::StartThreadInProcess {
                thread_pointer_vaddr,
                ..
            } if *thread_pointer_vaddr == TEST_TLS_BASE + if bexos_app_manifest::Architecture::current_guest() == bexos_app_manifest::Architecture::X86_64 { 4096 } else { 0 })
    ));
}

#[test]
fn elf_runner_loads_declared_crypto_library_and_returns_link_map() {
    let mut manifest = launch_manifest();
    manifest
        .library_dependencies
        .push(bexos_appd::LibraryDependency {
            package_name: "bexos.lib.crypto".to_string(),
            version_requirement: Some("^1".to_string()),
            mount_alias: None,
            abi_version: 1,
        });
    let process = &manifest.processes[0];
    let crypto = read_runfile("lib/crypto/libbexos_crypto.so");
    let resolver = StaticPackageImageResolver::new(elf_with_runtime_symbols(), 70).with_library(
        "bexos.lib.crypto",
        "bexos_crypto_",
        crypto,
        71,
    );
    let mut kernel = FakeKernelOps::new();

    let result = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect("elf should launch with crypto library");

    let (link_map, link_map_len) = result.runtime_linker_data.expect("runtime linker data");
    assert!(link_map.raw != 0);
    assert!(link_map_len > 16);
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::CreateSubVmar {
                offset,
                ..
            } if *offset == TEST_LIBRARY_BASE - bexos_boot::USER_START)
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::MapVmoInVmar {
                flags,
                ..
            } if *flags == 0x0000_0001)
    ));
    assert!(kernel.operations.iter().any(
        |operation| matches!(operation, KernelOperation::CreateVmoFromBytes {
                size_bytes
            } if *size_bytes == link_map_len)
    ));
}

#[test]
fn elf_runner_exports_declared_net_library_symbols() {
    let mut manifest = launch_manifest();
    manifest
        .library_dependencies
        .push(bexos_appd::LibraryDependency {
            package_name: "bexos.lib.net".to_string(),
            version_requirement: None,
            mount_alias: None,
            abi_version: 1,
        });
    let process = &manifest.processes[0];
    let net = read_runfile("lib/net/libbexos_net.so");
    let resolver = StaticPackageImageResolver::new(elf_with_runtime_symbols(), 70).with_library(
        "bexos.lib.net",
        "bexos_net_",
        net,
        71,
    );
    let mut kernel = FakeKernelOps::new();

    let result = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect("elf should launch with net library");

    assert!(result.runtime_linker_data.is_some());
}

#[test]
fn elf_runner_rejects_soname_mismatch() {
    let mut manifest = launch_manifest();
    manifest
        .library_dependencies
        .push(bexos_appd::LibraryDependency {
            package_name: "bexos.lib.crypto".to_string(),
            version_requirement: None,
            mount_alias: None,
            abi_version: 1,
        });
    let process = &manifest.processes[0];
    let crypto = read_runfile("lib/crypto/libbexos_crypto.so");
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70).with_library_metadata(
        "bexos.lib.crypto",
        "crypto",
        "wrong-soname.so",
        "bexos_crypto_",
        Vec::new(),
        crypto,
        71,
    );
    let mut kernel = FakeKernelOps::new();

    let err = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect_err("SONAME mismatch should fail");

    assert_eq!(err, LaunchError::Elf(ElfError::SonameMismatch));
}

#[test]
fn elf_runner_rejects_library_dependency_cycle() {
    let mut manifest = launch_manifest();
    manifest
        .library_dependencies
        .push(bexos_appd::LibraryDependency {
            package_name: "cycle.a".to_string(),
            version_requirement: None,
            mount_alias: None,
            abi_version: 1,
        });
    let process = &manifest.processes[0];
    let crypto = read_runfile("lib/crypto/libbexos_crypto.so");
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70)
        .with_library_metadata(
            "cycle.a",
            "a",
            "libbexos_crypto.so",
            "bexos_crypto_",
            vec![bexos_appd::PackageLibraryDependency {
                package_name: "cycle.b".to_string(),
                abi_version: 1,
                soname: "libbexos_crypto.so".to_string(),
            }],
            crypto.clone(),
            71,
        )
        .with_library_metadata(
            "cycle.b",
            "b",
            "libbexos_crypto.so",
            "bexos_crypto_",
            vec![bexos_appd::PackageLibraryDependency {
                package_name: "cycle.a".to_string(),
                abi_version: 1,
                soname: "libbexos_crypto.so".to_string(),
            }],
            crypto,
            72,
        );
    let mut kernel = FakeKernelOps::new();

    let err = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect_err("dependency cycle should fail");

    assert_eq!(err, LaunchError::Elf(ElfError::LibraryDependencyCycle));
}

#[test]
fn elf_runner_rejects_text_relocation_dynamic_flags() {
    let mut manifest = launch_manifest();
    manifest
        .library_dependencies
        .push(bexos_appd::LibraryDependency {
            package_name: "bexos.lib.net".to_string(),
            version_requirement: None,
            mount_alias: None,
            abi_version: 1,
        });
    let process = &manifest.processes[0];
    let mut net = read_runfile("lib/net/libbexos_net.so");
    mark_dynamic_flags_textrel(&mut net);
    let resolver = StaticPackageImageResolver::new(elf_with_runtime_symbols(), 70).with_library(
        "bexos.lib.net",
        "bexos_net_",
        net,
        71,
    );
    let mut kernel = FakeKernelOps::new();

    let err = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect_err("DF_TEXTREL should fail");

    assert_eq!(err, LaunchError::Elf(ElfError::TextRelocation));
}

#[test]
fn elf_runner_rejects_missing_declared_library() {
    let mut manifest = launch_manifest();
    manifest
        .library_dependencies
        .push(bexos_appd::LibraryDependency {
            package_name: "bexos.lib.crypto".to_string(),
            version_requirement: None,
            mount_alias: None,
            abi_version: 1,
        });
    let process = &manifest.processes[0];
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();

    let err = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process,
                trust_tier: PackageTrustTier::SystemHardware,
                identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 1,
            },
            &mut kernel,
            &resolver,
        )
        .expect_err("missing declared library should fail");

    assert_eq!(err, LaunchError::Elf(ElfError::MissingLibraryDependency));
}

fn camera_service(bind_permission: Option<&str>, lifecycle: Lifecycle) -> ExposedService {
    ExposedService {
        name: "bexos.hardware.Camera".to_string(),
        protocol: "CameraController".to_string(),
        lifecycle,
        visibility: Visibility::Public,
        bind_permission: bind_permission.map(str::to_string),
        metadata: vec![Metadata {
            key: "supports_capture".to_string(),
            value: "true".to_string(),
        }],
        capabilities: Vec::new(),
        ..Default::default()
    }
}

fn client(permissions: &[&str], is_foreground: bool) -> ClientContext {
    ClientContext {
        package_name: "com.example:client".to_string(),
        permissions: permissions
            .iter()
            .map(|permission| permission.to_string())
            .collect(),
        permission_values: permissions
            .iter()
            .map(|permission| PermissionValueGrant {
                permission: permission.to_string(),
                values: vec!["default".to_string()],
            })
            .collect(),
        is_foreground,
        user_id: Some(1),
    }
}

fn read_runfile(suffix: &str) -> Vec<u8> {
    let runfiles = std::env::var("TEST_SRCDIR").expect("TEST_SRCDIR");
    let direct = std::path::PathBuf::from(&runfiles)
        .join("_main")
        .join(suffix);
    if let Ok(bytes) = std::fs::read(&direct) {
        return bytes;
    }
    let mut stack = vec![std::path::PathBuf::from(runfiles)];
    while let Some(path) = stack.pop() {
        if path.to_string_lossy().ends_with(suffix) {
            return std::fs::read(path).expect("runfile read");
        }
        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                stack.push(entry.path());
            }
        }
    }
    panic!("missing runfile suffix {suffix}");
}

struct StaticPackageImageResolver {
    bytes: Vec<u8>,
    vmo: KernelHandle,
    libraries: Vec<StaticLibrary>,
}

struct StaticLibrary {
    package: &'static str,
    export_name: &'static str,
    soname: &'static str,
    symbol_prefix: &'static str,
    abi_version: u32,
    bytes: Vec<u8>,
    vmo: KernelHandle,
    direct_dependencies: Vec<bexos_appd::PackageLibraryDependency>,
}

impl StaticPackageImageResolver {
    fn new(bytes: Vec<u8>, vmo: u64) -> Self {
        Self {
            bytes,
            vmo: KernelHandle { raw: vmo },
            libraries: Vec::new(),
        }
    }

    fn with_library(
        mut self,
        package: &'static str,
        symbol_prefix: &'static str,
        bytes: Vec<u8>,
        vmo: u64,
    ) -> Self {
        self.libraries.push(StaticLibrary {
            package,
            export_name: "default",
            soname: if package == "bexos.lib.net" {
                "libbexos_net.so"
            } else {
                "libbexos_crypto.so"
            },
            symbol_prefix,
            abi_version: 1,
            bytes,
            vmo: KernelHandle { raw: vmo },
            direct_dependencies: Vec::new(),
        });
        self
    }

    fn with_library_metadata(
        mut self,
        package: &'static str,
        export_name: &'static str,
        soname: &'static str,
        symbol_prefix: &'static str,
        direct_dependencies: Vec<bexos_appd::PackageLibraryDependency>,
        bytes: Vec<u8>,
        vmo: u64,
    ) -> Self {
        self.libraries.push(StaticLibrary {
            package,
            export_name,
            soname,
            symbol_prefix,
            abi_version: 1,
            bytes,
            vmo: KernelHandle { raw: vmo },
            direct_dependencies,
        });
        self
    }
}

impl PackageImageResolver for StaticPackageImageResolver {
    fn resolve_executable<'a>(
        &'a self,
        _package_name: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        assert_eq!(path, "/pkg/bin/camera_service");
        Ok(PackageImage {
            bytes: &self.bytes,
            vmo: self.vmo,
            vmo_offset: 0,
        })
    }

    fn resolve_library<'a>(
        &'a self,
        package_name: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        let library = self
            .libraries
            .iter()
            .find(|library| library.package == package_name && library.abi_version == abi_version)
            .ok_or(PackageImageError::NotFound)?;
        if package_name != library.package || abi_version != 1 {
            return Err(PackageImageError::NotFound);
        }
        Ok(PackageLibrary {
            package_name: library.package,
            export_name: library.export_name,
            soname: library.soname,
            image: PackageImage {
                bytes: &library.bytes,
                vmo: library.vmo,
                vmo_offset: 0,
            },
            symbol_prefix: library.symbol_prefix,
            abi_version,
            kind: PackageLibraryKind::Native,
            direct_dependencies: &library.direct_dependencies,
        })
    }
}

fn launch_manifest() -> Manifest {
    Manifest {
        architecture: bexos_app_manifest::Architecture::current_guest(),
        package_name: "bexos.hardware:camera".to_string(),
        name: "Camera Provider".to_string(),
        processes: vec![bexos_appd::Process {
            name: "camera_service".to_string(),
            runner: "elf".to_string(),
            permissions: Vec::new(),
            service: true,
            depends_on: Vec::new(),
            link: None,
            runner_options: Some(ProcessRunnerOptions::Elf(bexos_appd::ElfRunnerOptions {
                path: "/pkg/bin/camera_service".to_string(),
            })),
            wave: Some(1),
            lifecycle: bexos_appd::ProcessLifecycle {
                update_strategy: bexos_appd::UpdateStrategy::HeartTransplant,
                ..Default::default()
            },
            resource_group: None,
            network_domain: None,
            shell_role: Default::default(),
            handles: Vec::new(),
        }],
        permissions: Vec::new(),
        services_exposed: Vec::new(),
        services_consumed: Vec::new(),
        resource_groups: Vec::new(),
        driver_info: None,
        bind_rules: Vec::new(),
        config_schema: Default::default(),
        package_kind: Default::default(),
        library_dependencies: Vec::new(),
        package_version: Default::default(),
        multi_version_policy: Default::default(),
        min_bexos_abi_version: 0,
        library_exports: Vec::new(),
        jobs: Vec::new(),
        shared_vaults: Vec::new(),
        trusted_app: None,
        commands: vec![],
    }
}

fn package_identity<'a>(
    manifest: &'a Manifest,
    trust_tier: PackageTrustTier,
    is_driver: bool,
) -> PackageIdentity<'a> {
    PackageIdentity {
        package_id: &manifest.package_name,
        signer: "bexos_official_platform_v1",
        trust_tier,
        is_driver,
    }
}

fn platform_config_bytes() -> Vec<u8> {
    let metadata = message(&[
        string_field(1, "//device/virtual/qemu/base/aarch64"),
        string_field(2, "virtual_aarch64"),
        varint_field(3, 1),
        varint_field(4, 1),
    ]);
    let microvm = message(&[
        varint_field(1, 1024),
        varint_field(2, 0),
        varint_field(3, 0),
    ]);
    let native_grant = message(&[
        string_field(1, "bexos.platform.appd"),
        string_field(2, "bexos_official_platform_v1"),
    ]);
    let runner_policy = message(&[
        varint_field(1, 1),
        varint_field(2, 1),
        message_field(3, &microvm),
        varint_field(4, 0),
        message_field(5, &native_grant),
    ]);
    let tee_policy = message(&[
        varint_field(1, 1),
        varint_field(2, 1),
        string_field(3, "sha256:test"),
        string_field(4, "bexos.ta.keymint"),
        string_field(4, "bexos.ta.gatekeeper"),
        string_field(4, "bexos.ta.storage"),
        string_field(4, "bexos.ta.avb"),
        string_field(4, "bexos.ta.authmgr"),
        string_field(4, "bexos.ta.orchestrator"),
    ]);
    let tier_1 = message(&[
        string_field(1, "bexos.driver.storage.nvme"),
        string_field(2, "bexos_official_platform_v1"),
    ]);
    let tier_2_rules = message(&[
        varint_field(1, 1),
        varint_field(2, 0),
        varint_field(3, 1000),
    ]);
    let driver_policy = message(&[
        varint_field(1, 1),
        message_field(2, &tier_1),
        message_field(3, &tier_2_rules),
    ]);
    let update_key = message(&[
        string_field(1, "bexos-qemu-test-ed25519-key-v001"),
        string_field(
            2,
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        ),
    ]);
    let update_policy = message(&[
        string_field(1, "https://repo.example/metadata"),
        string_field(2, "https://repo.example/targets"),
        message_field(3, &update_key),
        string_field(5, "apps/"),
        string_field(6, "kernel.img"),
        string_field(7, "tee.bin"),
        varint_field(8, 1048576),
        varint_field(9, 67108864),
        varint_field(10, 1),
    ]);

    message(&[
        message_field(1, &metadata),
        message_field(2, &runner_policy),
        message_field(3, &tee_policy),
        message_field(4, &driver_policy),
        message_field(5, &update_policy),
    ])
}

fn wave_manifest(package_name: &str, processes: &[(&str, Option<u32>)]) -> Manifest {
    Manifest {
        architecture: bexos_app_manifest::Architecture::current_guest(),
        package_name: package_name.to_string(),
        name: "Wave Test".to_string(),
        processes: processes
            .iter()
            .map(|(name, wave)| bexos_appd::Process {
                name: (*name).to_string(),
                runner: "elf".to_string(),
                permissions: Vec::new(),
                service: true,
                depends_on: Vec::new(),
                link: None,
                runner_options: Some(ProcessRunnerOptions::Elf(bexos_appd::ElfRunnerOptions {
                    path: "/pkg/bin/camera_service".to_string(),
                })),
                wave: *wave,
                lifecycle: Default::default(),
                resource_group: None,
                network_domain: None,
                shell_role: Default::default(),
                handles: Vec::new(),
            })
            .collect(),
        permissions: Vec::new(),
        services_exposed: Vec::new(),
        services_consumed: Vec::new(),
        resource_groups: Vec::new(),
        driver_info: None,
        bind_rules: Vec::new(),
        config_schema: Default::default(),
        package_kind: Default::default(),
        library_dependencies: Vec::new(),
        package_version: Default::default(),
        multi_version_policy: Default::default(),
        min_bexos_abi_version: 0,
        library_exports: Vec::new(),
        jobs: Vec::new(),
        shared_vaults: Vec::new(),
        trusted_app: None,
        commands: vec![],
    }
}

fn nvme_driver_manifest(package_name: &str, wave: u32, vendor_specific: bool) -> Manifest {
    let mut properties = vec![
        BindProperty {
            key: "pci.class".to_string(),
            value: 0x01,
        },
        BindProperty {
            key: "pci.subclass".to_string(),
            value: 0x08,
        },
        BindProperty {
            key: "pci.prog_if".to_string(),
            value: 0x02,
        },
    ];
    if vendor_specific {
        properties.push(BindProperty {
            key: "pci.vendor_id".to_string(),
            value: 0x1b36,
        });
        properties.push(BindProperty {
            key: "pci.device_id".to_string(),
            value: 0x0010,
        });
    }

    Manifest {
        architecture: bexos_app_manifest::Architecture::current_guest(),
        package_name: package_name.to_string(),
        name: "NVMe Driver".to_string(),
        processes: vec![bexos_appd::Process {
            name: "nvme_driver".to_string(),
            runner: "elf".to_string(),
            permissions: Vec::new(),
            service: true,
            depends_on: Vec::new(),
            link: None,
            runner_options: Some(ProcessRunnerOptions::Elf(bexos_appd::ElfRunnerOptions {
                path: "/pkg/bin/camera_service".to_string(),
            })),
            wave: Some(wave),
            lifecycle: Default::default(),
            resource_group: None,
            network_domain: None,
            shell_role: Default::default(),
            handles: Vec::new(),
        }],
        permissions: Vec::new(),
        services_exposed: Vec::new(),
        services_consumed: Vec::new(),
        resource_groups: Vec::new(),
        driver_info: Some(DriverInfo {
            name: "nvme_driver".to_string(),
            package_id: package_name.to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        }),
        bind_rules: vec![BindRule {
            conditions: vec![BindCondition {
                bus: BindBusType::Pci,
                properties,
            }],
            priority: 0,
        }],
        config_schema: Default::default(),
        package_kind: Default::default(),
        library_dependencies: Vec::new(),
        package_version: Default::default(),
        multi_version_policy: Default::default(),
        min_bexos_abi_version: 0,
        library_exports: Vec::new(),
        jobs: Vec::new(),
        shared_vaults: Vec::new(),
        trusted_app: None,
        commands: vec![],
    }
}

fn peripheral_driver_manifest(
    package_name: &str,
    bus: BindBusType,
    property_key: &str,
    property_value: u32,
) -> Manifest {
    Manifest {
        architecture: bexos_app_manifest::Architecture::current_guest(),
        package_name: package_name.to_string(),
        name: package_name.to_string(),
        processes: vec![bexos_appd::Process {
            name: "peripheral_driver".to_string(),
            runner: "elf".to_string(),
            permissions: Vec::new(),
            service: true,
            depends_on: Vec::new(),
            link: None,
            runner_options: Some(ProcessRunnerOptions::Elf(bexos_appd::ElfRunnerOptions {
                path: "/pkg/bin/peripheral_driver".to_string(),
            })),
            wave: Some(3),
            lifecycle: bexos_appd::ProcessLifecycle {
                update_strategy: bexos_appd::UpdateStrategy::HeartTransplant,
                ..Default::default()
            },
            resource_group: None,
            network_domain: None,
            shell_role: Default::default(),
            handles: Vec::new(),
        }],
        permissions: Vec::new(),
        services_exposed: Vec::new(),
        services_consumed: Vec::new(),
        resource_groups: Vec::new(),
        driver_info: Some(DriverInfo {
            name: "peripheral_driver".to_string(),
            package_id: package_name.to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        }),
        bind_rules: vec![BindRule {
            conditions: vec![BindCondition {
                bus,
                properties: vec![BindProperty {
                    key: property_key.to_string(),
                    value: property_value,
                }],
            }],
            priority: 0,
        }],
        config_schema: Default::default(),
        package_kind: Default::default(),
        library_dependencies: Vec::new(),
        package_version: Default::default(),
        multi_version_policy: Default::default(),
        min_bexos_abi_version: 0,
        library_exports: Vec::new(),
        jobs: Vec::new(),
        shared_vaults: Vec::new(),
        trusted_app: None,
        commands: vec![],
    }
}

fn system_driver_identity(manifest: &Manifest) -> PackageIdentity<'_> {
    PackageIdentity {
        package_id: &manifest.package_name,
        signer: "bexos_official_platform_v1",
        trust_tier: PackageTrustTier::SystemHardware,
        is_driver: true,
    }
}

fn nvme_device_node(node_id: u64) -> RegisteredDeviceNode {
    RegisteredDeviceNode {
        info: DeviceNodeInfo {
            node_id,
            bus: BusType::Pci,
            parent_node_id: None,
            topological_path: format!("pci/{node_id}"),
            properties: vec![
                DeviceProperty {
                    key: "pci.vendor_id".to_string(),
                    value: 0x1b36,
                },
                DeviceProperty {
                    key: "pci.device_id".to_string(),
                    value: 0x0010,
                },
                DeviceProperty {
                    key: "pci.class".to_string(),
                    value: 0x01,
                },
                DeviceProperty {
                    key: "pci.subclass".to_string(),
                    value: 0x08,
                },
                DeviceProperty {
                    key: "pci.prog_if".to_string(),
                    value: 0x02,
                },
            ],
        },
        resources: vec![HardwareResourceLease {
            kind: HardwareResourceKind::Mmio,
            resource_id: 0,
            base: 0x1000,
            length: 0x4000,
            flags: 0,
            capability: Capability {
                object_id: 100 + node_id,
                rights: 0b11,
            },
        }],
        mmio_vmo: Some(Capability {
            object_id: 100 + node_id,
            rights: 0b11,
        }),
        irq_channel: None,
        registrar: None,
        present: true,
        state: Default::default(),
    }
}

fn child_device_node(node_id: u64, parent_node_id: u64) -> RegisteredDeviceNode {
    RegisteredDeviceNode {
        info: DeviceNodeInfo {
            node_id,
            bus: BusType::Pci,
            parent_node_id: Some(parent_node_id),
            topological_path: format!("pci/{parent_node_id}/{node_id}"),
            properties: vec![DeviceProperty {
                key: "pci.vendor_id".to_string(),
                value: node_id as u32,
            }],
        },
        resources: Vec::new(),
        mmio_vmo: None,
        irq_channel: None,
        registrar: None,
        present: true,
        state: Default::default(),
    }
}

#[derive(Default)]
struct RecordingReadiness {
    ready_processes: Vec<String>,
    fail_on: Option<&'static str>,
    registry: DeviceRegistry,
    register_on_ready: Option<&'static str>,
}

impl ReadinessGate for RecordingReadiness {
    fn wait_ready(
        &mut self,
        launched: &bexos_appd::LaunchedProcess<'_>,
    ) -> Result<(), ReadinessError> {
        if self.fail_on == Some(launched.process_ref.process.name.as_str()) {
            return Err(ReadinessError::TimedOut);
        }
        if self.register_on_ready == Some(launched.process_ref.manifest.package_name.as_str())
            && self.registry.nodes().is_empty()
        {
            self.registry
                .register_device_node(nvme_device_node(7))
                .expect("device node should register during readiness");
        }

        self.ready_processes.push(format!(
            "{}:{}",
            launched.process_ref.manifest.package_name, launched.process_ref.process.name
        ));
        Ok(())
    }

    fn registered_device_nodes(&self) -> &[RegisteredDeviceNode] {
        self.registry.nodes()
    }

    fn begin_device_binding(
        &mut self,
        node_id: u64,
        package_id: String,
        process_name: String,
        _hardware_access: HardwareAccessTier,
    ) -> Result<(), DeviceRegistryError> {
        self.registry
            .begin_binding(node_id, package_id, process_name)
    }

    fn finish_device_binding(
        &mut self,
        node_id: u64,
        binding: bexos_appd::DriverBinding,
    ) -> Result<(), DeviceRegistryError> {
        self.registry.bind_active(
            node_id,
            binding.process_handle,
            binding.manager_channel,
            binding.lifecycle_channel,
        )
    }

    fn fail_device_binding(
        &mut self,
        node_id: u64,
        package_id: String,
        process_name: String,
    ) -> Result<(), DeviceRegistryError> {
        self.registry.bind_failed(node_id, package_id, process_name)
    }
}

fn valid_elf() -> Vec<u8> {
    let mut bytes = vec![0; 0x2200];
    bytes[0..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    put_u16(&mut bytes, 16, 2);
    put_u16(
        &mut bytes,
        18,
        bexos_app_manifest::Architecture::current_guest()
            .elf_machine()
            .unwrap(),
    );
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, TEST_IMAGE_ENTRY);
    put_u64(&mut bytes, 32, 64);
    put_u16(&mut bytes, 52, 64);
    put_u16(&mut bytes, 54, 56);
    put_u16(&mut bytes, 56, 1);

    put_u32(&mut bytes, 64, 1);
    put_u32(&mut bytes, 68, 0x5);
    put_u64(&mut bytes, 72, 0);
    put_u64(&mut bytes, 80, TEST_IMAGE_BASE);
    put_u64(&mut bytes, 88, TEST_IMAGE_BASE);
    put_u64(&mut bytes, 96, 0x2000);
    put_u64(&mut bytes, 104, 0x2000);
    put_u64(&mut bytes, 112, 0x1000);

    bytes
}

fn valid_elf_with_tls() -> Vec<u8> {
    let mut bytes = valid_elf();
    put_u16(&mut bytes, 56, 2);

    let offset = 64 + 56;
    put_u32(&mut bytes, offset, 7);
    put_u64(&mut bytes, offset + 8, 0x2100);
    put_u64(&mut bytes, offset + 16, TEST_TLS_PHDR_VADDR);
    put_u64(&mut bytes, offset + 24, TEST_TLS_PHDR_VADDR);
    put_u64(&mut bytes, offset + 32, 4);
    put_u64(&mut bytes, offset + 40, 16);
    put_u64(&mut bytes, offset + 48, 16);
    bytes[0x2100..0x2104].copy_from_slice(&[1, 2, 3, 4]);
    bytes
}

fn message(fields: &[Vec<u8>]) -> Vec<u8> {
    fields.iter().flatten().copied().collect()
}

fn permission(name: &str, values: &[&str], requirement: u64) -> Vec<u8> {
    let mut fields = vec![string_field(1, name), varint_field(3, requirement)];
    fields.extend(values.iter().map(|value| string_field(2, value)));
    message(&fields)
}

fn shared_vault_manifest_bytes(package_name: &str, vault_name: &str, peers: &[&str]) -> Vec<u8> {
    let mut vault_fields = vec![string_field(1, vault_name), varint_field(2, 1)];
    vault_fields.extend(peers.iter().map(|peer| string_field(3, peer)));
    message(&[
        string_field(1, package_name),
        message_field(18, &message(&vault_fields)),
    ])
}

fn string_field(number: u32, value: &str) -> Vec<u8> {
    let mut out = key(number, 2);
    out.extend(varint(value.len() as u64));
    out.extend(value.as_bytes());
    out
}

fn message_field(number: u32, value: &[u8]) -> Vec<u8> {
    let mut out = key(number, 2);
    out.extend(varint(value.len() as u64));
    out.extend(value);
    out
}

fn bytes_field(number: u32, value: &[u8]) -> Vec<u8> {
    message_field(number, value)
}

fn varint_field(number: u32, value: u64) -> Vec<u8> {
    let mut out = key(number, 0);
    out.extend(varint(value));
    out
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn mark_dynamic_flags_textrel(bytes: &mut [u8]) {
    let phoff = get_u64(bytes, 32) as usize;
    let phentsize = get_u16(bytes, 54) as usize;
    let phnum = get_u16(bytes, 56) as usize;
    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        if get_u32(bytes, offset) != 2 {
            continue;
        }
        let dynamic_offset = get_u64(bytes, offset + 8) as usize;
        let dynamic_size = get_u64(bytes, offset + 32) as usize;
        let mut cursor = dynamic_offset;
        while cursor + 16 <= dynamic_offset + dynamic_size {
            let tag = get_u64(bytes, cursor);
            if tag == 0 {
                break;
            }
            if tag == 0x1e {
                let value = get_u64(bytes, cursor + 8) | 0x4;
                put_u64(bytes, cursor + 8, value);
                return;
            }
            cursor += 16;
        }
    }
    panic!("fixture missing DT_FLAGS");
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn key(number: u32, wire_type: u8) -> Vec<u8> {
    varint((u64::from(number) << 3) | u64::from(wire_type))
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return out;
        }
    }
}

fn semver(major: u32, minor: u32, patch: u32) -> SemVer {
    SemVer {
        major,
        minor,
        patch,
        build: 0,
        prerelease: String::new(),
    }
}

// A synthetic executable with explicit runtime ABI symbols for real library fixtures.
fn elf_with_runtime_symbols() -> Vec<u8> {
    let mut bytes = valid_elf();
    let names = [
        "__bexos_tls_start",
        "__bexos_tls_file_end",
        "__bexos_pthread_local",
        "__bexos_tls_end",
        "__bexos_tls_alignment",
        "__tls_get_addr",
    ];
    let mut strings = vec![0];
    let mut symbols = vec![0; 24];
    for (index, name) in names.iter().enumerate() {
        let name_offset = strings.len() as u32;
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
        let mut symbol = [0; 24];
        symbol[..4].copy_from_slice(&name_offset.to_le_bytes());
        symbol[4] = 0x10;
        symbol[6..8].copy_from_slice(&0xfff1u16.to_le_bytes());
        let address: u64 = match index {
            4 => 16,
            5 => 0x8000_0000,
            _ => 0x8000_1000,
        };
        symbol[8..16].copy_from_slice(&address.to_le_bytes());
        symbols.extend_from_slice(&symbol);
    }
    let string_start = bytes.len();
    bytes.extend_from_slice(&strings);
    let symbol_start = bytes.len();
    bytes.extend_from_slice(&symbols);
    let section_start = bytes.len();
    bytes.resize(section_start + 192, 0);
    bytes[40..48].copy_from_slice(&(section_start as u64).to_le_bytes());
    bytes[58..60].copy_from_slice(&64u16.to_le_bytes());
    bytes[60..62].copy_from_slice(&3u16.to_le_bytes());
    for (index, kind, start, len) in [
        (1, 3u32, string_start, strings.len()),
        (2, 2u32, symbol_start, symbols.len()),
    ] {
        let base = section_start + index * 64;
        bytes[base + 4..base + 8].copy_from_slice(&kind.to_le_bytes());
        bytes[base + 24..base + 32].copy_from_slice(&(start as u64).to_le_bytes());
        bytes[base + 32..base + 40].copy_from_slice(&(len as u64).to_le_bytes());
    }
    let base = section_start + 128;
    bytes[base + 40..base + 44].copy_from_slice(&1u32.to_le_bytes());
    bytes[base + 56..base + 64].copy_from_slice(&24u64.to_le_bytes());
    bytes
}

#[test]
fn elf_partial_load_failure_closes_handles_and_terminates_the_process() {
    let manifest = launch_manifest();
    let resolver = StaticPackageImageResolver::new(valid_elf_with_tls(), 70);
    let request = LaunchRequest {
        manifest: &manifest,
        process: &manifest.processes[0],
        trust_tier: PackageTrustTier::SystemHardware,
        identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
        runner_policy: None,
        hardware_access: HardwareAccessTier::None,
        realtime_scheduling: false,
        resource_group_id: 1,
    };
    let mut successful = FakeKernelOps::new();
    RunnerRegistry::new()
        .launch(&request, &mut successful, &resolver)
        .unwrap();
    for call in 1..=successful.call_count() {
        let mut kernel = FakeKernelOps::new();
        kernel.fail_call(call);
        assert!(
            RunnerRegistry::new()
                .launch(&request, &mut kernel, &resolver)
                .is_err(),
            "failure point {call}"
        );
        assert_eq!(
            kernel.live_handle_count(),
            0,
            "leaked handles after failure point {call}: {:?}",
            kernel.operations
        );
        if call > 1 {
            assert!(
                kernel
                    .operations
                    .iter()
                    .any(|operation| matches!(operation, KernelOperation::TerminateProcess { .. })),
                "failure point {call}"
            );
        }
    }
}

#[test]
fn optional_splash_readiness_failure_does_not_block_storage_wave() {
    let manifests = vec![
        wave_manifest("bexos.service.splashd", &[("splashd", Some(1))]),
        wave_manifest("bexos.service.vfsd", &[("vfsd", Some(3))]),
    ];
    let config = PlatformConfig::decode(&platform_config_bytes()).unwrap();
    let resolver = StaticPackageImageResolver::new(valid_elf(), 70);
    let mut kernel = FakeKernelOps::new();
    let mut readiness = RecordingReadiness {
        fail_on: Some("splashd"),
        ..Default::default()
    };
    let launched = AppdWaveOrchestrator::new()
        .launch_automatic_with_policy(
            &manifests,
            &config.runner_policy,
            &config.driver_policy,
            |manifest| PackageIdentity {
                package_id: &manifest.package_name,
                signer: "bexos_official_platform_v1",
                trust_tier: PackageTrustTier::SystemHardware,
                is_driver: false,
            },
            1,
            &mut kernel,
            &resolver,
            &mut readiness,
        )
        .unwrap();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].process_ref.process.name, "vfsd");
    assert_eq!(readiness.ready_processes, vec!["bexos.service.vfsd:vfsd"]);
}

#[test]
fn elf_launches_borrow_cached_executable_for_multiple_device_instances() {
    let manifest = launch_manifest();
    use bexos_appd::KernelOps;
    let mut kernel = FakeKernelOps::new();
    let executable = kernel.create_vmo_from_bytes(&valid_elf()).unwrap();
    let resolver = StaticPackageImageResolver::new(valid_elf(), executable.raw);
    for _ in 0..4 {
        RunnerRegistry::new()
            .launch(
                &LaunchRequest {
                    manifest: &manifest,
                    process: &manifest.processes[0],
                    trust_tier: PackageTrustTier::SystemHardware,
                    identity: package_identity(&manifest, PackageTrustTier::SystemHardware, false),
                    runner_policy: None,
                    hardware_access: HardwareAccessTier::None,
                    realtime_scheduling: false,
                    resource_group_id: 1,
                },
                &mut kernel,
                &resolver,
            )
            .unwrap();
        assert!(
            kernel.is_handle_live(executable),
            "resolver still owns the executable VMO"
        );
    }
}
