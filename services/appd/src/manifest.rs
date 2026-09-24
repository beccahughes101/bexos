use alloc::string::String;
use alloc::vec::Vec;
pub use bexos_app_manifest::Architecture;
use bexos_package_version::{MultiVersionPolicy, SemVer};
pub use bexos_permission_store::{PermissionDeclaration, PermissionRequirement};
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Manifest {
    pub architecture: Architecture,
    pub package_name: String,
    pub name: String,
    pub processes: Vec<Process>,
    pub permissions: Vec<PermissionDeclaration>,
    pub services_exposed: Vec<ExposedService>,
    pub services_consumed: Vec<ConsumedService>,
    pub resource_groups: Vec<ResourceGroup>,
    pub driver_info: Option<DriverInfo>,
    pub bind_rules: Vec<BindRule>,
    pub config_schema: ComponentConfigSchema,
    pub package_kind: PackageKind,
    pub library_dependencies: Vec<LibraryDependency>,
    pub package_version: SemVer,
    pub multi_version_policy: MultiVersionPolicy,
    pub min_bexos_abi_version: u32,
    pub library_exports: Vec<LibraryExport>,
    pub jobs: Vec<JobDefinition>,
    pub shared_vaults: Vec<SharedVault>,
    pub trusted_app: Option<TrustedAppInfo>,
    pub commands: Vec<BinaryCommand>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryCommand {
    pub command_name: String,
    pub process_name: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PackageKind {
    #[default]
    Application,
    Library,
    TrustedApp,
    Unspecified,
}

impl PackageKind {
    pub fn from_proto(value: u64) -> Self {
        match value {
            0 => Self::Application,
            1 => Self::Library,
            2 => Self::TrustedApp,
            _ => Self::Unspecified,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrustedAppInfo {
    pub provider: String,
    pub uuid: [u8; 16],
    pub secure_version: u64,
    pub archive_payload_path: String,
    pub allowed_service_ports: Vec<String>,
    pub protected: bool,
    pub uninstallable: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryDependency {
    pub package_name: String,
    pub version_requirement: Option<String>,
    pub mount_alias: Option<String>,
    pub abi_version: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryExport {
    pub name: String,
    pub path: String,
    pub symbol_prefix: String,
    pub abi_version: u32,
    pub soname: String,
    pub kind: LibraryExportKind,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LibraryExportKind {
    #[default]
    Native,
    WasmComponent,
    Unspecified,
}

impl LibraryExportKind {
    fn from_proto(value: u64) -> Self {
        match value {
            0 => Self::Native,
            1 => Self::WasmComponent,
            _ => Self::Unspecified,
        }
    }
}

pub use bexos_component_config::schema::{
    Field as ComponentConfigField, Schema as ComponentConfigSchema, Type as ComponentConfigType,
    Value as ComponentConfigValue,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Process {
    pub name: String,
    pub runner: String,
    pub permissions: Vec<PermissionDeclaration>,
    pub service: bool,
    pub depends_on: Vec<String>,
    pub link: Option<Link>,
    pub runner_options: Option<ProcessRunnerOptions>,
    pub wave: Option<u32>,
    pub lifecycle: ProcessLifecycle,
    pub resource_group: Option<String>,
    pub handles: Vec<IntentFilter>,
    pub shell_role: ShellRole,
    pub network_domain: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShellRole {
    #[default]
    None,
    System,
    User,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IntentFilter {
    pub schemes: Vec<String>,
    pub domains: Vec<String>,
    pub mime_types: Vec<String>,
    pub provides_interfaces: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JobNetworkConstraint {
    #[default]
    Any,
    UnmeteredOnly,
    None,
    Unspecified,
}

impl JobNetworkConstraint {
    fn from_proto(value: u64) -> Self {
        match value {
            0 => Self::Any,
            1 => Self::UnmeteredOnly,
            2 => Self::None,
            _ => Self::Unspecified,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JobDefinition {
    pub job_id: String,
    pub target_component: String,
    pub initial_delay_seconds: u64,
    pub interval_seconds: u64,
    pub flex_window_seconds: u32,
    pub network: JobNetworkConstraint,
    pub requires_charging: bool,
    pub requires_device_idle: bool,
    pub requires_battery_not_low: bool,
    pub persist_across_reboots: bool,
    pub max_execution_seconds: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UpdateStrategy {
    #[default]
    Restart,
    HeartTransplant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessLifecycle {
    pub update_strategy: UpdateStrategy,
    pub migration_timeout_ms: u32,
    pub preparation_timeout_ms: u32,
}
impl Default for ProcessLifecycle {
    fn default() -> Self {
        Self {
            update_strategy: UpdateStrategy::Restart,
            migration_timeout_ms: 150,
            preparation_timeout_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Link {
    pub icon: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroup {
    pub name: String,
    pub cpu_shares: u32,
    pub memory_limit_pages: u64,
    pub parent: Option<String>,
    pub cpu: ResourceGroupCpuLimits,
    pub memory: ResourceGroupMemoryLimits,
    pub gpu: ResourceGroupGpuLimits,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupCpuLimits {
    pub weight: u32,
    pub max_utilization_permille: u32,
    pub allow_realtime: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupMemoryLimits {
    pub low_watermark_bytes: u64,
    pub high_watermark_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupGpuLimits {
    pub max_render_budget_percent: u32,
    pub max_vram_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SharedVaultAccess {
    #[default]
    ReadOnly,
    ReadWrite,
    Unspecified,
}

impl SharedVaultAccess {
    fn from_proto(value: u64) -> Self {
        match value {
            0 => Self::ReadOnly,
            1 => Self::ReadWrite,
            _ => Self::Unspecified,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SharedVault {
    pub name: String,
    pub access: SharedVaultAccess,
    pub allowed_peer_package_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverInfo {
    pub name: String,
    pub package_id: String,
    pub version: String,
    pub execution: DriverExecution,
    pub required_resources: Vec<RequiredHardwareResource>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DriverColocationPolicy {
    #[default]
    Isolated,
    Colocated,
    HostShared,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DriverRestartStrategy {
    #[default]
    HeartTransplant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverExecution {
    pub colocation_policy: DriverColocationPolicy,
    pub max_instances_per_host: u32,
    pub restart_strategy: DriverRestartStrategy,
}

impl Default for DriverExecution {
    fn default() -> Self {
        Self {
            colocation_policy: DriverColocationPolicy::Isolated,
            max_instances_per_host: 1,
            restart_strategy: DriverRestartStrategy::HeartTransplant,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DriverHardwareResourceKind {
    #[default]
    Unspecified,
    Mmio,
    Interrupt,
    DmaPool,
    IommuDomain,
    RegisterProxy,
    BusControl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiredHardwareResource {
    pub kind: DriverHardwareResourceKind,
    pub min_count: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BindRule {
    pub conditions: Vec<BindCondition>,
    pub priority: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BindCondition {
    pub bus: BindBusType,
    pub properties: Vec<BindProperty>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BindBusType {
    #[default]
    Unspecified,
    Pci,
    Usb,
    PlatformDt,
    I2c,
    Spi,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BindProperty {
    pub key: String,
    pub value: u32,
}

pub const ELF_RUNNER_OPTIONS_TYPE_URL: &str = "type.googleapis.com/bexos.app.ELFRunnerOptions";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessRunnerOptions {
    Elf(ElfRunnerOptions),
    Wasm(bexos_wasm_abi::WasmRunnerOptions),
    Nix(bexos_starnix_abi::NixRunnerOptions),
    Unknown(AnyRunnerOptions),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ElfRunnerOptions {
    pub path: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnyRunnerOptions {
    pub type_url: String,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExposedService {
    pub name: String,
    pub protocol: String,
    pub lifecycle: Lifecycle,
    pub visibility: Visibility,
    pub bind_permission: Option<String>,
    pub metadata: Vec<Metadata>,
    pub capabilities: Vec<CapabilityMetadata>,
    pub activation: ServiceActivation,
    pub idle_timeout_ms: Option<u32>,
    pub provider_process: Option<String>,
}

impl Default for ExposedService {
    fn default() -> Self {
        Self {
            name: String::new(),
            protocol: String::new(),
            lifecycle: Lifecycle::Unspecified,
            visibility: Visibility::Unspecified,
            bind_permission: None,
            metadata: Vec::new(),
            capabilities: Vec::new(),
            activation: ServiceActivation::Eager,
            idle_timeout_ms: None,
            provider_process: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilityMetadata {
    pub capability: String,
    pub permission: Option<String>,
    pub method_ordinals: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsumedService {
    pub name: String,
    pub link_type: LinkType,
    pub filter: Option<String>,
    pub capabilities: Vec<ConsumedCapability>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConsumedCapability {
    pub capability: String,
    pub methods: Vec<MethodDependency>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MethodDependency {
    pub ordinal: u64,
    pub link_type: LinkType,
}

impl Default for MethodDependency {
    fn default() -> Self {
        Self {
            ordinal: 0,
            link_type: LinkType::Unspecified,
        }
    }
}

impl Default for ConsumedService {
    fn default() -> Self {
        Self {
            name: String::new(),
            link_type: LinkType::Unspecified,
            filter: None,
            capabilities: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    Unspecified,
    Singleton,
    UserScopedSingleton,
    MultipleInstance,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ServiceActivation {
    #[default]
    Eager,
    Lazy,
    Unspecified,
}

pub const DEFAULT_LAZY_IDLE_TIMEOUT_MS: u32 = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    Unspecified,
    Public,
    DomainShared,
    Private,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkType {
    Unspecified,
    Required,
    Optional,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Metadata {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestError {
    InvalidArchitecture,
    UnexpectedEof,
    InvalidVarint,
    InvalidWireType(u8),
    InvalidUtf8,
    LengthOverflow,
    InvalidLifecycle,
    InvalidConfigValue,
    InvalidPermissionRequirement,
    InvalidTrustedApp,
    InvalidWasmOptions,
    InvalidRunnerOptions,
    InvalidCommand,
    InvalidLazyService,
    InvalidDriver,
}

impl Manifest {
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        decode_manifest(bytes)
    }

    pub fn validate_package_shape(&self) -> Result<(), ManifestError> {
        self.validate_commands()?;
        self.validate_lazy_services()?;
        self.validate_driver()?;
        match self.package_kind {
            PackageKind::Application => Ok(()),
            PackageKind::Library => {
                if self.processes.is_empty() && self.config_schema.fields.is_empty() {
                    Ok(())
                } else {
                    Err(ManifestError::InvalidTrustedApp)
                }
            }
            PackageKind::TrustedApp => self.validate_trusted_app(),
            PackageKind::Unspecified => Err(ManifestError::InvalidTrustedApp),
        }
    }

    fn validate_driver(&self) -> Result<(), ManifestError> {
        let is_driver = self.driver_info.is_some() || !self.bind_rules.is_empty();
        if !is_driver {
            return Ok(());
        }
        let info = self
            .driver_info
            .as_ref()
            .ok_or(ManifestError::InvalidDriver)?;
        if info.name.is_empty()
            || self.bind_rules.is_empty()
            || self.processes.len() != 1
            || self.processes[0].lifecycle.update_strategy != UpdateStrategy::HeartTransplant
            || !(1..=64).contains(&info.execution.max_instances_per_host)
            || info.required_resources.len() > 16
        {
            return Err(ManifestError::InvalidDriver);
        }
        if info.execution.colocation_policy == DriverColocationPolicy::Isolated
            && info.execution.max_instances_per_host != 1
        {
            return Err(ManifestError::InvalidDriver);
        }
        for (index, required) in info.required_resources.iter().enumerate() {
            if required.kind == DriverHardwareResourceKind::Unspecified
                || required.min_count == 0
                || required.min_count > 16
                || info.required_resources[..index]
                    .iter()
                    .any(|prior| prior.kind == required.kind)
            {
                return Err(ManifestError::InvalidDriver);
            }
        }
        for rule in &self.bind_rules {
            if rule.conditions.is_empty()
                || rule.conditions.iter().any(|condition| {
                    condition.bus == BindBusType::Unspecified
                        || condition.properties.is_empty()
                        || condition
                            .properties
                            .iter()
                            .any(|property| property.key.is_empty())
                })
            {
                return Err(ManifestError::InvalidDriver);
            }
        }
        // Hardware drivers may consume control-plane services, but never an
        // ambient socket provider. Package acquisition is performed by appd.
        if self.services_consumed.iter().any(|service| {
            matches!(
                service.name.as_str(),
                "bexos.net.Netstack" | "bexos.net.SocketProvider"
            )
        }) {
            return Err(ManifestError::InvalidDriver);
        }
        Ok(())
    }

    pub fn service_provider_process<'a>(
        &'a self,
        service: &'a ExposedService,
    ) -> Result<&'a Process, ManifestError> {
        let process_name = if let Some(process_name) = service.provider_process.as_deref() {
            process_name
        } else {
            let mut service_processes = self.processes.iter().filter(|process| process.service);
            let Some(process) = service_processes.next() else {
                return Err(ManifestError::InvalidLazyService);
            };
            if service_processes.next().is_some() {
                return Err(ManifestError::InvalidLazyService);
            }
            return Ok(process);
        };
        self.processes
            .iter()
            .find(|process| process.name == process_name && process.service)
            .ok_or(ManifestError::InvalidLazyService)
    }

    pub fn service_idle_timeout_ms(&self, service: &ExposedService) -> Result<u32, ManifestError> {
        match service.activation {
            ServiceActivation::Eager => Ok(service.idle_timeout_ms.unwrap_or(0)),
            ServiceActivation::Lazy => Ok(service
                .idle_timeout_ms
                .unwrap_or(DEFAULT_LAZY_IDLE_TIMEOUT_MS)),
            ServiceActivation::Unspecified => Err(ManifestError::InvalidLazyService),
        }
    }

    pub fn process_has_lazy_exposures(&self, process_name: &str) -> bool {
        self.services_exposed.iter().any(|service| {
            service.activation == ServiceActivation::Lazy
                && self
                    .service_provider_process(service)
                    .is_ok_and(|process| process.name == process_name)
        })
    }

    fn validate_lazy_services(&self) -> Result<(), ManifestError> {
        for service in &self.services_exposed {
            if service.activation == ServiceActivation::Unspecified {
                return Err(ManifestError::InvalidLazyService);
            }
            if service.activation == ServiceActivation::Eager {
                if let Some(process_name) = service.provider_process.as_deref() {
                    if !self
                        .processes
                        .iter()
                        .any(|process| process.name == process_name && process.service)
                    {
                        return Err(ManifestError::InvalidLazyService);
                    }
                }
                continue;
            }
            if !matches!(
                service.lifecycle,
                Lifecycle::Singleton | Lifecycle::UserScopedSingleton
            ) {
                return Err(ManifestError::InvalidLazyService);
            }
            if !self.bind_rules.is_empty() || self.driver_info.is_some() {
                return Err(ManifestError::InvalidLazyService);
            }
            let provider = self.service_provider_process(service)?;
            if provider.lifecycle.update_strategy != UpdateStrategy::HeartTransplant {
                return Err(ManifestError::InvalidLazyService);
            }
            if provider.resource_group.as_deref() == Some("device") {
                return Err(ManifestError::InvalidLazyService);
            }
        }
        for (index, service) in self.services_exposed.iter().enumerate() {
            let Ok(provider) = self.service_provider_process(service) else {
                continue;
            };
            for other in self.services_exposed.iter().skip(index + 1) {
                let Ok(other_provider) = self.service_provider_process(other) else {
                    continue;
                };
                if provider.name == other_provider.name
                    && (service.activation != other.activation
                        || self.service_idle_timeout_ms(service)?
                            != self.service_idle_timeout_ms(other)?)
                {
                    return Err(ManifestError::InvalidLazyService);
                }
            }
        }
        Ok(())
    }

    pub fn validate_commands(&self) -> Result<(), ManifestError> {
        for (i, command) in self.commands.iter().enumerate() {
            if command.command_name.is_empty()
                || command.command_name.len() > 64
                || !command
                    .command_name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
                || command.command_name == "."
                || command.command_name == ".."
                || self.commands[..i]
                    .iter()
                    .any(|c| c.command_name == command.command_name)
                || !self.processes.iter().any(|p| {
                    p.name == command.process_name
                        && !p.service
                        && crate::commands::executable(p).is_some()
                })
                || self.package_kind != PackageKind::Application
            {
                return Err(ManifestError::InvalidCommand);
            }
        }
        Ok(())
    }

    fn validate_trusted_app(&self) -> Result<(), ManifestError> {
        let Some(info) = self.trusted_app.as_ref() else {
            return Err(ManifestError::InvalidTrustedApp);
        };
        if !self.processes.is_empty()
            || !self.library_exports.is_empty()
            || !self.services_exposed.is_empty()
            || info.provider != "trusty"
            || info.secure_version == 0
            || info.archive_payload_path.is_empty()
            || !info.archive_payload_path.starts_with("/pkg/")
            || info.archive_payload_path.contains("..")
            || info.allowed_service_ports.is_empty()
            || info
                .allowed_service_ports
                .iter()
                .any(|port| port.is_empty())
        {
            return Err(ManifestError::InvalidTrustedApp);
        }
        Ok(())
    }
}

fn decode_manifest(bytes: &[u8]) -> Result<Manifest, ManifestError> {
    let mut manifest = Manifest::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => manifest.package_name = field.string()?,
            2 => manifest.name = field.string()?,
            3 => manifest.processes.push(decode_process(field.bytes()?)?),
            4 => manifest
                .permissions
                .push(decode_permission_declaration(field.bytes()?)?),
            5 => manifest
                .services_exposed
                .push(decode_exposed_service(field.bytes()?)?),
            6 => manifest
                .services_consumed
                .push(decode_consumed_service(field.bytes()?)?),
            7 => manifest
                .resource_groups
                .push(decode_resource_group(field.bytes()?)?),
            8 => manifest.driver_info = Some(decode_driver_info(field.bytes()?)?),
            9 => manifest.bind_rules.push(decode_bind_rule(field.bytes()?)?),
            10 => manifest.config_schema = decode_component_config_schema(field.bytes()?)?,
            11 => manifest.package_kind = PackageKind::from_proto(field.varint()?),
            12 => manifest
                .library_dependencies
                .push(decode_library_dependency(field.bytes()?)?),
            13 => manifest.package_version = decode_semver(field.bytes()?)?,
            14 => manifest.multi_version_policy = MultiVersionPolicy::from_proto(field.varint()?),
            15 => manifest.min_bexos_abi_version = field.varint()? as u32,
            16 => manifest
                .library_exports
                .push(decode_library_export(field.bytes()?)?),
            17 => manifest.jobs.push(decode_job_definition(field.bytes()?)?),
            18 => manifest
                .shared_vaults
                .push(decode_shared_vault(field.bytes()?)?),
            19 => manifest.trusted_app = Some(decode_trusted_app_info(field.bytes()?)?),
            21 => manifest
                .commands
                .push(decode_binary_command(field.bytes()?)?),
            20 => {
                manifest.architecture = Architecture::from_proto(field.varint()?)
                    .map_err(|_| ManifestError::InvalidArchitecture)?
            }
            _ => {}
        }
    }

    manifest.validate_commands()?;
    Ok(manifest)
}

fn decode_binary_command(bytes: &[u8]) -> Result<BinaryCommand, ManifestError> {
    let mut value = BinaryCommand::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => value.command_name = field.string()?,
            2 => value.process_name = field.string()?,
            _ => (),
        }
    }
    Ok(value)
}

fn decode_trusted_app_info(bytes: &[u8]) -> Result<TrustedAppInfo, ManifestError> {
    let mut info = TrustedAppInfo::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => info.provider = field.string()?,
            2 => {
                let bytes = field.bytes()?;
                if bytes.len() != 16 {
                    return Err(ManifestError::InvalidTrustedApp);
                }
                info.uuid.copy_from_slice(bytes);
            }
            3 => info.secure_version = field.varint()?,
            4 => info.archive_payload_path = field.string()?,
            5 => info.allowed_service_ports.push(field.string()?),
            6 => info.protected = field.varint()? != 0,
            7 => info.uninstallable = field.varint()? != 0,
            _ => {}
        }
    }

    Ok(info)
}

fn decode_shared_vault(bytes: &[u8]) -> Result<SharedVault, ManifestError> {
    let mut vault = SharedVault::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => vault.name = field.string()?,
            2 => vault.access = SharedVaultAccess::from_proto(field.varint()?),
            3 => vault.allowed_peer_package_prefixes.push(field.string()?),
            _ => {}
        }
    }

    Ok(vault)
}

fn decode_job_definition(bytes: &[u8]) -> Result<JobDefinition, ManifestError> {
    let mut job = JobDefinition::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => job.job_id = field.string()?,
            2 => job.target_component = field.string()?,
            3 => job.initial_delay_seconds = field.varint()?,
            4 => job.interval_seconds = field.varint()?,
            5 => job.flex_window_seconds = field.varint()? as u32,
            6 => job.network = JobNetworkConstraint::from_proto(field.varint()?),
            7 => job.requires_charging = field.varint()? != 0,
            8 => job.requires_device_idle = field.varint()? != 0,
            9 => job.requires_battery_not_low = field.varint()? != 0,
            10 => job.persist_across_reboots = field.varint()? != 0,
            11 => job.max_execution_seconds = field.varint()? as u32,
            _ => {}
        }
    }

    Ok(job)
}

fn decode_semver(bytes: &[u8]) -> Result<SemVer, ManifestError> {
    let mut version = SemVer::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => version.major = field.varint()? as u32,
            2 => version.minor = field.varint()? as u32,
            3 => version.patch = field.varint()? as u32,
            4 => version.build = field.varint()? as u32,
            5 => version.prerelease = field.string()?,
            _ => {}
        }
    }

    Ok(version)
}

fn decode_library_dependency(bytes: &[u8]) -> Result<LibraryDependency, ManifestError> {
    let mut dependency = LibraryDependency::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => dependency.package_name = field.string()?,
            2 => {
                let value = field.string()?;
                if !value.is_empty() {
                    dependency.version_requirement = Some(value);
                }
            }
            3 => {
                let value = field.string()?;
                if !value.is_empty() {
                    dependency.mount_alias = Some(value);
                }
            }
            4 => dependency.abi_version = field.varint()? as u32,
            _ => {}
        }
    }

    Ok(dependency)
}

fn decode_library_export(bytes: &[u8]) -> Result<LibraryExport, ManifestError> {
    let mut export = LibraryExport::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => export.name = field.string()?,
            2 => export.path = field.string()?,
            3 => export.symbol_prefix = field.string()?,
            4 => export.abi_version = field.varint()? as u32,
            5 => export.soname = field.string()?,
            6 => export.kind = LibraryExportKind::from_proto(field.varint()?),
            _ => {}
        }
    }

    Ok(export)
}

fn decode_component_config_schema(bytes: &[u8]) -> Result<ComponentConfigSchema, ManifestError> {
    let schema =
        ComponentConfigSchema::decode(bytes).map_err(|_| ManifestError::InvalidConfigValue)?;
    schema
        .validate()
        .map_err(|_| ManifestError::InvalidConfigValue)?;
    Ok(schema)
}

fn decode_process(bytes: &[u8]) -> Result<Process, ManifestError> {
    let mut process = Process::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => process.name = field.string()?,
            2 => process.runner = field.string()?,
            3 => process
                .permissions
                .push(decode_permission_declaration(field.bytes()?)?),
            4 => process.service = field.varint()? != 0,
            5 => process.depends_on.push(field.string()?),
            6 => process.link = Some(decode_link(field.bytes()?)?),
            7 => process.runner_options = Some(decode_runner_options(field.bytes()?)?),
            8 => process.wave = Some(field.varint()? as u32),
            9 => process.lifecycle = decode_lifecycle(field.bytes()?)?,
            10 => {
                let resource_group = field.string()?;
                if !resource_group.is_empty() {
                    process.resource_group = Some(resource_group);
                }
            }
            11 => process.handles.push(decode_intent_filter(field.bytes()?)?),
            12 => {
                process.shell_role = match field.varint()? {
                    0 => ShellRole::None,
                    1 => ShellRole::System,
                    2 => ShellRole::User,
                    _ => return Err(ManifestError::InvalidConfigValue),
                }
            }
            13 => {
                let domain = field.string()?;
                if !domain.is_empty() {
                    if !valid_network_domain(&domain) {
                        return Err(ManifestError::InvalidConfigValue);
                    }
                    process.network_domain = Some(domain);
                }
            }
            _ => {}
        }
    }

    Ok(process)
}

fn valid_network_domain(value: &str) -> bool {
    value.len() <= 63
        && value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'0'..=b'9' | b'_' => true,
            b'-' => index != 0 && index + 1 != value.len(),
            _ => false,
        })
}

fn decode_intent_filter(bytes: &[u8]) -> Result<IntentFilter, ManifestError> {
    let mut filter = IntentFilter::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => filter.schemes.push(field.string()?),
            2 => filter.domains.push(field.string()?),
            3 => filter.mime_types.push(field.string()?),
            4 => filter.provides_interfaces.push(field.string()?),
            _ => {}
        }
    }

    Ok(filter)
}

fn decode_permission_declaration(bytes: &[u8]) -> Result<PermissionDeclaration, ManifestError> {
    let mut declaration = PermissionDeclaration::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => declaration.name = field.string()?,
            2 => declaration.values.push(field.string()?),
            3 => {
                declaration.requirement = match field.varint()? {
                    0 => PermissionRequirement::Required,
                    1 => PermissionRequirement::Optional,
                    _ => return Err(ManifestError::InvalidPermissionRequirement),
                }
            }
            4 => declaration.usage_description = field.string()?,
            _ => {}
        }
    }

    Ok(declaration)
}

fn decode_resource_group(bytes: &[u8]) -> Result<ResourceGroup, ManifestError> {
    let mut group = ResourceGroup::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => group.name = field.string()?,
            2 => group.cpu_shares = field.varint()? as u32,
            3 => group.memory_limit_pages = field.varint()?,
            4 => group.parent = Some(field.string()?),
            5 => group.cpu = decode_resource_group_cpu_limits(field.bytes()?)?,
            6 => group.memory = decode_resource_group_memory_limits(field.bytes()?)?,
            7 => group.gpu = decode_resource_group_gpu_limits(field.bytes()?)?,
            _ => {}
        }
    }

    Ok(group)
}

fn decode_resource_group_cpu_limits(bytes: &[u8]) -> Result<ResourceGroupCpuLimits, ManifestError> {
    let mut limits = ResourceGroupCpuLimits::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => limits.weight = field.varint()? as u32,
            2 => limits.max_utilization_permille = field.varint()? as u32,
            3 => limits.allow_realtime = field.varint()? != 0,
            _ => {}
        }
    }
    Ok(limits)
}

fn decode_resource_group_memory_limits(
    bytes: &[u8],
) -> Result<ResourceGroupMemoryLimits, ManifestError> {
    let mut limits = ResourceGroupMemoryLimits::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => limits.low_watermark_bytes = field.varint()?,
            2 => limits.high_watermark_bytes = field.varint()?,
            _ => {}
        }
    }
    Ok(limits)
}

fn decode_resource_group_gpu_limits(bytes: &[u8]) -> Result<ResourceGroupGpuLimits, ManifestError> {
    let mut limits = ResourceGroupGpuLimits::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => limits.max_render_budget_percent = field.varint()? as u32,
            2 => limits.max_vram_bytes = field.varint()?,
            _ => {}
        }
    }
    Ok(limits)
}

fn decode_driver_info(bytes: &[u8]) -> Result<DriverInfo, ManifestError> {
    let mut info = DriverInfo::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => info.name = field.string()?,
            2 => info.package_id = field.string()?,
            3 => info.version = field.string()?,
            4 => info.execution = decode_driver_execution(field.bytes()?)?,
            5 => info
                .required_resources
                .push(decode_required_hardware_resource(field.bytes()?)?),
            _ => {}
        }
    }

    Ok(info)
}

fn decode_driver_execution(bytes: &[u8]) -> Result<DriverExecution, ManifestError> {
    let mut execution = DriverExecution::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => execution.colocation_policy = DriverColocationPolicy::from_proto(field.varint()?),
            2 => {
                let count = field.varint()? as u32;
                execution.max_instances_per_host = count.max(1);
            }
            3 => execution.restart_strategy = DriverRestartStrategy::from_proto(field.varint()?)?,
            _ => {}
        }
    }
    Ok(execution)
}

fn decode_required_hardware_resource(
    bytes: &[u8],
) -> Result<RequiredHardwareResource, ManifestError> {
    let mut required = RequiredHardwareResource {
        kind: DriverHardwareResourceKind::Unspecified,
        min_count: 1,
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => required.kind = DriverHardwareResourceKind::from_proto(field.varint()?),
            2 => required.min_count = (field.varint()? as u32).max(1),
            _ => {}
        }
    }
    Ok(required)
}

fn decode_bind_rule(bytes: &[u8]) -> Result<BindRule, ManifestError> {
    let mut rule = BindRule::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => rule.conditions.push(decode_bind_condition(field.bytes()?)?),
            2 => rule.priority = field.varint()? as u32,
            _ => {}
        }
    }

    Ok(rule)
}

fn decode_bind_condition(bytes: &[u8]) -> Result<BindCondition, ManifestError> {
    let mut condition = BindCondition::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => condition.bus = BindBusType::from_proto(field.varint()?),
            2 => condition
                .properties
                .push(decode_bind_property(field.bytes()?)?),
            _ => {}
        }
    }

    Ok(condition)
}

fn decode_bind_property(bytes: &[u8]) -> Result<BindProperty, ManifestError> {
    let mut property = BindProperty::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => property.key = field.string()?,
            2 => property.value = field.varint()? as u32,
            _ => {}
        }
    }

    Ok(property)
}

fn decode_lifecycle(bytes: &[u8]) -> Result<ProcessLifecycle, ManifestError> {
    let mut lifecycle = ProcessLifecycle::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => {
                lifecycle.update_strategy = match field.varint()? {
                    0 => UpdateStrategy::Restart,
                    1 => UpdateStrategy::HeartTransplant,
                    _ => return Err(ManifestError::InvalidLifecycle),
                }
            }
            2 | 3 => {
                let value =
                    u32::try_from(field.varint()?).map_err(|_| ManifestError::InvalidLifecycle)?;
                if value == 0 {
                    return Err(ManifestError::InvalidLifecycle);
                }
                if field.number == 2 {
                    lifecycle.migration_timeout_ms = value;
                } else {
                    lifecycle.preparation_timeout_ms = value;
                }
            }
            _ => {}
        }
    }
    Ok(lifecycle)
}

fn decode_link(bytes: &[u8]) -> Result<Link, ManifestError> {
    let mut link = Link::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => link.icon = field.string()?,
            2 => link.name = field.string()?,
            _ => {}
        }
    }

    Ok(link)
}

fn decode_runner_options(bytes: &[u8]) -> Result<ProcessRunnerOptions, ManifestError> {
    let any = decode_any_runner_options(bytes)?;
    if any.type_url == ELF_RUNNER_OPTIONS_TYPE_URL {
        return Ok(ProcessRunnerOptions::Elf(decode_elf_runner_options(
            &any.value,
        )?));
    }
    if any.type_url == bexos_wasm_abi::OPTIONS_TYPE_URL {
        return bexos_wasm_abi::WasmRunnerOptions::decode(&any.value)
            .map(ProcessRunnerOptions::Wasm)
            .map_err(|_| ManifestError::InvalidWasmOptions);
    }
    if any.type_url == bexos_starnix_abi::OPTIONS_TYPE_URL {
        return bexos_starnix_abi::NixRunnerOptions::decode(&any.value)
            .map(ProcessRunnerOptions::Nix)
            .map_err(|_| ManifestError::InvalidRunnerOptions);
    }
    Ok(ProcessRunnerOptions::Unknown(any))
}

fn decode_any_runner_options(bytes: &[u8]) -> Result<AnyRunnerOptions, ManifestError> {
    let mut options = AnyRunnerOptions::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => options.type_url = field.string()?,
            2 => options.value = field.bytes()?.to_vec(),
            _ => {}
        }
    }

    Ok(options)
}

fn decode_elf_runner_options(bytes: &[u8]) -> Result<ElfRunnerOptions, ManifestError> {
    let mut options = ElfRunnerOptions::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => options.path = field.string()?,
            _ => {}
        }
    }

    Ok(options)
}

