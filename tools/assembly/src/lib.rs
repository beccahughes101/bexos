use std::collections::{BTreeMap, BTreeSet};

const CONFIG_MAGIC: &[u8; 8] = b"BEXCFG\0\0";
const MAX_CONFIG_SNAPSHOT_LEN: usize = 64 * 1024;

pub struct ProductInput<'a> {
    pub product: &'a [u8],
    pub bundles: &'a [Vec<u8>],
    pub manifests: &'a [(String, Vec<u8>)],
}

pub struct CompileConfigInput<'a> {
    pub manifest: &'a [u8],
    pub override_bytes: Option<&'a [u8]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblyOutput {
    pub product_name: String,
    pub board: String,
    pub platform_config: String,
    pub packages: Vec<AssembledPackage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledPackage {
    pub package_id: String,
    pub label: String,
    pub placement: Placement,
    pub config_path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placement {
    Bootfs,
    SystemImage,
    Unspecified,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ProductDefinition {
    product_name: String,
    board: String,
    platform_config: String,
    bundles: Vec<String>,
    overrides: Vec<ConfigOverride>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Bundle {
    name: String,
    packages: Vec<Package>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Package {
    label: String,
    package_id: String,
    placement: Placement,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AppManifest {
    package_name: String,
    process_count: usize,
    package_kind: PackageKind,
    library_dependencies: Vec<LibraryDependency>,
    library_exports: Vec<LibraryExport>,
    services_exposed: Vec<ExposedService>,
    services_consumed: Vec<ConsumedService>,
    schema: ConfigSchema,
    trusted_app: Option<TrustedAppInfo>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PackageKind {
    #[default]
    Application,
    Library,
    TrustedApp,
    Unspecified,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrustedAppInfo {
    provider: String,
    uuid_len: usize,
    secure_version: u64,
    archive_payload_path: String,
    ports: Vec<String>,
    protected: bool,
    uninstallable: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct LibraryDependency {
    package_name: String,
    version_requirement: String,
    mount_alias: String,
    abi_version: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct LibraryExport {
    name: String,
    path: String,
    symbol_prefix: String,
    abi_version: u32,
    kind: LibraryExportKind,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LibraryExportKind {
    #[default]
    Native,
    WasmComponent,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ExposedService {
    name: String,
    visibility: Visibility,
    capabilities: Vec<ServiceCapability>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ConsumedService {
    name: String,
    link_type: LinkType,
    capabilities: Vec<ConsumedCapability>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ServiceCapability {
    capability: String,
    method_ordinals: Vec<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ConsumedCapability {
    capability: String,
    methods: Vec<MethodDependency>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MethodDependency {
    ordinal: u64,
    link_type: LinkType,
}

impl Default for MethodDependency {
    fn default() -> Self {
        Self {
            ordinal: 0,
            link_type: LinkType::Unspecified,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Visibility {
    #[default]
    Unspecified,
    Public,
    DomainShared,
    Private,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LinkType {
    #[default]
    Unspecified,
    Required,
    Optional,
}

use bexos_component_config::schema::{
    Field as ConfigField, Schema as ConfigSchema, Type as ConfigType, Value as ConfigValue,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ConfigOverride {
    locked_fields: Vec<String>,
    package_id: String,
    values: Vec<ConfigAssignment>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ConfigAssignment {
    name: String,
    value: Option<ConfigValue>,
}

pub fn validate_product(input: ProductInput<'_>) -> Result<AssemblyOutput, String> {
    validate_product_for(input, bexos_app_manifest::Architecture::current_guest())
}

pub fn validate_product_for(
    input: ProductInput<'_>,
    architecture: bexos_app_manifest::Architecture,
) -> Result<AssemblyOutput, String> {
    let product = decode_product(input.product)?;
    let selected = product.bundles.iter().cloned().collect::<BTreeSet<_>>();
    let mut bundle_names = BTreeSet::new();
    let mut packages = Vec::new();
    for bytes in input.bundles {
        let bundle = decode_bundle(bytes)?;
        if !bundle_names.insert(bundle.name.clone()) {
            return Err(format!("duplicate bundle {}", bundle.name));
        }
        if selected.contains(&bundle.name) {
            packages.extend(bundle.packages);
        }
    }
    for name in &selected {
        if !bundle_names.contains(name) {
            return Err(format!("product selects missing bundle {name}"));
        }
    }

    let manifests = input
        .manifests
        .iter()
        .map(|(label, bytes)| {
            bexos_app_manifest::ManifestArchitecture::decode(bytes)
                .and_then(|manifest| manifest.validate(Some(architecture)))
                .map_err(|error| format!("manifest {label}: {error:?}; rebuild native packages for the selected architecture"))?;
            Ok((label.clone(), decode_manifest(bytes)?))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;

    let mut seen_labels = BTreeSet::new();
    let mut seen_ids = BTreeSet::new();
    let mut assembled = Vec::new();
    for package in packages {
        if !seen_labels.insert(package.label.clone()) {
            return Err(format!("duplicate package label {}", package.label));
        }
        let manifest = manifests
            .get(&package.label)
            .ok_or_else(|| format!("missing manifest for {}", package.label))?;
        let package_id = if package.package_id.is_empty() {
            manifest.package_name.clone()
        } else {
            package.package_id.clone()
        };
        if package_id != manifest.package_name {
            return Err(format!(
                "package {} declares id {}, manifest has {}",
                package.label, package_id, manifest.package_name
            ));
        }
        if !seen_ids.insert(package_id.clone()) {
            return Err(format!("duplicate package id {package_id}"));
        }
        assembled.push(AssembledPackage {
            package_id: package_id.clone(),
            label: package.label,
            placement: package.placement,
            config_path: if manifest.schema.fields.is_empty() {
                String::new()
            } else {
                format!("pkg/{package_id}/config/component.bexconfig")
            },
        });
    }

    let assembled_ids = assembled
        .iter()
        .map(|package| package.package_id.as_str())
        .collect::<BTreeSet<_>>();
    for manifest in manifests.values() {
        match manifest.package_kind {
            PackageKind::Application => {}
            PackageKind::Library => {
                if manifest.process_count != 0 {
                    return Err(format!(
                        "library package {} cannot declare processes",
                        manifest.package_name
                    ));
                }
                if !manifest.schema.fields.is_empty() {
                    return Err(format!(
                        "library package {} cannot declare component config",
                        manifest.package_name
                    ));
                }
            }
            PackageKind::TrustedApp => validate_trusted_app_manifest(manifest)?,
            PackageKind::Unspecified => {
                return Err(format!(
                    "package {} declares unsupported package_kind",
                    manifest.package_name
                ));
            }
        }
    }
    for package in &assembled {
        let manifest = manifests
            .values()
            .find(|manifest| manifest.package_name == package.package_id)
            .expect("assembled package checked above");
        for dependency in &manifest.library_dependencies {
            if dependency.package_name.is_empty() {
                return Err(format!(
                    "package {} declares library dependency without package_name",
                    manifest.package_name
                ));
            }
            if dependency.abi_version == 0 {
                return Err(format!(
                    "package {} declares library dependency {} without abi_version",
                    manifest.package_name, dependency.package_name
                ));
            }
            if !assembled_ids.contains(dependency.package_name.as_str()) {
                return Err(format!(
                    "package {} depends on missing library {}",
                    manifest.package_name, dependency.package_name
                ));
            }
            let dependency_manifest = manifests
                .values()
                .find(|candidate| candidate.package_name == dependency.package_name)
                .expect("dependency package id checked above");
            if dependency_manifest.package_kind != PackageKind::Library {
                return Err(format!(
                    "package {} depends on non-library package {}",
                    manifest.package_name, dependency.package_name
                ));
            }
            let Some(export) = dependency_manifest.library_exports.first() else {
                return Err(format!(
                    "package {} depends on library {} without exports",
                    manifest.package_name, dependency.package_name
                ));
            };
            if !export.usable_for_dependency() {
                return Err(format!(
                    "library package {} declares unusable library export",
                    dependency.package_name
                ));
            }
            if export.abi_version != dependency.abi_version {
                return Err(format!(
                    "package {} depends on library {} abi_version {}, but selected export has abi_version {}",
                    manifest.package_name,
                    dependency.package_name,
                    dependency.abi_version,
                    export.abi_version
                ));
            }
        }
    }
    validate_consumed_services(&assembled, &manifests)?;
    for override_values in &product.overrides {
        if !assembled_ids.contains(override_values.package_id.as_str()) {
            return Err(format!(
                "config override targets unknown package {}",
                override_values.package_id
            ));
        }
        let manifest = manifests
            .values()
            .find(|manifest| manifest.package_name == override_values.package_id)
            .expect("assembled package checked above");
        validate_config(&manifest.schema, Some(override_values))?;
    }

    Ok(AssemblyOutput {
        product_name: product.product_name,
        board: product.board,
        platform_config: product.platform_config,
        packages: assembled,
    })
}

fn validate_consumed_services(
    assembled: &[AssembledPackage],
    manifests: &BTreeMap<String, AppManifest>,
) -> Result<(), String> {
    let assembled_ids = assembled
        .iter()
        .map(|package| package.package_id.as_str())
        .collect::<BTreeSet<_>>();
    for package in assembled {
        let manifest = manifests
            .values()
            .find(|manifest| manifest.package_name == package.package_id)
            .expect("assembled package checked above");
        for consumed in &manifest.services_consumed {
            if consumed.name.is_empty() {
                return Err(format!(
                    "package {} declares consumed service without name",
                    manifest.package_name
                ));
            }
            if consumed.name.starts_with("bexos.kernel.") {
                continue;
            }
            let providers = manifests
                .values()
                .filter(|candidate| assembled_ids.contains(candidate.package_name.as_str()))
                .filter(|candidate| {
                    candidate
                        .services_exposed
                        .iter()
                        .any(|service| service.name == consumed.name)
                })
                .collect::<Vec<_>>();
            let Some(provider_manifest) = providers.first().copied() else {
                if consumed.link_type == LinkType::Optional {
                    continue;
                }
                return Err(format!(
                    "package {} depends on missing service {}",
                    manifest.package_name, consumed.name
                ));
            };
            let service = provider_manifest
                .services_exposed
                .iter()
                .find(|service| service.name == consumed.name)
                .expect("provider matched above");
            if !visibility_allows(
                &manifest.package_name,
                &provider_manifest.package_name,
                service.visibility,
            ) {
                if consumed.link_type == LinkType::Optional {
                    continue;
                }
                return Err(format!(
                    "package {} cannot see service {} from {}",
                    manifest.package_name, consumed.name, provider_manifest.package_name
                ));
            }
            validate_consumed_capabilities(manifest, consumed, service)?;
        }
    }
    Ok(())
}

fn validate_consumed_capabilities(
    manifest: &AppManifest,
    consumed: &ConsumedService,
    service: &ExposedService,
) -> Result<(), String> {
    if consumed.capabilities.is_empty() {
        return Ok(());
    }
    for requested in &consumed.capabilities {
        let Some(provider) = service
            .capabilities
            .iter()
            .find(|capability| capability.capability == requested.capability)
        else {
            return Err(format!(
                "package {} requests unknown capability {} on service {}",
                manifest.package_name, requested.capability, consumed.name
            ));
        };
        for method in &requested.methods {
            if method.ordinal == 0 {
                return Err(format!(
                    "package {} requests zero ordinal on capability {} service {}",
                    manifest.package_name, requested.capability, consumed.name
                ));
            }
            if method.link_type == LinkType::Required
                && !provider.method_ordinals.contains(&method.ordinal)
            {
                return Err(format!(
                    "package {} requests unknown ordinal {} on capability {} service {}",
                    manifest.package_name, method.ordinal, requested.capability, consumed.name
                ));
            }
        }
    }
    Ok(())
}

fn visibility_allows(client: &str, provider: &str, visibility: Visibility) -> bool {
    match visibility {
        Visibility::Public => true,
        Visibility::Private => client == provider,
        Visibility::DomainShared => package_domain(client) == package_domain(provider),
        Visibility::Unspecified => false,
    }
}

fn package_domain(package: &str) -> &str {
    package
        .split_once(':')
        .map_or(package, |(domain, _)| domain)
}

pub fn compile_config_blob(input: CompileConfigInput<'_>) -> Result<Vec<u8>, String> {
    let manifest = decode_manifest(input.manifest)?;
    let override_values = input.override_bytes.map(decode_override).transpose()?;
    if let Some(override_values) = &override_values {
        if override_values.package_id != manifest.package_name {
            return Err(format!(
                "override package {} does not match manifest {}",
                override_values.package_id, manifest.package_name
            ));
        }
    }
    let resolved = validate_config(&manifest.schema, override_values.as_ref())?;
    encode_config_blob(&manifest.schema, 0, &resolved)
}

pub fn compile_config_blob_from_product(
    manifest: &[u8],
    product: &[u8],
    package_id: &str,
) -> Result<Vec<u8>, String> {
    let manifest = decode_manifest(manifest)?;
    if manifest.package_name != package_id {
        return Err(format!(
            "package id {package_id} does not match manifest {}",
            manifest.package_name
        ));
    }
    let product = decode_product(product)?;
    let override_values = product
        .overrides
        .iter()
        .find(|override_values| override_values.package_id == package_id);
    let resolved = validate_config(&manifest.schema, override_values)?;
    encode_config_blob(&manifest.schema, 0, &resolved)
}

pub fn generate_config_rust(manifest: &[u8]) -> Result<String, String> {
    let manifest = decode_manifest(manifest)?;
    validate_config_schema(&manifest.schema)?;
    let mut out = String::new();
    out.push_str("// Generated by bexos_assembly config-rust; do not edit.\n");
    out.push_str("extern crate alloc;\n");
    out.push_str("use alloc::{string::{String, ToString}, vec::Vec};\n");
    out.push_str("use bexos_userspace::{Memory, Startup, config::ConfigTable};\n\n");
    out.push_str("#[derive(Clone, Debug, Eq, PartialEq)]\n");
    out.push_str("pub struct ComponentConfig {\n");
    for field in &manifest.schema.fields {
        out.push_str("    pub ");
        out.push_str(&field.name);
        out.push_str(": ");
        out.push_str(generated_field_type(field));
        out.push_str(",\n");
    }
    out.push_str("}\n\nimpl ComponentConfig {\n");
    out.push_str("    pub const SCHEMA_FINGERPRINT: u64 = ");
    out.push_str(&schema_fingerprint(&manifest.schema).to_string());
    out.push_str(";\n\n");
    out.push_str(&format!(
        "    pub const SCHEMA_BYTES: [u8; {}] = {:?};\n",
        manifest.schema.encode().len(),
        manifest.schema.encode()
    ));
    out.push_str("    pub fn from_startup(startup: &Startup) -> Result<Self, bexos_userspace::config::ConfigError> {\n");
    out.push_str("        let Some(config) = startup.config else { return Err(bexos_userspace::config::ConfigError::MissingField); };\n");
    out.push_str("        if startup.config_len == 0 { let _ = Memory::close(config); return Err(bexos_userspace::config::ConfigError::InvalidHeader); }\n");
    out.push_str("        let mapped_len = (startup.config_len + 4095) & !4095;\n");
    out.push_str("        let va = Memory::map(config, mapped_len, 2).map_err(|_| bexos_userspace::config::ConfigError::InvalidHeader)?;\n");
    out.push_str("        let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, startup.config_len as usize) };\n");
    out.push_str("        let parsed = ConfigTable::parse(bytes).and_then(Self::from_table);\n");
    out.push_str("        let _ = Memory::unmap(va, mapped_len);\n");
    out.push_str("        let _ = Memory::close(config);\n");
    out.push_str("        parsed\n    }\n\n");
    out.push_str("    pub fn from_table(table: ConfigTable<'_>) -> Result<Self, bexos_userspace::config::ConfigError> {\n");
    out.push_str("        if table.version() >= 2 && table.schema_fingerprint() != Self::SCHEMA_FINGERPRINT { return Err(bexos_userspace::config::ConfigError::InvalidHeader); }\n");
    out.push_str("        let schema = bexos_userspace::config::schema::Schema::decode(&Self::SCHEMA_BYTES).map_err(|_| bexos_userspace::config::ConfigError::InvalidHeader)?;\n");
    out.push_str("        schema.decode_table(table.as_bytes()).map_err(|_| bexos_userspace::config::ConfigError::InvalidEntry)?;\n");
    out.push_str("        Ok(Self {\n");
    for field in &manifest.schema.fields {
        out.push_str("            ");
        out.push_str(&field.name);
        out.push_str(": ");
        out.push_str(&generated_field_read(field)?);
        out.push_str(",\n");
    }
    out.push_str("        })\n    }\n}\n");
    out.push_str("impl bexos_userspace::preferences::DecodeConfig for ComponentConfig { fn from_config(table: ConfigTable<'_>) -> Result<Self, bexos_userspace::config::ConfigError> { Self::from_table(table) } }\n");
    out.push_str("pub type LiveComponentConfig = bexos_userspace::preferences::LiveConfig<ComponentConfig>;\n");
    Ok(out)
}

impl AssemblyOutput {
    pub fn index_text(&self) -> String {
        let mut out = format!(
            "product_name: {}\nboard: {}\nplatform_config: {}\n",
            self.product_name, self.board, self.platform_config
        );
        for package in &self.packages {
            out.push_str(&format!(
                "package: {} label={} placement={} config={}\n",
                package.package_id,
                package.label,
                package.placement.as_str(),
                package.config_path
            ));
        }
        out
    }

    pub fn labels_for_placement_text(&self, placement: &str) -> String {
        let mut out = String::new();
        for package in &self.packages {
            if package.placement.as_str() == placement {
                out.push_str(&package.label);
                out.push('\n');
            }
        }
        out
    }
}

impl Placement {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bootfs => "BOOTFS",
            Self::SystemImage => "SYSTEM_IMAGE",
            Self::Unspecified => "UNSPECIFIED",
        }
    }
}

impl Default for Placement {
    fn default() -> Self {
        Self::Unspecified
    }
}

fn validate_config(
    schema: &ConfigSchema,
    override_values: Option<&ConfigOverride>,
) -> Result<Vec<(String, ConfigType, Vec<u8>)>, String> {
    validate_config_schema(schema)?;
    if let Some(overrides) = override_values {
        let locks = overrides
            .locked_fields
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if locks.len() != overrides.locked_fields.len() {
            return Err("duplicate config lock".into());
        }
        schema.validate_locks(&locks).map_err(|e| e.to_string())?;
        let names = overrides
            .values
            .iter()
            .map(|v| &v.name)
            .collect::<BTreeSet<_>>();
        if names.len() != overrides.values.len() {
            return Err("duplicate config assignment".into());
        }
    }
    let overrides = override_values
        .map(|values| {
            values
                .values
                .iter()
                .map(|assignment| {
                    Ok((
                        assignment.name.as_str(),
                        assignment.value.clone().ok_or("missing override value")?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, &str>>()
        })
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let known = schema
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    for key in overrides.keys() {
        if !known.contains(key) {
            return Err(format!("unknown config field {key}"));
        }
    }
    let mut resolved = Vec::new();
    for field in &schema.fields {
        let value = overrides
            .get(field.name.as_str())
            .cloned()
            .or_else(|| field.default_value.clone());
        let Some(value) = value else {
            if field.required {
                return Err(format!("missing required config field {}", field.name));
            }
            continue;
        };
        let bytes = encode_value(field, &value)?;
        resolved.push((field.name.clone(), field.config_type, bytes));
    }
    Ok(resolved)
}

fn validate_config_schema(schema: &ConfigSchema) -> Result<(), String> {
    schema.validate()
}
fn encode_value(field: &ConfigField, value: &ConfigValue) -> Result<Vec<u8>, String> {
    field.validate_value(value)
}

fn generated_field_type(field: &ConfigField) -> &'static str {
    match (field.config_type, generated_field_optional(field)) {
        (ConfigType::Bool, false) => "bool",
        (ConfigType::Uint32, false) => "u32",
        (ConfigType::Uint64, false) => "u64",
        (ConfigType::String, false) => "String",
        (ConfigType::Bytes, false) => "Vec<u8>",
        (ConfigType::Bool, true) => "Option<bool>",
        (ConfigType::Uint32, true) => "Option<u32>",
        (ConfigType::Uint64, true) => "Option<u64>",
        (ConfigType::String, true) => "Option<String>",
        (ConfigType::Bytes, true) => "Option<Vec<u8>>",
        (ConfigType::Unspecified, _) => "()",
    }
}

fn generated_field_read(field: &ConfigField) -> Result<String, String> {
    if generated_field_optional(field) {
        return Ok(match field.config_type {
            ConfigType::Bool => format!("table.get_bool({:?}).ok()", field.name),
            ConfigType::Uint32 => format!("table.get_u32({:?}).ok()", field.name),
            ConfigType::Uint64 => format!("table.get_u64({:?}).ok()", field.name),
            ConfigType::String => format!(
                "table.get_string({:?}).ok().map(ToString::to_string)",
                field.name
            ),
            ConfigType::Bytes => format!(
                "table.get_bytes({:?}).ok().map(|value| value.to_vec())",
                field.name
            ),
            ConfigType::Unspecified => return Err("config fields require a name and type".into()),
        });
    }
    if let Some(default) = &field.default_value {
        return Ok(match (field.config_type, default) {
            (ConfigType::Bool, ConfigValue::Bool(value)) => {
                format!("table.get_bool({:?}).unwrap_or({value})", field.name)
            }
            (ConfigType::Uint32, ConfigValue::Uint32(value)) => {
                format!("table.get_u32({:?}).unwrap_or({value})", field.name)
            }
            (ConfigType::Uint64, ConfigValue::Uint64(value)) => {
                format!("table.get_u64({:?}).unwrap_or({value})", field.name)
            }
            (ConfigType::String, ConfigValue::String(value)) => format!(
                "table.get_string({:?}).unwrap_or({value:?}).to_string()",
                field.name
            ),
            (ConfigType::Bytes, ConfigValue::Bytes(value)) => {
                let bytes = value
                    .iter()
                    .map(|byte| byte.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "table.get_bytes({:?}).map(|value| value.to_vec()).unwrap_or_else(|_| alloc::vec![{bytes}])",
                    field.name
                )
            }
            _ => return Err(format!("type mismatch for config field {}", field.name)),
        });
    }
    Ok(match field.config_type {
        ConfigType::Bool => format!("table.get_bool({:?})?", field.name),
        ConfigType::Uint32 => format!("table.get_u32({:?})?", field.name),
        ConfigType::Uint64 => format!("table.get_u64({:?})?", field.name),
        ConfigType::String => format!("table.get_string({:?})?.to_string()", field.name),
        ConfigType::Bytes => format!("table.get_bytes({:?})?.to_vec()", field.name),
        ConfigType::Unspecified => return Err("config fields require a name and type".into()),
    })
}

fn generated_field_optional(field: &ConfigField) -> bool {
    !field.required && field.default_value.is_none()
}

fn encode_config_blob(
    schema: &ConfigSchema,
    generation: u64,
    entries: &[(String, ConfigType, Vec<u8>)],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(CONFIG_MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&schema_fingerprint(schema).to_le_bytes());
    out.extend_from_slice(&generation.to_le_bytes());
    for (name, config_type, value) in entries {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.push(config_type.to_wire());
        out.push(0);
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(value);
    }
    if out.len() > MAX_CONFIG_SNAPSHOT_LEN {
        Err("component config exceeds maximum snapshot size".to_string())
    } else {
        Ok(out)
    }
}

fn schema_fingerprint(schema: &ConfigSchema) -> u64 {
    schema.fingerprint()
}

fn decode_product(bytes: &[u8]) -> Result<ProductDefinition, String> {
    let mut product = ProductDefinition::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => product.product_name = field.string()?,
            2 => product.board = field.string()?,
            3 => product.platform_config = field.string()?,
            4 => product.bundles.push(field.string()?),
            5 => product.overrides.push(decode_override(field.bytes()?)?),
            _ => {}
        }
    }
    Ok(product)
}

fn decode_bundle(bytes: &[u8]) -> Result<Bundle, String> {
    let mut bundle = Bundle::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => bundle.name = field.string()?,
            2 => bundle.packages.push(decode_package(field.bytes()?)?),
            _ => {}
        }
    }
    Ok(bundle)
}

fn decode_package(bytes: &[u8]) -> Result<Package, String> {
    let mut package = Package::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => package.label = field.string()?,
            2 => package.package_id = field.string()?,
            3 => package.placement = Placement::from_proto(field.varint()?),
            _ => {}
        }
    }
    Ok(package)
}

fn decode_manifest(bytes: &[u8]) -> Result<AppManifest, String> {
    let mut manifest = AppManifest::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => manifest.package_name = field.string()?,
            3 => {
                let _ = field.bytes()?;
                manifest.process_count += 1;
            }
            5 => manifest
                .services_exposed
                .push(decode_exposed_service(field.bytes()?)?),
            6 => manifest
                .services_consumed
                .push(decode_consumed_service(field.bytes()?)?),
            10 => manifest.schema = decode_schema(field.bytes()?)?,
            11 => manifest.package_kind = PackageKind::from_proto(field.varint()?),
            12 => manifest
                .library_dependencies
                .push(decode_library_dependency(field.bytes()?)?),
            16 => manifest
                .library_exports
                .push(decode_library_export(field.bytes()?)?),
            19 => manifest.trusted_app = Some(decode_trusted_app_info(field.bytes()?)?),
            _ => {}
        }
    }
    Ok(manifest)
}

fn validate_trusted_app_manifest(manifest: &AppManifest) -> Result<(), String> {
    if manifest.process_count != 0
        || !manifest.library_exports.is_empty()
        || !manifest.services_exposed.is_empty()
    {
        return Err(format!(
            "trusted-app package {} cannot declare processes or library/service exports",
            manifest.package_name
        ));
    }
    let Some(info) = manifest.trusted_app.as_ref() else {
        return Err(format!(
            "trusted-app package {} missing trusted_app info",
            manifest.package_name
        ));
    };
    if info.provider != "trusty" {
        return Err(format!(
            "trusted-app package {} has unsupported provider {}",
            manifest.package_name, info.provider
        ));
    }
    if info.uuid_len != 16 || info.secure_version == 0 {
        return Err(format!(
            "trusted-app package {} has invalid uuid or secure version",
            manifest.package_name
        ));
    }
    if info.archive_payload_path.is_empty()
        || !info.archive_payload_path.starts_with("/pkg/")
        || info.archive_payload_path.contains("..")
    {
        return Err(format!(
            "trusted-app package {} has unsafe archive payload path",
            manifest.package_name
        ));
    }
    if info.ports.is_empty() || info.ports.iter().any(|port| port.is_empty()) {
        return Err(format!(
            "trusted-app package {} must declare non-empty service ports",
            manifest.package_name
        ));
    }
    Ok(())
}

fn decode_trusted_app_info(bytes: &[u8]) -> Result<TrustedAppInfo, String> {
    let mut info = TrustedAppInfo::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => info.provider = field.string()?,
            2 => info.uuid_len = field.bytes()?.len(),
            3 => info.secure_version = field.varint()?,
            4 => info.archive_payload_path = field.string()?,
            5 => info.ports.push(field.string()?),
            6 => info.protected = field.varint()? != 0,
            7 => info.uninstallable = field.varint()? != 0,
            _ => {}
        }
    }
    Ok(info)
}

fn decode_exposed_service(bytes: &[u8]) -> Result<ExposedService, String> {
    let mut service = ExposedService::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => service.name = field.string()?,
            4 => service.visibility = Visibility::from_proto(field.varint()?),
            7 => service
                .capabilities
                .push(decode_service_capability(field.bytes()?)?),
            _ => {}
        }
    }
    if service.capabilities.is_empty() {
        service.capabilities.push(ServiceCapability {
            capability: "Public".to_string(),
            method_ordinals: Vec::new(),
        });
    }
    Ok(service)
}

fn decode_consumed_service(bytes: &[u8]) -> Result<ConsumedService, String> {
    let mut service = ConsumedService::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => service.name = field.string()?,
            2 => service.link_type = LinkType::from_proto(field.varint()?),
            4 => service
                .capabilities
                .push(decode_consumed_capability(field.bytes()?)?),
            _ => {}
        }
    }
    Ok(service)
}

fn decode_service_capability(bytes: &[u8]) -> Result<ServiceCapability, String> {
    let mut capability = ServiceCapability::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => capability.capability = field.string()?,
            3 => read_repeated_u64(field, &mut capability.method_ordinals)?,
            _ => {}
        }
    }
    Ok(capability)
}

fn decode_consumed_capability(bytes: &[u8]) -> Result<ConsumedCapability, String> {
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

fn decode_method_dependency(bytes: &[u8]) -> Result<MethodDependency, String> {
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

fn read_repeated_u64(field: Field<'_>, values: &mut Vec<u64>) -> Result<(), String> {
    if field.wire_type == 2 {
        let mut packed = Cursor::new(field.bytes()?);
        while packed.pos < packed.bytes.len() {
            values.push(packed.read_varint()?);
        }
    } else {
        values.push(field.varint()?);
    }
    Ok(())
}

fn decode_library_dependency(bytes: &[u8]) -> Result<LibraryDependency, String> {
    let mut dependency = LibraryDependency::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => dependency.package_name = field.string()?,
            2 => dependency.version_requirement = field.string()?,
            3 => dependency.mount_alias = field.string()?,
            4 => dependency.abi_version = field.varint()? as u32,
            _ => {}
        }
    }
    Ok(dependency)
}

impl LibraryExport {
    fn usable_for_dependency(&self) -> bool {
        if self.path.is_empty() || self.abi_version == 0 {
            return false;
        }
        match self.kind {
            LibraryExportKind::Native => !self.symbol_prefix.is_empty(),
            LibraryExportKind::WasmComponent => !self.name.is_empty(),
        }
    }
}

fn decode_library_export(bytes: &[u8]) -> Result<LibraryExport, String> {
    let mut export = LibraryExport::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => export.name = field.string()?,
            2 => export.path = field.string()?,
            3 => export.symbol_prefix = field.string()?,
            4 => export.abi_version = field.varint()? as u32,
            6 => {
                export.kind = match field.varint()? {
                    0 => LibraryExportKind::Native,
                    1 => LibraryExportKind::WasmComponent,
                    other => return Err(format!("unsupported library export kind {other}")),
                }
            }
            _ => {}
        }
    }
    Ok(export)
}

fn decode_schema(bytes: &[u8]) -> Result<ConfigSchema, String> {
    ConfigSchema::decode(bytes).map_err(|e| e.to_string())
}

fn decode_override(bytes: &[u8]) -> Result<ConfigOverride, String> {
    let mut override_values = ConfigOverride::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => override_values.package_id = field.string()?,
            2 => override_values
                .values
                .push(decode_assignment(field.bytes()?)?),
            3 => override_values.locked_fields.push(field.string()?),
            _ => {}
        }
    }
    Ok(override_values)
}

fn decode_assignment(bytes: &[u8]) -> Result<ConfigAssignment, String> {
    let mut assignment = ConfigAssignment::default();
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => assignment.name = field.string()?,
            2 => assignment.value = Some(decode_config_value(field.bytes()?)?),
            _ => {}
        }
    }
    Ok(assignment)
}

fn decode_config_value(bytes: &[u8]) -> Result<ConfigValue, String> {
    let mut cursor = Cursor::new(bytes);
    let mut value = None;
    while let Some(field) = cursor.next_field()? {
        value = Some(match field.number {
            1 => ConfigValue::Bool(field.varint()? != 0),
            2 => ConfigValue::Uint32(field.varint()? as u32),
            3 => ConfigValue::Uint64(field.varint()?),
            4 => ConfigValue::String(field.string()?),
            5 => ConfigValue::Bytes(field.bytes()?.to_vec()),
            _ => continue,
        });
    }
    value.ok_or_else(|| "missing config value".to_string())
}

impl Placement {
    fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Bootfs,
            2 => Self::SystemImage,
            _ => Self::Unspecified,
        }
    }
}

