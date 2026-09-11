use crate::manifest::{Cursor, ManifestError};
use crate::runner::PackageTrustTier;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlatformConfig {
    pub interface_defaults: Vec<InterfaceDefault>,
    pub metadata: PlatformMetadata,
    pub runner_policy: RunnerPolicy,
    pub tee_policy: TeePolicy,
    pub driver_policy: DriverPolicy,
    pub update_policy: UpdatePolicy,
    pub app_lifecycle_policy: AppLifecyclePolicy,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InterfaceDefault {
    pub interface_name: String,
    pub package_name: String,
    pub process_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformMetadata {
    pub target_board: String,
    pub platform_profile: String,
    pub min_platform_version: u32,
    pub architecture: Architecture,
}

impl Default for PlatformMetadata {
    fn default() -> Self {
        Self {
            target_board: String::new(),
            platform_profile: String::new(),
            min_platform_version: 0,
            architecture: Architecture::Unspecified,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Architecture {
    Unspecified,
    Aarch64,
    Riscv64,
    X86_64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerPolicy {
    pub default_tier: RunnerTier,
    pub allow_microvm_runner: bool,
    pub microvm_constraints: MicrovmConstraints,
    pub allow_native_elf_runner: bool,
    pub native_elf_runner_allowlist: Vec<NativeRunnerGrant>,
    pub realtime_scheduling_allowlist: Vec<RealtimeSchedulingGrant>,
}

impl Default for RunnerPolicy {
    fn default() -> Self {
        Self {
            default_tier: RunnerTier::Tier1Wasm,
            allow_microvm_runner: false,
            microvm_constraints: MicrovmConstraints::default(),
            allow_native_elf_runner: false,
            native_elf_runner_allowlist: Vec::new(),
            realtime_scheduling_allowlist: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerTier {
    Unspecified,
    Tier1Wasm,
    Tier2Microvm,
    Tier0PlatformElf,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MicrovmConstraints {
    pub max_vram_allocation_mb: u32,
    pub allow_network_bridge: bool,
    pub allow_pci_passthrough: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeRunnerGrant {
    pub package_id: String,
    pub expected_signer: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RealtimeSchedulingGrant {
    pub package_id: String,
    pub expected_signer: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeePolicy {
    pub enforce_secure_boot: bool,
    pub rpmb_anti_rollback: bool,
    pub orchestrator_verification: String,
    pub allowed_trusted_apps: Vec<String>,
    pub secure_storage_backend: SecureStorageBackend,
    pub require_secure_persistent_keys: bool,
    pub allow_authenticated_firmware_selection: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SecureStorageBackend {
    #[default]
    Unspecified,
    HardwareRpmb,
    QemuEmulatedRpmb,
}

impl SecureStorageBackend {
    fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::HardwareRpmb,
            2 => Self::QemuEmulatedRpmb,
            _ => Self::Unspecified,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverPolicy {
    pub default_unmatched_action: DriverUnmatchedAction,
    pub tier_1_allowlist: Vec<DriverGrant>,
    pub tier_2_rules: Tier2DriverRules,
}

impl Default for DriverPolicy {
    fn default() -> Self {
        Self {
            default_unmatched_action: DriverUnmatchedAction::RejectAndIsolate,
            tier_1_allowlist: Vec::new(),
            tier_2_rules: Tier2DriverRules::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverUnmatchedAction {
    Unspecified,
    RejectAndIsolate,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverGrant {
    pub package_id: String,
    pub expected_signer: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tier2DriverRules {
    pub enforce_strict_iommu: bool,
    pub allow_raw_mmio: bool,
    pub max_dma_bandwidth_mbps: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdatePolicy {
    pub metadata_base_url: String,
    pub targets_base_url: String,
    pub trusted_root_keys: Vec<UpdateRootKey>,
    pub trusted_root_metadata: Vec<u8>,
    pub app_target_prefix: String,
    pub kernel_target_name: String,
    pub tee_target_name: String,
    pub max_metadata_bytes: u64,
    pub max_target_bytes: u64,
    pub default_apply_policy: UpdateApplyPolicy,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            metadata_base_url: String::new(),
            targets_base_url: String::new(),
            trusted_root_keys: Vec::new(),
            trusted_root_metadata: Vec::new(),
            app_target_prefix: "apps/".into(),
            kernel_target_name: "kernel.img".into(),
            tee_target_name: "tee.bin".into(),
            max_metadata_bytes: 1024 * 1024,
            max_target_bytes: 64 * 1024 * 1024,
            default_apply_policy: UpdateApplyPolicy::CheckOnly,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateRootKey {
    pub key_id: String,
    pub public_key: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateApplyPolicy {
    Unspecified,
    CheckOnly,
    StageOnly,
    ApplyImmediately,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppLifecyclePolicy {
    pub probation_window_seconds: u64,
    pub crash_window_seconds: u64,
    pub crash_threshold: u32,
    pub restart_delay_seconds: u64,
}

impl Default for AppLifecyclePolicy {
    fn default() -> Self {
        Self {
            probation_window_seconds: 60,
            crash_window_seconds: 60,
            crash_threshold: 3,
            restart_delay_seconds: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageIdentity<'a> {
    pub package_id: &'a str,
    pub signer: &'a str,
    pub trust_tier: PackageTrustTier,
    pub is_driver: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerPolicyDecision {
    Allow,
    Deny,
    RouteToMicrovm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HardwareAccessTier {
    None,
    Isolated,
    Direct,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverPolicyDecision {
    RejectAndIsolate,
    Allow { hardware_access: HardwareAccessTier },
}

impl PlatformConfig {
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        decode_platform_config(bytes)
    }
}

impl RunnerPolicy {
    pub fn evaluate_runner(
        &self,
        runner: &str,
        identity: PackageIdentity<'_>,
    ) -> RunnerPolicyDecision {
        match runner.trim().to_ascii_lowercase().as_str() {
            "wasm" | "web" => RunnerPolicyDecision::Allow,
            "elf" => self.evaluate_elf(identity),
            "android" | "nix" => {
                if self.allow_microvm_runner {
                    RunnerPolicyDecision::RouteToMicrovm
                } else {
                    RunnerPolicyDecision::Deny
                }
            }
            _ => RunnerPolicyDecision::Deny,
        }
    }

    fn evaluate_elf(&self, identity: PackageIdentity<'_>) -> RunnerPolicyDecision {
        if matches!(
            identity.trust_tier,
            PackageTrustTier::PlatformCore | PackageTrustTier::SystemHardware
        ) {
            return RunnerPolicyDecision::Allow;
        }
        if self.allow_native_elf_runner
            && self.native_elf_runner_allowlist.iter().any(|grant| {
                grant.package_id == identity.package_id && grant.expected_signer == identity.signer
            })
        {
            RunnerPolicyDecision::Allow
        } else {
            RunnerPolicyDecision::Deny
        }
    }

    pub fn permits_realtime_scheduling(&self, identity: PackageIdentity<'_>) -> bool {
        self.realtime_scheduling_allowlist.iter().any(|grant| {
            grant.package_id == identity.package_id && grant.expected_signer == identity.signer
        })
    }
}

impl DriverPolicy {
    pub fn evaluate_driver(&self, identity: PackageIdentity<'_>) -> DriverPolicyDecision {
        if !identity.is_driver {
            return DriverPolicyDecision::Allow {
                hardware_access: HardwareAccessTier::None,
            };
        }

        if self.tier_1_allowlist.iter().any(|grant| {
            grant.package_id == identity.package_id && grant.expected_signer == identity.signer
        }) {
            return DriverPolicyDecision::Allow {
                hardware_access: HardwareAccessTier::Direct,
            };
        }

        if self.tier_2_rules.enforce_strict_iommu && !self.tier_2_rules.allow_raw_mmio {
            return DriverPolicyDecision::Allow {
                hardware_access: HardwareAccessTier::Isolated,
            };
        }

        DriverPolicyDecision::RejectAndIsolate
    }
}

fn decode_platform_config(bytes: &[u8]) -> Result<PlatformConfig, ManifestError> {
    let mut config = PlatformConfig::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => config.metadata = decode_metadata(field.bytes()?)?,
            2 => config.runner_policy = decode_runner_policy(field.bytes()?)?,
            3 => config.tee_policy = decode_tee_policy(field.bytes()?)?,
            4 => config.driver_policy = decode_driver_policy(field.bytes()?)?,
            5 => config.update_policy = decode_update_policy(field.bytes()?)?,
            6 => config.app_lifecycle_policy = decode_app_lifecycle_policy(field.bytes()?)?,
            7 => config
                .interface_defaults
                .push(decode_interface_default(field.bytes()?)?),
            _ => {}
        }
    }
    Ok(config)
}

fn decode_app_lifecycle_policy(bytes: &[u8]) -> Result<AppLifecyclePolicy, ManifestError> {
    let mut policy = AppLifecyclePolicy::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => policy.probation_window_seconds = field.varint()?,
            2 => policy.crash_window_seconds = field.varint()?,
            3 => policy.crash_threshold = field.varint()? as u32,
            4 => policy.restart_delay_seconds = field.varint()?,
            _ => {}
        }
    }
    if policy.probation_window_seconds == 0 {
        policy.probation_window_seconds = 60;
    }
    if policy.crash_window_seconds == 0 {
        policy.crash_window_seconds = 60;
    }
    if policy.crash_threshold == 0 {
        policy.crash_threshold = 3;
    }
    if policy.restart_delay_seconds == 0 {
        policy.restart_delay_seconds = 1;
    }
    Ok(policy)
}

fn decode_metadata(bytes: &[u8]) -> Result<PlatformMetadata, ManifestError> {
    let mut metadata = PlatformMetadata::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => metadata.target_board = field.string()?,
            2 => metadata.platform_profile = field.string()?,
            3 => metadata.min_platform_version = field.varint()? as u32,
            4 => metadata.architecture = Architecture::from_proto(field.varint()?),
            _ => {}
        }
    }
    Ok(metadata)
}

fn decode_runner_policy(bytes: &[u8]) -> Result<RunnerPolicy, ManifestError> {
    let mut policy = RunnerPolicy::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => policy.default_tier = RunnerTier::from_proto(field.varint()?),
            2 => policy.allow_microvm_runner = field.varint()? != 0,
            3 => policy.microvm_constraints = decode_microvm_constraints(field.bytes()?)?,
            4 => policy.allow_native_elf_runner = field.varint()? != 0,
            5 => policy
                .native_elf_runner_allowlist
                .push(decode_native_runner_grant(field.bytes()?)?),
            6 => policy
                .realtime_scheduling_allowlist
                .push(decode_realtime_scheduling_grant(field.bytes()?)?),
            _ => {}
        }
    }
    Ok(policy)
}

fn decode_realtime_scheduling_grant(
    bytes: &[u8],
) -> Result<RealtimeSchedulingGrant, ManifestError> {
    let mut grant = RealtimeSchedulingGrant::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => grant.package_id = field.string()?,
            2 => grant.expected_signer = field.string()?,
            _ => {}
        }
    }
    Ok(grant)
}

fn decode_microvm_constraints(bytes: &[u8]) -> Result<MicrovmConstraints, ManifestError> {
    let mut constraints = MicrovmConstraints::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => constraints.max_vram_allocation_mb = field.varint()? as u32,
            2 => constraints.allow_network_bridge = field.varint()? != 0,
            3 => constraints.allow_pci_passthrough = field.varint()? != 0,
            _ => {}
        }
    }
    Ok(constraints)
}

fn decode_native_runner_grant(bytes: &[u8]) -> Result<NativeRunnerGrant, ManifestError> {
    let mut grant = NativeRunnerGrant::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => grant.package_id = field.string()?,
            2 => grant.expected_signer = field.string()?,
            _ => {}
        }
    }
    Ok(grant)
}

fn decode_tee_policy(bytes: &[u8]) -> Result<TeePolicy, ManifestError> {
    let mut policy = TeePolicy::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => policy.enforce_secure_boot = field.varint()? != 0,
            2 => policy.rpmb_anti_rollback = field.varint()? != 0,
            3 => policy.orchestrator_verification = field.string()?,
            4 => policy.allowed_trusted_apps.push(field.string()?),
            5 => policy.secure_storage_backend = SecureStorageBackend::from_proto(field.varint()?),
            6 => policy.require_secure_persistent_keys = field.varint()? != 0,
            7 => policy.allow_authenticated_firmware_selection = field.varint()? != 0,
            _ => {}
        }
    }
    Ok(policy)
}

fn decode_driver_policy(bytes: &[u8]) -> Result<DriverPolicy, ManifestError> {
    let mut policy = DriverPolicy::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => {
                policy.default_unmatched_action = DriverUnmatchedAction::from_proto(field.varint()?)
            }
            2 => policy
                .tier_1_allowlist
                .push(decode_driver_grant(field.bytes()?)?),
            3 => policy.tier_2_rules = decode_tier_2_driver_rules(field.bytes()?)?,
            _ => {}
        }
    }
    Ok(policy)
}

fn decode_driver_grant(bytes: &[u8]) -> Result<DriverGrant, ManifestError> {
    let mut grant = DriverGrant::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => grant.package_id = field.string()?,
            2 => grant.expected_signer = field.string()?,
            _ => {}
        }
    }
    Ok(grant)
}

fn decode_tier_2_driver_rules(bytes: &[u8]) -> Result<Tier2DriverRules, ManifestError> {
    let mut rules = Tier2DriverRules::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => rules.enforce_strict_iommu = field.varint()? != 0,
            2 => rules.allow_raw_mmio = field.varint()? != 0,
            3 => rules.max_dma_bandwidth_mbps = field.varint()? as u32,
            _ => {}
        }
    }
    Ok(rules)
}

fn decode_update_policy(bytes: &[u8]) -> Result<UpdatePolicy, ManifestError> {
    let mut policy = UpdatePolicy::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => policy.metadata_base_url = field.string()?,
            2 => policy.targets_base_url = field.string()?,
            3 => policy
                .trusted_root_keys
                .push(decode_update_root_key(field.bytes()?)?),
            4 => policy.trusted_root_metadata = field.bytes()?.to_vec(),
            5 => policy.app_target_prefix = field.string()?,
            6 => policy.kernel_target_name = field.string()?,
            7 => policy.tee_target_name = field.string()?,
            8 => policy.max_metadata_bytes = field.varint()?,
            9 => policy.max_target_bytes = field.varint()?,
            10 => policy.default_apply_policy = UpdateApplyPolicy::from_proto(field.varint()?),
            _ => {}
        }
    }
    Ok(policy)
}

fn decode_update_root_key(bytes: &[u8]) -> Result<UpdateRootKey, ManifestError> {
    let mut key = UpdateRootKey::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => key.key_id = field.string()?,
            2 => key.public_key = field.string()?,
            _ => {}
        }
    }
    Ok(key)
}

impl Architecture {
    const fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Aarch64,
            2 => Self::Riscv64,
            3 => Self::X86_64,
            _ => Self::Unspecified,
        }
    }
}

impl RunnerTier {
    const fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Tier1Wasm,
            2 => Self::Tier2Microvm,
            3 => Self::Tier0PlatformElf,
            _ => Self::Unspecified,
        }
    }
}

impl DriverUnmatchedAction {
    const fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::RejectAndIsolate,
            _ => Self::Unspecified,
        }
    }
}

impl UpdateApplyPolicy {
    const fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::CheckOnly,
            2 => Self::StageOnly,
            3 => Self::ApplyImmediately,
            _ => Self::Unspecified,
        }
    }
}

fn decode_interface_default(bytes: &[u8]) -> Result<InterfaceDefault, ManifestError> {
    let mut value = InterfaceDefault::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.interface_name = field.string()?,
            2 => value.package_name = field.string()?,
            3 => value.process_name = field.string()?,
            _ => (),
        }
    }
    Ok(value)
}