fn decode_exposed_service(bytes: &[u8]) -> Result<ExposedService, ManifestError> {
    let mut service = ExposedService::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => service.name = field.string()?,
            2 => service.protocol = field.string()?,
            3 => service.lifecycle = Lifecycle::from_proto(field.varint()?),
            4 => service.visibility = Visibility::from_proto(field.varint()?),
            5 => {
                let permission = field.string()?;
                if !permission.is_empty() {
                    service.bind_permission = Some(permission);
                }
            }
            6 => service.metadata.push(decode_metadata(field.bytes()?)?),
            7 => service
                .capabilities
                .push(decode_capability_metadata(field.bytes()?)?),
            8 => service.activation = ServiceActivation::from_proto(field.varint()?),
            9 => {
                service.idle_timeout_ms = Some(
                    u32::try_from(field.varint()?)
                        .map_err(|_| ManifestError::InvalidLazyService)?,
                );
            }
            10 => {
                let provider_process = field.string()?;
                if !provider_process.is_empty() {
                    service.provider_process = Some(provider_process);
                }
            }
            _ => {}
        }
    }

    Ok(service)
}

fn decode_capability_metadata(bytes: &[u8]) -> Result<CapabilityMetadata, ManifestError> {
    let mut capability = CapabilityMetadata::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => capability.capability = field.string()?,
            2 => {
                let permission = field.string()?;
                if !permission.is_empty() {
                    capability.permission = Some(permission);
                }
            }
            3 => {
                if field.wire_type == 2 {
                    let mut packed = Cursor::new(field.bytes()?);
                    while packed.pos < packed.bytes.len() {
                        capability.method_ordinals.push(packed.read_varint()?);
                    }
                } else {
                    capability.method_ordinals.push(field.varint()?);
                }
            }
            _ => {}
        }
    }

    Ok(capability)
}

