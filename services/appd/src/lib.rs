extern crate alloc;

pub mod broker;
pub mod checkpoint;
pub mod command_state;
pub mod commands;
pub mod debug;
pub mod device_registry;
pub mod driver_manager;
pub mod firmware_policy;
pub mod guest;
pub mod hardware_resources;
pub mod kernel_services;
pub mod lazy;
pub mod lifecycle;
pub mod manager;
pub mod manifest;
pub mod namespace;
pub mod opener;
pub mod package_install;
pub mod permission_persistence;
pub mod permission_route;
pub mod platform_config;
pub mod policy;
pub mod recovery;
pub mod recovery_image;
pub mod registry;
pub mod routing;
pub mod runner;
pub mod service_directory;
pub mod shell;
pub mod stores;
pub mod watchdog;
pub mod waves;

pub use bexos_app_registry::{InstallSource, MemoryAppRegistry};
pub use bexos_domain_association::{
    DomainAssociationError, DomainPolicyRecord, MemoryDomainAssociationCache, canonical_domain,
    encode_record as encode_domain_policy_record, parse_well_known_json, record_allows_package,
    validate_distribution_origin, validate_record_for_domain,
};
pub use bexos_opener_store::{
    HandlerId, HandlerRegistration, MemoryOpenerRegistry, OpenKind, OpenerScope, OpenerStoreError,
    ResolveOutcome, ResolvedHandler, UserDefaultSnapshot, UserHandlerSnapshot,
};
pub use bexos_package_version::{HealthCheckStatus, MultiVersionPolicy, SemVer};
pub use bexos_permission_store::{
    MemoryPermissionStore, PermissionDeclaration, PermissionRequirement, PermissionStoreError,
    SystemGrantRecord, UserGrantRecord, UserGrantState,
};
pub use broker::{AppdBroker, BindError, BoundCapability, InterfaceQuery};
pub use checkpoint::{AppdSnapshot, SnapshotError};
pub use debug::{AppDebugProcess, AppDebugProcessRegistry, AppDebugProcessState};
pub use device_registry::{
    BusType, DeviceNodeInfo, DeviceNodeState, DeviceProperty, DeviceRegistrar, DeviceRegistry,
    DeviceRegistryError, DriverBinding, HardwareResourceKind, HardwareResourceLease,
    RegisteredDeviceNode,
};
pub use driver_manager::{
    DriverCandidate, DriverExclusions, DriverIndex, DriverLifecycleLog, ServiceContract,
    driver_package_id,
};
pub use kernel_services::{
    KERNEL_PROVIDER_PACKAGE, SYSTEM_PRIVILEGED_PERMISSION, publish_kernel_services,
};
pub use lifecycle::{PrepareStopResult, prepare_stop};
pub use manager::{
    AppBundleFetcher, AppManagerBinding, WebInstallError, WellKnownFetcher, install_app_from_url,
    reload_well_known_for_domain,
};
pub use manifest::{
    AnyRunnerOptions, BindBusType, BindCondition, BindProperty, BindRule, CapabilityMetadata,
    ComponentConfigField, ComponentConfigSchema, ComponentConfigType, ComponentConfigValue,
    ConsumedCapability, ConsumedService, DEFAULT_LAZY_IDLE_TIMEOUT_MS, DriverInfo,
    ElfRunnerOptions, ExposedService, IntentFilter, JobDefinition, JobNetworkConstraint,
    LibraryDependency, LibraryExport, LibraryExportKind, Lifecycle, Link, LinkType, Manifest,
    ManifestError, Metadata, MethodDependency, PackageKind, Process, ProcessLifecycle,
    ProcessRunnerOptions, ResourceGroup, ServiceActivation, SharedVault, SharedVaultAccess,
    UpdateStrategy, Visibility,
};
pub use namespace::{
    DependencyNamespaceEntry, NamespaceEntry, NamespaceError, SYSTEM_UID,
    SharedVaultNamespaceEntry, StartupNamespace, app_storage_namespace,
    app_storage_namespace_with_dependencies,
    app_storage_namespace_with_shared_vaults_and_dependencies,
};
pub use opener::{OpenerBinding, OpenerRequest, OpenerRequestKind, register_manifest_openers};
pub use permission_route::{PermissionRoute, PermissionRouteTable};
pub use platform_config::{
    AppLifecyclePolicy, Architecture, DriverPolicy, DriverPolicyDecision, DriverUnmatchedAction,
    HardwareAccessTier, MicrovmConstraints, NativeRunnerGrant, PackageIdentity, PlatformConfig,
    PlatformMetadata, RunnerPolicy, RunnerPolicyDecision, RunnerTier, TeePolicy, Tier2DriverRules,
    UpdateApplyPolicy, UpdatePolicy, UpdateRootKey,
};
pub use policy::{
    ClientContext, FidlCapability, PermissionDecision, PermissionValueGrant, allowed_capabilities,
};
pub use recovery::{
    DriverRecoveryBudget, NodeRecoveryCounter, RECOVERY_STABLE_WINDOW_MS, RecoveryDecision,
};
pub use recovery_image::{
    CachedImageVmo, CachedLibraryVmo, DriverRecoveryImage, DriverRecoveryImageCache,
};
pub use registry::{PublishedInterface, RegistryError};
pub use routing::{DriverRouteTable, RetainedProviderEndpoint};
pub use runner::{
    ElfError, ElfMapping, ElfRunner, FakeKernelOps, KernelError, KernelFidlOps, KernelHandle,
    KernelOperation, KernelOps, LaunchError, LaunchRequest, LaunchResult, PackageImage,
    PackageImageError, PackageImageResolver, PackageLibrary, PackageLibraryDependency,
    PackageLibraryKind, PackageTrustTier, ParsedElf, RunnerKind, RunnerOptions, RunnerRegistry,
};
pub use service_directory::ServiceDirectoryBinding;
pub use waves::{
    AppdWaveOrchestrator, ImmediateReadiness, LaunchedProcess, ProcessRef, ReadinessError,
    ReadinessGate, StartupClass, StartupError, StartupPlan, StartupWave,
};
