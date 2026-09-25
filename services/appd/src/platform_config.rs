use crate::manifest::{Cursor, ManifestError};
use crate::runner::PackageTrustTier;
use alloc::string::String;
use alloc::vec;
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
    pub network_policy: NetworkPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkPolicy {
    pub isolation_groups: Vec<NetworkIsolationGroup>,
    pub domains: Vec<NetworkDomain>,
    pub virtual_ports: Vec<NetworkVirtualPort>,
    pub routes: Vec<NetworkRoute>,
    pub dns_upstreams: Vec<NetworkDnsUpstream>,
    pub resource_templates: Vec<NetworkResourceTemplate>,
    pub max_dynamic_providers: u32,
    pub routed_interfaces: Vec<NetworkRoutedInterface>,
    pub switch_routes: Vec<NetworkSwitchRoute>,
    pub firewall: NetworkFirewallPolicy,
    pub nat: NetworkNatPolicy,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            isolation_groups: Vec::new(),
            domains: vec![NetworkDomain {
                name: "system_default".into(),
                isolation_group: "system_default".into(),
                table_id: 0,
                system_default: true,
                authorized_packages: Vec::new(),
            }],
            virtual_ports: Vec::new(),
            routes: Vec::new(),
            dns_upstreams: Vec::new(),
            resource_templates: Vec::new(),
            max_dynamic_providers: 64,
            routed_interfaces: Vec::new(),
            switch_routes: Vec::new(),
            firewall: NetworkFirewallPolicy::default(),
            nat: NetworkNatPolicy::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkIsolationGroup {
    pub name: String,
    pub networkd_package: String,
    pub networkd_process: String,
    pub netstackd_package: String,
    pub netstackd_process: String,
    pub resource_template: String,
    pub table_ids: Vec<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkDomain {
    pub name: String,
    pub isolation_group: String,
    pub table_id: u32,
    pub system_default: bool,
    pub authorized_packages: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkResourceTemplate {
    pub name: String,
    pub max_instances: u32,
    pub cpu_weight: u32,
    pub memory_high_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkVirtualPort {
    pub port_id: u64,
    pub isolation_group: String,
    pub table_id: u32,
    pub physical_selector: String,
    pub source_mac: Vec<u8>,
    pub vlan_id: u16,
    pub tagged: bool,
    pub rx_queue_depth: u32,
    pub tx_queue_depth: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkIpPrefix {
    pub address: Vec<u8>,
    pub prefix_len: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkRoute {
    pub isolation_group: String,
    pub table_id: u32,
    pub destination: NetworkIpPrefix,
    pub gateway: Vec<u8>,
    pub interface_id: u64,
    pub metric: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkRoutedInterface {
    pub interface_id: u64,
    pub physical_interface: u64,
    pub virtual_port: u64,
    pub table_id: u32,
    pub bridge_domain: u32,
    pub vlan_id: u16,
    pub mac: Vec<u8>,
    pub mtu: u32,
    pub addresses: Vec<NetworkIpPrefix>,
    pub security_zone: u16,
    pub routing_enabled: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkSwitchRoute {
    pub table_id: u32,
    pub destination: NetworkIpPrefix,
    pub gateway: Vec<u8>,
    pub interface_id: u64,
    pub metric: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkExtensionArtifact {
    pub registry_host: String,
    pub repository: String,
    pub tag: String,
    pub expected_digest: Vec<u8>,
    pub media_type: String,
    pub abi: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkFirewallRule {
    pub source_zone: String,
    pub destination_zone: String,
    pub direction: String,
    pub source: NetworkIpPrefix,
    pub destination: NetworkIpPrefix,
    pub protocol: u8,
    pub source_port_start: u16,
    pub source_port_end: u16,
    pub destination_port_start: u16,
    pub destination_port_end: u16,
    pub icmp_type: Option<u32>,
    pub icmp_code: Option<u32>,
    pub connection_state: String,
    pub allow: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkFirewallPolicy {
    pub rules: Vec<NetworkFirewallRule>,
    pub desired_artifact: NetworkExtensionArtifact,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkNatPolicy {
    pub enabled: bool,
    pub external_ipv4_pool: Vec<Vec<u8>>,
    pub ephemeral_port_start: u16,
    pub ephemeral_port_end: u16,
    pub port_forwards: Vec<NetworkNatPortForward>,
    pub nptv6_internal: NetworkIpPrefix,
    pub nptv6_external: NetworkIpPrefix,
    pub desired_artifact: NetworkExtensionArtifact,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkNatPortForward {
    pub external_address: Vec<u8>,
    pub external_port: u16,
    pub internal_address: Vec<u8>,
    pub internal_port: u16,
    pub protocol: u8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NetworkDnsTransport {
    #[default]
    Unspecified,
    Udp53,
    Dot,
    Doh,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkDnsUpstream {
    pub isolation_group: String,
    pub table_id: u32,
    pub provider: String,
    pub domain_suffix: String,
    pub bootstrap_address: Vec<u8>,
    pub port: u16,
    pub transport: NetworkDnsTransport,
    pub tls_server_name: String,
    pub doh_path: String,
    pub priority: u32,
}

impl NetworkPolicy {
    pub fn domain(&self, name: &str) -> Option<&NetworkDomain> {
        self.domains.iter().find(|domain| domain.name == name)
    }

    pub fn authorize_domain(&self, package: &str, name: &str) -> Option<&NetworkDomain> {
        let domain = self.domain(name)?;
        if domain.authorized_packages.is_empty()
            || domain
                .authorized_packages
                .iter()
                .any(|value| value == package)
        {
            Some(domain)
        } else {
            None
        }
    }

    fn validate(&self) -> Result<(), ManifestError> {
        let default_count = self
            .domains
            .iter()
            .filter(|domain| domain.system_default)
            .count();
        if self.domains.is_empty()
            || default_count != 1
            || !self
                .domain("system_default")
                .is_some_and(|domain| domain.system_default)
        {
            return Err(ManifestError::InvalidConfigValue);
        }
        for (index, domain) in self.domains.iter().enumerate() {
            if !valid_identifier(&domain.name)
                || self.domains[..index]
                    .iter()
                    .any(|other| other.name == domain.name)
                || (!self.isolation_groups.is_empty()
                    && !self.isolation_groups.iter().any(|group| {
                        group.name == domain.isolation_group
                            && group.table_ids.contains(&domain.table_id)
                    }))
            {
                return Err(ManifestError::InvalidConfigValue);
            }
        }
        for (index, group) in self.isolation_groups.iter().enumerate() {
            if !valid_identifier(&group.name)
                || group.networkd_package.is_empty()
                || group.networkd_process.is_empty()
                || group.netstackd_package.is_empty()
                || group.netstackd_process.is_empty()
                || group.table_ids.is_empty()
                || group.table_ids.iter().any(|table| {
                    self.isolation_groups[..index]
                        .iter()
                        .any(|other| other.table_ids.contains(table))
                })
                || !self.resource_templates.iter().any(|template| {
                    template.name == group.resource_template
                        && template.max_instances != 0
                        && template.cpu_weight != 0
                        && template.memory_high_bytes != 0
                })
                || self.isolation_groups[..index]
                    .iter()
                    .any(|other| other.name == group.name)
            {
                return Err(ManifestError::InvalidConfigValue);
            }
        }
        for (index, template) in self.resource_templates.iter().enumerate() {
            let instance_count = self
                .isolation_groups
                .iter()
                .filter(|group| group.resource_template == template.name)
                .count();
            if !valid_identifier(&template.name)
                || template.max_instances == 0
                || template.cpu_weight == 0
                || template.memory_high_bytes == 0
                || instance_count > template.max_instances as usize
                || self.resource_templates[..index]
                    .iter()
                    .any(|other| other.name == template.name)
            {
                return Err(ManifestError::InvalidConfigValue);
            }
        }
        for port in &self.virtual_ports {
            if port.port_id == 0
                || !valid_selector(&port.physical_selector)
                || !matches!(port.source_mac.len(), 0 | 6)
                || port.vlan_id > 4094
                || port.rx_queue_depth == 0
                || port.tx_queue_depth == 0
                || port.rx_queue_depth > 4096
                || port.tx_queue_depth > 4096
                || !self.table_in_group(&port.isolation_group, port.table_id)
            {
                return Err(ManifestError::InvalidConfigValue);
            }
        }
        for route in &self.routes {
            if !valid_prefix(&route.destination)
                || (!route.gateway.is_empty() && !matches!(route.gateway.len(), 4 | 16))
                || !self.table_in_group(&route.isolation_group, route.table_id)
            {
                return Err(ManifestError::InvalidConfigValue);
            }
        }
        for upstream in &self.dns_upstreams {
            if !self.table_in_group(&upstream.isolation_group, upstream.table_id)
                || !valid_identifier(&upstream.provider)
                || !matches!(upstream.bootstrap_address.len(), 4 | 16)
                || upstream.port == 0
                || upstream.transport == NetworkDnsTransport::Unspecified
                || matches!(
                    upstream.transport,
                    NetworkDnsTransport::Dot | NetworkDnsTransport::Doh
                ) && upstream.tls_server_name.is_empty()
                || upstream.transport == NetworkDnsTransport::Doh
                    && !upstream.doh_path.starts_with('/')
            {
                return Err(ManifestError::InvalidConfigValue);
            }
        }
        Ok(())
    }

    fn table_in_group(&self, name: &str, table_id: u32) -> bool {
        self.isolation_groups.is_empty() && name == "system_default" && table_id == 0
            || self
                .isolation_groups
                .iter()
                .any(|group| group.name == name && group.table_ids.contains(&table_id))
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn valid_selector(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.is_ascii()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
}

fn valid_prefix(prefix: &NetworkIpPrefix) -> bool {
    matches!(prefix.address.len(), 4 | 16) && prefix.prefix_len as usize <= prefix.address.len() * 8
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
    pub allow_starnix_runner: bool,
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
            allow_starnix_runner: false,
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
        let config = decode_platform_config(bytes)?;
        config.network_policy.validate()?;
        Ok(config)
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
            "nix" if self.allow_starnix_runner => RunnerPolicyDecision::Allow,
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
            8 => config.network_policy = decode_network_policy(field.bytes()?)?,
            _ => {}
        }
    }
    Ok(config)
}

fn decode_network_policy(bytes: &[u8]) -> Result<NetworkPolicy, ManifestError> {
    let mut policy = NetworkPolicy {
        domains: Vec::new(),
        max_dynamic_providers: 64,
        ..NetworkPolicy::default()
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => policy
                .isolation_groups
                .push(decode_network_isolation_group(field.bytes()?)?),
            2 => policy.domains.push(decode_network_domain(field.bytes()?)?),
            3 => policy
                .virtual_ports
                .push(decode_network_virtual_port(field.bytes()?)?),
            4 => policy.routes.push(decode_network_route(field.bytes()?)?),
            5 => policy
                .dns_upstreams
                .push(decode_network_dns_upstream(field.bytes()?)?),
            6 => policy
                .resource_templates
                .push(decode_network_resource_template(field.bytes()?)?),
            7 => policy.max_dynamic_providers = field.varint()? as u32,
            8 => policy
                .routed_interfaces
                .push(decode_network_routed_interface(field.bytes()?)?),
            9 => policy
                .switch_routes
                .push(decode_network_switch_route(field.bytes()?)?),
            10 => policy.firewall = decode_network_firewall_policy(field.bytes()?)?,
            11 => policy.nat = decode_network_nat_policy(field.bytes()?)?,
            _ => {}
        }
    }
    if policy.max_dynamic_providers == 0 {
        policy.max_dynamic_providers = 64;
    }
    Ok(policy)
}

fn decode_network_routed_interface(bytes: &[u8]) -> Result<NetworkRoutedInterface, ManifestError> {
    let mut value = NetworkRoutedInterface::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.interface_id = field.varint()?,
            2 => value.physical_interface = field.varint()?,
            3 => value.virtual_port = field.varint()?,
            4 => value.table_id = field.varint()? as u32,
            5 => value.bridge_domain = field.varint()? as u32,
            6 => value.vlan_id = field.varint()? as u16,
            7 => value.mac = field.bytes()?.to_vec(),
            8 => value.mtu = field.varint()? as u32,
            9 => value
                .addresses
                .push(decode_network_ip_prefix(field.bytes()?)?),
            10 => value.security_zone = field.varint()? as u16,
            11 => value.routing_enabled = field.varint()? != 0,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_switch_route(bytes: &[u8]) -> Result<NetworkSwitchRoute, ManifestError> {
    let mut value = NetworkSwitchRoute::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.table_id = field.varint()? as u32,
            2 => value.destination = decode_network_ip_prefix(field.bytes()?)?,
            3 => value.gateway = field.bytes()?.to_vec(),
            4 => value.interface_id = field.varint()?,
            5 => value.metric = field.varint()? as u32,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_extension_artifact(
    bytes: &[u8],
) -> Result<NetworkExtensionArtifact, ManifestError> {
    let mut value = NetworkExtensionArtifact::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.registry_host = field.string()?,
            2 => value.repository = field.string()?,
            3 => value.tag = field.string()?,
            4 => value.expected_digest = field.bytes()?.to_vec(),
            5 => value.media_type = field.string()?,
            6 => value.abi = field.string()?,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_firewall_policy(bytes: &[u8]) -> Result<NetworkFirewallPolicy, ManifestError> {
    let mut value = NetworkFirewallPolicy::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value
                .rules
                .push(decode_network_firewall_rule(field.bytes()?)?),
            2 => value.desired_artifact = decode_network_extension_artifact(field.bytes()?)?,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_firewall_rule(bytes: &[u8]) -> Result<NetworkFirewallRule, ManifestError> {
    let mut value = NetworkFirewallRule::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.source_zone = field.string()?,
            2 => value.destination_zone = field.string()?,
            3 => value.direction = field.string()?,
            4 => value.source = decode_network_ip_prefix(field.bytes()?)?,
            5 => value.destination = decode_network_ip_prefix(field.bytes()?)?,
            6 => value.protocol = field.varint()? as u8,
            7 => value.source_port_start = field.varint()? as u16,
            8 => value.source_port_end = field.varint()? as u16,
            9 => value.destination_port_start = field.varint()? as u16,
            10 => value.destination_port_end = field.varint()? as u16,
            11 => value.icmp_type = Some(field.varint()? as u32),
            12 => value.icmp_code = Some(field.varint()? as u32),
            13 => value.connection_state = field.string()?,
            14 => value.allow = field.varint()? != 0,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_nat_policy(bytes: &[u8]) -> Result<NetworkNatPolicy, ManifestError> {
    let mut value = NetworkNatPolicy::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.enabled = field.varint()? != 0,
            2 => value.external_ipv4_pool.push(field.bytes()?.to_vec()),
            3 => value.ephemeral_port_start = field.varint()? as u16,
            4 => value.ephemeral_port_end = field.varint()? as u16,
            5 => value
                .port_forwards
                .push(decode_network_nat_port_forward(field.bytes()?)?),
            6 => value.nptv6_internal = decode_network_ip_prefix(field.bytes()?)?,
            7 => value.nptv6_external = decode_network_ip_prefix(field.bytes()?)?,
            8 => value.desired_artifact = decode_network_extension_artifact(field.bytes()?)?,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_nat_port_forward(bytes: &[u8]) -> Result<NetworkNatPortForward, ManifestError> {
    let mut value = NetworkNatPortForward::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.external_address = field.bytes()?.to_vec(),
            2 => value.external_port = field.varint()? as u16,
            3 => value.internal_address = field.bytes()?.to_vec(),
            4 => value.internal_port = field.varint()? as u16,
            5 => value.protocol = field.varint()? as u8,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_isolation_group(bytes: &[u8]) -> Result<NetworkIsolationGroup, ManifestError> {
    let mut value = NetworkIsolationGroup::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.name = field.string()?,
            2 => value.networkd_package = field.string()?,
            3 => value.networkd_process = field.string()?,
            4 => value.netstackd_package = field.string()?,
            5 => value.netstackd_process = field.string()?,
            6 => value.resource_template = field.string()?,
            7 => value.table_ids.push(field.varint()? as u32),
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_domain(bytes: &[u8]) -> Result<NetworkDomain, ManifestError> {
    let mut value = NetworkDomain::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.name = field.string()?,
            2 => value.isolation_group = field.string()?,
            3 => value.table_id = field.varint()? as u32,
            4 => value.system_default = field.varint()? != 0,
            5 => value.authorized_packages.push(field.string()?),
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_resource_template(
    bytes: &[u8],
) -> Result<NetworkResourceTemplate, ManifestError> {
    let mut value = NetworkResourceTemplate::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.name = field.string()?,
            2 => value.max_instances = field.varint()? as u32,
            3 => value.cpu_weight = field.varint()? as u32,
            4 => value.memory_high_bytes = field.varint()?,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_virtual_port(bytes: &[u8]) -> Result<NetworkVirtualPort, ManifestError> {
    let mut value = NetworkVirtualPort::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.port_id = field.varint()?,
            2 => value.isolation_group = field.string()?,
            3 => value.table_id = field.varint()? as u32,
            4 => value.physical_selector = field.string()?,
            5 => value.source_mac = field.bytes()?.to_vec(),
            6 => value.vlan_id = field.varint()? as u16,
            7 => value.tagged = field.varint()? != 0,
            8 => value.rx_queue_depth = field.varint()? as u32,
            9 => value.tx_queue_depth = field.varint()? as u32,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_ip_prefix(bytes: &[u8]) -> Result<NetworkIpPrefix, ManifestError> {
    let mut value = NetworkIpPrefix::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.address = field.bytes()?.to_vec(),
            2 => value.prefix_len = field.varint()? as u8,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_route(bytes: &[u8]) -> Result<NetworkRoute, ManifestError> {
    let mut value = NetworkRoute::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.isolation_group = field.string()?,
            2 => value.table_id = field.varint()? as u32,
            3 => value.destination = decode_network_ip_prefix(field.bytes()?)?,
            4 => value.gateway = field.bytes()?.to_vec(),
            5 => value.interface_id = field.varint()?,
            6 => value.metric = field.varint()? as u32,
            _ => {}
        }
    }
    Ok(value)
}

fn decode_network_dns_upstream(bytes: &[u8]) -> Result<NetworkDnsUpstream, ManifestError> {
    let mut value = NetworkDnsUpstream::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.isolation_group = field.string()?,
            2 => value.table_id = field.varint()? as u32,
            3 => value.provider = field.string()?,
            4 => value.domain_suffix = field.string()?,
            5 => value.bootstrap_address = field.bytes()?.to_vec(),
            6 => value.port = field.varint()? as u16,
            7 => {
                value.transport = match field.varint()? {
                    1 => NetworkDnsTransport::Udp53,
                    2 => NetworkDnsTransport::Dot,
                    3 => NetworkDnsTransport::Doh,
                    _ => NetworkDnsTransport::Unspecified,
                }
            }
            8 => value.tls_server_name = field.string()?,
            9 => value.doh_path = field.string()?,
            10 => value.priority = field.varint()? as u32,
            _ => {}
        }
    }
    Ok(value)
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
            7 => policy.allow_starnix_runner = field.varint()? != 0,
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