fn decode_consumed_service(bytes: &[u8]) -> Result<ConsumedService, ManifestError> {
    let mut service = ConsumedService::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => service.name = field.string()?,
            2 => service.link_type = LinkType::from_proto(field.varint()?),
            3 => {
                let filter = field.string()?;
                if !filter.is_empty() {
                    service.filter = Some(filter);
                }
            }
            4 => service
                .capabilities
                .push(decode_consumed_capability(field.bytes()?)?),
            _ => {}
        }
    }

    Ok(service)
}

fn decode_consumed_capability(bytes: &[u8]) -> Result<ConsumedCapability, ManifestError> {
    let mut capability = ConsumedCapability::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => capability.capability = field.string()?,
            2 => capability
                .methods
                .push(decode_method_dependency(field.bytes()?)?),
            _ => {}
        }
    }

    Ok(capability)
}

fn decode_method_dependency(bytes: &[u8]) -> Result<MethodDependency, ManifestError> {
    let mut dependency = MethodDependency::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => dependency.ordinal = field.varint()?,
            2 => dependency.link_type = LinkType::from_proto(field.varint()?),
            _ => {}
        }
    }

    Ok(dependency)
}

fn decode_metadata(bytes: &[u8]) -> Result<Metadata, ManifestError> {
    let mut metadata = Metadata::default();
    let mut cursor = Cursor::new(bytes);

    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => metadata.key = field.string()?,
            2 => metadata.value = field.string()?,
            _ => {}
        }
    }

    Ok(metadata)
}

