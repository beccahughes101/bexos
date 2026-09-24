use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
mod elf;
mod kernel;
mod nix;
mod policy;
mod wasm;

pub use elf::{
    BssMapping, DEFAULT_STACK_SIZE, DEFAULT_STACK_TOP, ElfError, ElfMapping, ElfRunner, PAGE_SIZE,
    ParsedElf, TlsSegment,
};
pub use kernel::{
    CreatedChannel, CreatedProcess, CreatedResourceGroup, FakeKernelOps, KernelError,
    KernelFidlOps, KernelHandle, KernelOperation, KernelOps, ResourceGroupLimits,
};
pub use policy::PackageTrustTier;

use crate::manifest::{Manifest, PackageKind, Process};
use crate::platform_config::{
    DriverPolicyDecision, HardwareAccessTier, PackageIdentity, RunnerPolicy, RunnerPolicyDecision,
};

pub type RunnerOptions = crate::manifest::ProcessRunnerOptions;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunnerKind {
    Elf,
    Wasm,
    Web,
    Android,
    Nix,
    Unknown(String),
}

impl RunnerKind {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "elf" => Self::Elf,
            "wasm" => Self::Wasm,
            "web" => Self::Web,
            "android" => Self::Android,
            "nix" => Self::Nix,
            _ => Self::Unknown(value.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchRequest<'a> {
    pub manifest: &'a Manifest,
    pub process: &'a Process,
    pub trust_tier: PackageTrustTier,
    pub identity: PackageIdentity<'a>,
    pub runner_policy: Option<&'a RunnerPolicy>,
    pub hardware_access: HardwareAccessTier,
    pub realtime_scheduling: bool,
    pub resource_group_id: u32,
}

pub const REALTIME_SCHEDULING_PERMISSION: &str = "bexos.permission.REALTIME_SCHEDULING";

pub fn realtime_scheduling_for(
    manifest: &Manifest,
    process: &Process,
    identity: PackageIdentity<'_>,
    policy: Option<&RunnerPolicy>,
) -> bool {
    let declared = manifest
        .permissions
        .iter()
        .chain(process.permissions.iter())
        .any(|permission| permission.name == REALTIME_SCHEDULING_PERMISSION);
    declared && policy.is_some_and(|policy| policy.permits_realtime_scheduling(identity))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchResult {
    pub process_handle: KernelHandle,
    pub address_space_handle: KernelHandle,
    pub main_thread_handle: KernelHandle,
    pub service_manager_handle: KernelHandle,
    pub runtime_linker_data: Option<(KernelHandle, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchError {
    UnsupportedRunner(String),
    MissingRunnerOptions,
    InvalidWasmOptions,
    InvalidNixOptions,
    LibraryPackageNotLaunchable,
    RunnerPolicyDenied,
    ProcessNameTooLong,
    PackageImage(PackageImageError),
    Elf(ElfError),
    Kernel {
        operation: &'static str,
        source: KernelError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackageImageError {
    NotFound,
    AccessDenied,
    InvalidPath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Borrowed image: the resolver retains the VMO and may return it to later
/// launches. Runners own only the mappings and private VMOs they create.
pub struct PackageImage<'a> {
    pub bytes: &'a [u8],
    /// Borrowed from the resolver, which closes it after all launches finish.
    /// Runners may map read-only pages but must not close or mutate this VMO.
    pub vmo: KernelHandle,
    /// Offset where `bytes` begins within `vmo`.
    pub vmo_offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageLibrary<'a> {
    pub package_name: &'a str,
    pub export_name: &'a str,
    pub soname: &'a str,
    pub image: PackageImage<'a>,
    pub symbol_prefix: &'a str,
    pub abi_version: u32,
    pub kind: PackageLibraryKind,
    pub direct_dependencies: &'a [PackageLibraryDependency],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageLibraryKind {
    Native,
    WasmComponent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageLibraryDependency {
    pub package_name: String,
    pub abi_version: u32,
    pub soname: String,
}

pub trait PackageImageResolver {
    /// Only trusted resolvers may override the built-in runtime digest, after
    /// verifying a distinct platform runtime archive with the fixed package role.
    fn wasm_runtime_digest(&self) -> [u8; 32] {
        *include_bytes!(env!("BEXOS_WASM_RUNNER_DIGEST"))
    }

    /// Authenticated platform image, never resolved from the caller's package.
    fn resolve_wasm_runtime(&self) -> Result<PackageImage<'_>, PackageImageError> {
        self.resolve_executable(bexos_wasm_abi::RUNNER_PACKAGE, bexos_wasm_abi::RUNNER_PATH)
    }

    /// Digest of the authenticated platform Starnix runtime selected by this
    /// resolver. Migration resolvers may select the separately built candidate.
    fn starnix_runtime_digest(&self) -> [u8; 32] {
        *include_bytes!(env!("BEXOS_STARNIX_RUNNER_DIGEST"))
    }

    fn resolve_starnix_runtime(&self) -> Result<PackageImage<'_>, PackageImageError> {
        Ok(PackageImage {
            bytes: include_bytes!(env!("BEXOS_STARNIX_RUNNER")),
            vmo: KernelHandle::none(),
            vmo_offset: 0,
        })
    }

    fn resolve_executable<'a>(
        &'a self,
        package_name: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError>;

    fn resolve_library<'a>(
        &'a self,
        package_name: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        let _ = (package_name, abi_version);
        Err(PackageImageError::NotFound)
    }

    fn resolve_wasm_component<'a>(
        &'a self,
        package_name: &str,
        export_name: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        let _ = export_name;
        self.resolve_library(package_name, abi_version)
            .and_then(|library| {
                if library.kind == PackageLibraryKind::WasmComponent
                    && library.export_name == export_name
                {
                    Ok(library)
                } else {
                    Err(PackageImageError::AccessDenied)
                }
            })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunnerRegistry {
    elf: ElfRunner,
}

impl RunnerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn launch<K: KernelOps, R: PackageImageResolver>(
        &self,
        request: &LaunchRequest<'_>,
        kernel: &mut K,
        resolver: &R,
    ) -> Result<LaunchResult, LaunchError> {
        if matches!(
            request.manifest.package_kind,
            PackageKind::Library | PackageKind::TrustedApp
        ) {
            return Err(LaunchError::LibraryPackageNotLaunchable);
        }
        if let Some(policy) = request.runner_policy {
            match policy.evaluate_runner(&request.process.runner, request.identity) {
                RunnerPolicyDecision::Allow => {}
                RunnerPolicyDecision::Deny => return Err(LaunchError::RunnerPolicyDenied),
                RunnerPolicyDecision::RouteToMicrovm => {
                    return Err(LaunchError::UnsupportedRunner("microvm".to_string()));
                }
            }
        }

        match RunnerKind::parse(&request.process.runner) {
            RunnerKind::Elf => self.elf.launch(request, kernel, resolver),
            RunnerKind::Wasm => wasm::launch(&self.elf, request, kernel, resolver),
            RunnerKind::Nix => nix::launch(&self.elf, request, kernel, resolver),
            RunnerKind::Unknown(value) => Err(LaunchError::UnsupportedRunner(value)),
            other => Err(LaunchError::UnsupportedRunner(format!("{other:?}"))),
        }
    }
}

pub fn hardware_access_for_driver(
    decision: DriverPolicyDecision,
) -> Result<HardwareAccessTier, LaunchError> {
    match decision {
        DriverPolicyDecision::RejectAndIsolate => Err(LaunchError::RunnerPolicyDenied),
        DriverPolicyDecision::Allow { hardware_access } => Ok(hardware_access),
    }
}
pub mod deferred;

pub mod runtime_archive;

mod startup_guard;
pub use startup_guard::PendingWasmLaunch;