impl PackageKind {
    fn from_proto(value: u64) -> Self {
        match value {
            0 => Self::Application,
            1 => Self::Library,
            2 => Self::TrustedApp,
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

impl LinkType {
    fn from_proto(value: u64) -> Self {
        match value {
            1 => Self::Required,
            2 => Self::Optional,
            _ => Self::Unspecified,
        }
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn next_field(&mut self) -> Result<Option<Field<'a>>, String> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }
        let key = self.read_varint()?;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x7) as u8;
        let value = match wire_type {
            0 => FieldValue::Varint(self.read_varint()?),
            2 => {
                let len = self.read_varint()? as usize;
                FieldValue::Bytes(self.take(len)?)
            }
            other => return Err(format!("invalid wire type {other}")),
        };
        Ok(Some(Field {
            number,
            wire_type,
            value,
        }))
    }

    fn read_varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.bytes.get(self.pos).ok_or("unexpected eof")?;
            self.pos += 1;
            if shift >= 64 {
                return Err("invalid varint".to_string());
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(len).ok_or("length overflow")?;
        let bytes = self.bytes.get(self.pos..end).ok_or("unexpected eof")?;
        self.pos = end;
        Ok(bytes)
    }
}

struct Field<'a> {
    number: u32,
    wire_type: u8,
    value: FieldValue<'a>,
}

impl<'a> Field<'a> {
    fn varint(&self) -> Result<u64, String> {
        match self.value {
            FieldValue::Varint(value) => Ok(value),
            FieldValue::Bytes(_) => Err(format!("expected varint, got {}", self.wire_type)),
        }
    }