impl Lifecycle {
    fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Singleton,
            2 => Self::UserScopedSingleton,
            3 => Self::MultipleInstance,
            _ => Self::Unspecified,
        }
    }
}

impl Visibility {
    fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Public,
            2 => Self::DomainShared,
            3 => Self::Private,
            _ => Self::Unspecified,
        }
    }
}

impl ServiceActivation {
    fn from_proto(value: u64) -> Self {
        match value {
            0 => Self::Eager,
            1 => Self::Lazy,
            _ => Self::Unspecified,
        }
    }
}

impl LinkType {
    fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Required,
            2 => Self::Optional,
            _ => Self::Unspecified,
        }
    }
}

impl BindBusType {
    const fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Pci,
            2 => Self::Usb,
            3 => Self::PlatformDt,
            4 => Self::I2c,
            5 => Self::Spi,
            _ => Self::Unspecified,
        }
    }
}

impl DriverColocationPolicy {
    const fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Colocated,
            2 => Self::HostShared,
            _ => Self::Isolated,
        }
    }
}

impl DriverRestartStrategy {
    fn from_proto(value: u64) -> Result<Self, ManifestError> {
        match value {
            0 => Ok(Self::HeartTransplant),
            _ => Err(ManifestError::InvalidDriver),
        }
    }
}

impl DriverHardwareResourceKind {
    const fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Mmio,
            2 => Self::Interrupt,
            3 => Self::DmaPool,
            4 => Self::IommuDomain,
            5 => Self::RegisterProxy,
            6 => Self::BusControl,
            _ => Self::Unspecified,
        }
    }
}

pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn next_field(&mut self) -> Result<Option<Field<'a>>, ManifestError> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }

        let key = self.read_varint()?;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x7) as u8;
        let value = match wire_type {
            0 => FieldValue::Varint(self.read_varint()?),
            1 => {
                let bytes = self.take(8)?;
                FieldValue::Bytes(bytes)
            }
            2 => {
                let len = self.read_varint()? as usize;
                FieldValue::Bytes(self.take(len)?)
            }
            5 => {
                let bytes = self.take(4)?;
                FieldValue::Bytes(bytes)
            }
            other => return Err(ManifestError::InvalidWireType(other)),
        };

        Ok(Some(Field {
            number,
            wire_type,
            value,
        }))
    }

    fn read_varint(&mut self) -> Result<u64, ManifestError> {
        let mut value = 0u64;
        let mut shift = 0u32;

        loop {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or(ManifestError::UnexpectedEof)?;
            self.pos += 1;

            if shift >= 64 {
                return Err(ManifestError::InvalidVarint);
            }

            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ManifestError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(ManifestError::LengthOverflow)?;
        if end > self.bytes.len() {
            return Err(ManifestError::UnexpectedEof);
        }
        let bytes = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
}

pub(crate) struct Field<'a> {
    pub(crate) number: u32,
    wire_type: u8,
    value: FieldValue<'a>,
}

impl<'a> Field<'a> {
    pub(crate) fn varint(&self) -> Result<u64, ManifestError> {
        match self.value {
            FieldValue::Varint(value) => Ok(value),
            FieldValue::Bytes(_) => Err(ManifestError::InvalidWireType(self.wire_type)),
        }
    }

    pub(crate) fn bytes(&self) -> Result<&'a [u8], ManifestError> {
        match self.value {
            FieldValue::Bytes(bytes) if self.wire_type == 2 => Ok(bytes),
            _ => Err(ManifestError::InvalidWireType(self.wire_type)),
        }
    }

    pub(crate) fn string(&self) -> Result<String, ManifestError> {
        let bytes = self.bytes()?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ManifestError::InvalidUtf8)
    }
}

enum FieldValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}