    fn bytes(&self) -> Result<&'a [u8], String> {
        match self.value {
            FieldValue::Bytes(bytes) => Ok(bytes),
            FieldValue::Varint(_) => Err(format!("expected bytes, got {}", self.wire_type)),
        }
    }

    fn string(&self) -> Result<String, String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| "invalid utf8".to_string())
    }
}

enum FieldValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

pub mod test_proto {
    pub fn varint_field(number: u32, value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        push_varint(&mut out, u64::from(number << 3));
        push_varint(&mut out, value);
        out
    }

    pub fn string_field(number: u32, value: &str) -> Vec<u8> {
        bytes_field(number, value.as_bytes())
    }

    pub fn bytes_field(number: u32, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        push_varint(&mut out, u64::from((number << 3) | 2));
        push_varint(&mut out, bytes.len() as u64);
        out.extend_from_slice(bytes);
        out
    }

    pub fn message_field(number: u32, fields: Vec<Vec<u8>>) -> Vec<u8> {
        let message = concat(fields);
        bytes_field(number, &message)
    }

    pub fn concat(fields: Vec<Vec<u8>>) -> Vec<u8> {
        fields.into_iter().flatten().collect()
    }

    fn push_varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }
}

pub fn compile_config_policy(
    manifest: &[u8],
    product: &[u8],
    package_id: &str,
) -> Result<Vec<u8>, String> {
    let manifest = decode_manifest(manifest)?;
    if manifest.package_name != package_id {
        return Err("policy package mismatch".into());
    }
    let product = decode_product(product)?;
    let values = product
        .overrides
        .iter()
        .find(|v| v.package_id == package_id);
    validate_config(&manifest.schema, values)?;
    let mut out = Vec::new();
    bexos_component_config::wire::word(&mut out, 1, manifest.schema.fingerprint());
    if let Some(values) = values {
        for name in &values.locked_fields {
            bexos_component_config::wire::bytes(&mut out, 2, name.as_bytes());
        }
    }
    Ok(out)
}
