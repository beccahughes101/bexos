use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
pub(crate) mod bootstrap;
pub mod kernel;
mod policy;
pub mod precreated;

pub use bexos_elf::{BssMapping, ElfError, ElfMapping, ParsedElf, TlsSegment};
pub const DEFAULT_STACK_SIZE: u64 = bexos_boot::USER_STACK_SIZE;
pub const DEFAULT_STACK_TOP: u64 = bexos_boot::USER_STACK_TOP;
pub const PAGE_SIZE: u64 = bexos_kernel_core::loader::PAGE_SIZE;
pub use kernel::{
    ComponentStartRequest, CreatedChannel, CreatedJob, CreatedProcess, CreatedResourceGroup,
    FakeKernelOps, KernelError, KernelFidlOps, KernelHandle, KernelOperation, KernelOps,
    ResourceGroupLimits,
};
pub use policy::PackageTrustTier;
pub use precreated::PrecreatedKernel;

use crate::manifest::{Manifest, PackageKind, Process};
use crate::platform_config::{
    ComponentRunnerProviderKind, DriverPolicyDecision, HardwareAccessTier, PackageIdentity,
    RunnerPolicy, RunnerPolicyDecision,
};

pub type RunnerOptions = crate::manifest::ProcessRunnerOptions;

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
pub const COMPONENT_STOP_GRACE_NS: u64 = 5_000_000_000;
const COMPONENT_JOB_RUNNER_RIGHTS: u32 = 1 | 2 | 32 | 128;
const IMMUTABLE_VMO_RIGHTS: u32 = 1 | 2 | 16;
pub const NATIVE_RUNNER_PACKAGE: &str = bootstrap::PACKAGE;

pub const fn component_stop_deadline(now_ns: u64) -> u64 {
    now_ns.saturating_add(COMPONENT_STOP_GRACE_NS)
}

/// Applies the controller Stop grace period. The component and its disposable
/// native-runner host form one failure boundary, so escalation always tears
/// down both jobs.
pub fn enforce_stop_deadline<K: KernelOps>(
    kernel: &mut K,
    now_ns: u64,
    deadline_ns: u64,
    component_job: KernelHandle,
    native_host_job: KernelHandle,
    exit_code: i32,
) -> Result<bool, KernelError> {
    if deadline_ns == 0 || now_ns < deadline_ns {
        return Ok(false);
    }
    kernel.terminate_job(component_job, exit_code)?;
    if !native_host_job.is_none() {
        kernel.terminate_job(native_host_job, exit_code)?;
    }
    Ok(true)
}

/// Terminates the isolated component boundary after loss of either runner
/// protocol endpoint. This is deliberately independent of restart policy;
/// appd applies the existing policy after observing the terminated process.
pub fn terminate_runner_boundary<K: KernelOps>(
    kernel: &mut K,
    component_job: KernelHandle,
    native_host_job: KernelHandle,
    exit_code: i32,
) -> Result<(), KernelError> {
    kernel.terminate_job(component_job, exit_code)?;
    if !native_host_job.is_none() {
        kernel.terminate_job(native_host_job, exit_code)?;
    }
    Ok(())
}

fn immutable_vmo<K: KernelOps>(
    kernel: &mut K,
    bytes: &[u8],
    operation: &'static str,
) -> Result<KernelHandle, LaunchError> {
    let writable = kernel
        .create_vmo_from_bytes(bytes)
        .map_err(|source| kernel_error(operation, source))?;
    let read_only = match kernel.duplicate_handle(writable, IMMUTABLE_VMO_RIGHTS) {
        Ok(handle) => handle,
        Err(source) => {
            let _ = kernel.release_vmo(writable);
            return Err(kernel_error(operation, source));
        }
    };
    if let Err(source) = kernel.release_vmo(writable) {
        let _ = kernel.release_vmo(read_only);
        return Err(kernel_error(operation, source));
    }
    Ok(read_only)
}

fn start_component_protocol<K: KernelOps>(
    kernel: &mut K,
    mut result: LaunchResult,
    resolved_url: &str,
    runner: &str,
    program_type_url: &str,
    program: &[u8],
    service: bool,
    migratable: bool,
    directories: &PackageDirectories,
    package_image: Option<(KernelHandle, u64)>,
) -> Result<LaunchResult, LaunchError> {
    let startup = match kernel.create_channel() {
        Ok(channel) => channel,
        Err(source) => {
            cleanup_component_launch(kernel, &result);
            return Err(kernel_error("component_startup_channel", source));
        }
    };
    let controller = match kernel.create_channel() {
        Ok(channel) => channel,
        Err(source) => {
            close_all(kernel, &[startup.local, startup.remote]);
            cleanup_component_launch(kernel, &result);
            return Err(kernel_error("component_controller_channel", source));
        }
    };
    let events = match kernel.create_channel() {
        Ok(channel) => channel,
        Err(source) => {
            close_all(
                kernel,
                &[
                    startup.local,
                    startup.remote,
                    controller.local,
                    controller.remote,
                ],
            );
            cleanup_component_launch(kernel, &result);
            return Err(kernel_error("component_events_channel", source));
        }
    };
    let delegated_job =
        match kernel.duplicate_handle(result.job_handle, COMPONENT_JOB_RUNNER_RIGHTS) {
            Ok(handle) => handle,
            Err(source) => {
                close_all(
                    kernel,
                    &[
                        startup.local,
                        startup.remote,
                        controller.local,
                        controller.remote,
                        events.local,
                        events.remote,
                    ],
                );
                cleanup_component_launch(kernel, &result);
                return Err(kernel_error("component_job_duplicate", source));
            }
        };
    let runner_channel = result.service_manager_handle;
    let dependency_handles = directories
        .dependencies
        .iter()
        .map(|dependency| kernel::ResolvedDependencyHandle {
            package_name: dependency.package_name.as_str(),
            mount_alias: dependency.mount_alias.as_str(),
            export_name: dependency.export_name.as_str(),
            export_path: dependency.export_path.as_str(),
            symbol_prefix: dependency.symbol_prefix.as_str(),
            soname: dependency.soname.as_str(),
            abi_version: dependency.abi_version,
            kind: dependency.kind,
            direct_dependencies: &dependency.direct_dependencies,
            directory: dependency.directory,
        })
        .collect::<Vec<_>>();
    let sent = kernel.send_component_start(
        runner_channel,
        &ComponentStartRequest {
            resolved_url,
            runner,
            program_type_url,
            program,
            service,
            migratable,
            package_dir: directories.package_dir,
            package_image: package_image.map(|(handle, _)| handle),
            package_image_size: package_image.map_or(0, |(_, size)| size),
            dependencies: &dependency_handles,
            startup: startup.remote,
            job: delegated_job,
            controller: controller.remote,
            events: events.remote,
        },
    );
    if let Err(source) = sent {
        for handle in [
            startup.local,
            startup.remote,
            controller.local,
            controller.remote,
            events.local,
            events.remote,
            delegated_job,
        ] {
            let _ = kernel.close_handle(handle);
        }
        cleanup_component_launch(kernel, &result);
        return Err(kernel_error("component_runner_start", source));
    }
    if let Err(source) = kernel.close_handle(runner_channel) {
        close_all(kernel, &[startup.local, controller.local, events.local]);
        cleanup_component_launch(kernel, &result);
        return Err(kernel_error("component_runner_channel_close", source));
    }
    result.service_manager_handle = startup.local;
    result.controller_handle = controller.local;
    result.events_handle = events.local;
    Ok(result)
}

fn close_all<K: KernelOps>(kernel: &mut K, handles: &[KernelHandle]) {
    for handle in handles.iter().copied().filter(|handle| !handle.is_none()) {
        let _ = kernel.close_handle(handle);
    }
}

fn cleanup_component_launch<K: KernelOps>(kernel: &mut K, result: &LaunchResult) {
    let _ = kernel.terminate_job(result.job_handle, -1);
    if let Some((handle, _)) = result.runtime_linker_data {
        let _ = kernel.release_vmo(handle);
    }
    close_all(
        kernel,
        &[
            result.service_manager_handle,
            result.main_thread_handle,
            result.address_space_handle,
            result.process_handle,
            result.job_handle,
            result.controller_handle,
            result.events_handle,
        ],
    );
    if let Some(host) = result.native_host {
        let _ = kernel.terminate_job(host.job, -1);
        close_all(
            kernel,
            &[host.thread, host.address_space, host.process, host.job],
        );
    }
}

fn kernel_error(operation: &'static str, source: KernelError) -> LaunchError {
    LaunchError::Kernel { operation, source }
}

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
    pub job_handle: KernelHandle,
    pub process_handle: KernelHandle,
    pub address_space_handle: KernelHandle,
    pub main_thread_handle: KernelHandle,
    pub service_manager_handle: KernelHandle,
    pub controller_handle: KernelHandle,
    pub events_handle: KernelHandle,
    pub runtime_linker_data: Option<(KernelHandle, u64)>,
    /// Disposable fixed host. It is deliberately distinct from the retained
    /// component job/process and is killed on host-channel loss.
    pub native_host: Option<NativeHostHandles>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeHostHandles {
    pub job: KernelHandle,
    pub process: KernelHandle,
    pub address_space: KernelHandle,
    pub thread: KernelHandle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchError {
    UnsupportedRunner(String),
    MissingRunnerOptions,
    InvalidElfOptions,
    InvalidWasmOptions,
    InvalidNixOptions,
    InvalidProgramMetadata,
    InvalidNativeRunner,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageDirectoryDependency {
    pub package_name: String,
    pub mount_alias: String,
    pub export_name: String,
    pub export_path: String,
    pub symbol_prefix: String,
    pub soname: String,
    pub abi_version: u32,
    pub kind: PackageLibraryKind,
    pub direct_dependencies: Vec<PackageLibraryDependency>,
    pub directory: KernelHandle,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageDirectories {
    pub package_dir: Option<KernelHandle>,
    pub dependencies: Vec<PackageDirectoryDependency>,
}

pub trait PackageImageResolver {
    /// True when the runner can read component payloads from the directory
    /// capabilities returned by `component_directories`. BootFS launch keeps
    /// using bounded immutable VMOs until the filesystem service is online.
    fn supports_directory_payloads(&self) -> bool {
        false
    }

    fn component_directories(
        &self,
        _package_name: &str,
    ) -> Result<PackageDirectories, PackageImageError> {
        Ok(PackageDirectories::default())
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
pub struct RunnerRegistry;

pub(super) struct PreparedProgram {
    type_url: String,
    program: Vec<u8>,
    service: bool,
    migratable: bool,
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
        let policy = request
            .runner_policy
            .ok_or(LaunchError::RunnerPolicyDenied)?;
        let provider = policy
            .provider_for(&request.process.runner)
            .ok_or(LaunchError::RunnerPolicyDenied)?;
        match policy.evaluate_runner(&request.process.runner, request.identity) {
            RunnerPolicyDecision::Allow => {}
            RunnerPolicyDecision::Deny => return Err(LaunchError::RunnerPolicyDenied),
            RunnerPolicyDecision::RouteToMicrovm => {
                return Err(LaunchError::UnsupportedRunner("microvm".to_string()));
            }
        }
        let provider_kind = provider.kind;
        if provider_kind == ComponentRunnerProviderKind::ComponentRunner
            && (request.identity.is_driver
                || request.manifest.driver_info.is_some()
                || request.hardware_access != HardwareAccessTier::None)
        {
            return Err(LaunchError::RunnerPolicyDenied);
        }
        let program = request
            .process
            .opaque_runner_program()
            .map_err(|_| LaunchError::InvalidProgramMetadata)?;
        if program.type_url.is_empty()
            || program.type_url.len() > 256
            || program.value.is_empty()
            || program.value.len() > 65_536
        {
            return Err(LaunchError::InvalidProgramMetadata);
        }
        let prepared = PreparedProgram {
            type_url: program.type_url,
            program: program.value,
            service: request.process.service,
            migratable: request.process.lifecycle.update_strategy
                == crate::manifest::UpdateStrategy::HeartTransplant,
        };
        let launched = self.launch_isolated(request, provider, &prepared, kernel, resolver);
        launched
    }

    fn launch_isolated<K: KernelOps, R: PackageImageResolver>(
        &self,
        request: &LaunchRequest<'_>,
        provider: &crate::platform_config::ComponentRunnerProvider,
        prepared: &PreparedProgram,
        kernel: &mut K,
        resolver: &R,
    ) -> Result<LaunchResult, LaunchError> {
        let mut construction_handles = Vec::new();
        let mut construction_vmos = Vec::new();
        let mut construction_jobs = Vec::new();
        let launched = (|| -> Result<LaunchResult, LaunchError> {
            let (provider_kind, provider_package, provider_path) = match provider.kind {
                ComponentRunnerProviderKind::ComponentRunner => (
                    ComponentRunnerProviderKind::ComponentRunner,
                    provider.package_id.as_str(),
                    provider.executable_path.as_str(),
                ),
                ComponentRunnerProviderKind::DirectElf => {
                    let Some(crate::manifest::ProcessRunnerOptions::Elf(options)) =
                        request.process.runner_options.as_ref()
                    else {
                        return Err(LaunchError::MissingRunnerOptions);
                    };
                    (
                        ComponentRunnerProviderKind::DirectElf,
                        request.manifest.package_name.as_str(),
                        options.path.as_str(),
                    )
                }
                ComponentRunnerProviderKind::Unspecified => {
                    return Err(LaunchError::RunnerPolicyDenied);
                }
            };
            let process_name =
                format!("{}:{}", request.manifest.package_name, request.process.name);
            if process_name.len() > 64 {
                return Err(LaunchError::ProcessNameTooLong);
            }
            let component_job = kernel
                .create_component_job(
                    &process_name,
                    request.resource_group_id,
                    request.identity.package_id,
                    request.hardware_access,
                    request.realtime_scheduling,
                    1,
                )
                .map_err(|source| kernel_error("create_component_job", source))?;
            construction_jobs.push(component_job.job);
            construction_handles.push(component_job.job);
            let component = match kernel.create_process_in_job(component_job.job, &process_name) {
                Ok(process) => process,
                Err(source) => {
                    let _ = kernel.terminate_job(component_job.job, -1);
                    let _ = kernel.close_handle(component_job.job);
                    return Err(kernel_error("create_process_in_job", source));
                }
            };
            construction_handles.extend([
                component.process,
                component.address_space,
                component.root_vmar,
            ]);
            let native_image = resolver
                .resolve_executable(bootstrap::PACKAGE, bootstrap::PATH)
                .map_err(LaunchError::PackageImage)?;
            let host = match bootstrap::launch(kernel, native_image) {
                Ok(host) => host,
                Err(error) => {
                    let _ = kernel.terminate_job(component_job.job, -1);
                    return Err(error);
                }
            };
            construction_jobs.push(host.job_handle);
            construction_handles.extend([
                host.job_handle,
                host.process_handle,
                host.address_space_handle,
                host.main_thread_handle,
                host.service_manager_handle,
            ]);
            let runner = kernel
                .create_channel()
                .map_err(|source| kernel_error("native_runner_endpoint", source))?;
            construction_handles.extend([runner.local, runner.remote]);
            let events = kernel
                .create_channel()
                .map_err(|source| kernel_error("native_runner_events", source))?;
            construction_handles.extend([events.local, events.remote]);
            let directories = resolver
                .component_directories(&request.manifest.package_name)
                .map_err(LaunchError::PackageImage)?;
            let component_image = if provider_kind == ComponentRunnerProviderKind::ComponentRunner
                && directories.package_dir.is_none()
            {
                if resolver.supports_directory_payloads() {
                    return Err(LaunchError::PackageImage(PackageImageError::AccessDenied));
                }
                let path = component_payload_path(request.process)
                    .ok_or(LaunchError::InvalidProgramMetadata)?;
                let image = resolver
                    .resolve_executable(&request.manifest.package_name, path)
                    .map_err(LaunchError::PackageImage)?;
                let vmo = immutable_vmo(kernel, image.bytes, "component_payload_vmo")?;
                construction_vmos.push(vmo);
                Some((vmo, image.bytes.len() as u64))
            } else {
                None
            };
            if provider_kind == ComponentRunnerProviderKind::ComponentRunner
                && directories
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.directory.is_none())
            {
                return Err(LaunchError::PackageImage(PackageImageError::AccessDenied));
            }
            if let Some(package_dir) = directories.package_dir {
                construction_handles.push(package_dir);
            }
            construction_handles.extend(
                directories
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.directory),
            );
            let mut host_directories = resolver
                .component_directories(provider_package)
                .map_err(LaunchError::PackageImage)?;
            if provider_kind == ComponentRunnerProviderKind::DirectElf
                && host_directories.package_dir.is_none()
                && host_directories.dependencies.is_empty()
            {
                host_directories.dependencies =
                    bootfs_dependency_metadata(resolver, &request.manifest.library_dependencies)?;
            }
            if let Some(package_dir) = host_directories.package_dir {
                construction_handles.push(package_dir);
            }
            construction_handles.extend(
                host_directories
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.directory),
            );
            let host_dependencies = host_directories
                .dependencies
                .iter()
                .map(directory_dependency)
                .collect::<Vec<_>>();
            let provider_image = if host_directories.package_dir.is_none() {
                let image = resolver
                    .resolve_executable(provider_package, provider_path)
                    .map_err(LaunchError::PackageImage)?;
                let vmo = immutable_vmo(kernel, image.bytes, "native_runner_provider_vmo")?;
                construction_vmos.push(vmo);
                Some((vmo, image.bytes.len() as u64))
            } else {
                None
            };
            let use_dependency_images = host_directories
                .dependencies
                .iter()
                .any(|dependency| dependency.directory.is_none());
            let mut dependency_images = Vec::new();
            let mut dependency_image_sizes = Vec::new();
            if use_dependency_images {
                for dependency in &host_directories.dependencies {
                    let image = resolver
                        .resolve_library(&dependency.package_name, dependency.abi_version)
                        .map_err(LaunchError::PackageImage)?;
                    let vmo =
                        immutable_vmo(kernel, image.image.bytes, "native_runner_dependency_vmo")?;
                    dependency_images.push(vmo);
                    dependency_image_sizes.push(image.image.bytes.len() as u64);
                    construction_vmos.push(vmo);
                }
            }
            let delegated_job = kernel
                .duplicate_handle(component_job.job, COMPONENT_JOB_RUNNER_RIGHTS)
                .map_err(|source| kernel_error("native_runner_job_duplicate", source))?;
            construction_handles.push(delegated_job);
            let delegated_process = kernel
                .duplicate_handle(component.process, 1 | 2 | 32 | 64 | 128)
                .map_err(|source| kernel_error("native_runner_process_duplicate", source))?;
            construction_handles.push(delegated_process);
            let delegated_space = kernel
                .duplicate_handle(component.address_space, 1 | 2 | 16 | 32)
                .map_err(|source| kernel_error("native_runner_space_duplicate", source))?;
            construction_handles.push(delegated_space);
            let delegated_vmar = kernel
                .duplicate_handle(component.root_vmar, 1 | 2 | 4 | 8 | 16 | 32)
                .map_err(|source| kernel_error("native_runner_vmar_duplicate", source))?;
            construction_handles.push(delegated_vmar);
            let prepared_target = kernel.prepare_native_runner(
                host.service_manager_handle,
                &kernel::NativeRunnerPrepareRequest {
                    provider_kind,
                    provider_package,
                    provider_path,
                    provider_package_dir: host_directories.package_dir,
                    provider_image: provider_image.map(|(handle, _)| handle),
                    provider_image_size: provider_image.map_or(0, |(_, size)| size),
                    dependencies: &host_dependencies,
                    dependency_images: &dependency_images,
                    dependency_image_sizes: &dependency_image_sizes,
                    target_process: delegated_process,
                    target_address_space: delegated_space,
                    target_root_vmar: delegated_vmar,
                    target_job: delegated_job,
                    runner: runner.remote,
                    events: events.remote,
                    events_reply: events.local,
                },
            );
            let prepared_target = match prepared_target {
                Ok(prepared) => prepared,
                Err(source) => {
                    let _ = kernel.terminate_job(component_job.job, -1);
                    let _ = kernel.terminate_job(host.job_handle, -1);
                    return Err(kernel_error("native_runner_prepare", source));
                }
            };
            construction_handles.push(prepared_target.main_thread);
            if let Some((linker_data, _)) = prepared_target.runtime_linker_data {
                construction_vmos.push(linker_data);
            }
            kernel
                .close_handle(host.service_manager_handle)
                .map_err(|source| kernel_error("native_runner_host_close", source))?;
            kernel
                .close_handle(events.local)
                .map_err(|source| kernel_error("native_runner_events_close", source))?;
            kernel
                .close_handle(component.root_vmar)
                .map_err(|source| kernel_error("component_root_vmar_close", source))?;
            if let Some((linker_data, _)) = prepared_target.runtime_linker_data {
                construction_vmos.retain(|handle| *handle != linker_data);
            }
            let result = LaunchResult {
                job_handle: component_job.job,
                process_handle: component.process,
                address_space_handle: component.address_space,
                main_thread_handle: prepared_target.main_thread,
                service_manager_handle: runner.local,
                controller_handle: KernelHandle::none(),
                events_handle: KernelHandle::none(),
                runtime_linker_data: prepared_target.runtime_linker_data,
                native_host: Some(NativeHostHandles {
                    job: host.job_handle,
                    process: host.process_handle,
                    address_space: host.address_space_handle,
                    thread: host.main_thread_handle,
                }),
            };
            let resolved_url = format!(
                "bexos-pkg://{}#{}",
                request.manifest.package_name, request.process.name
            );
            start_component_protocol(
                kernel,
                result,
                &resolved_url,
                &request.process.runner,
                &prepared.type_url,
                &prepared.program,
                prepared.service,
                prepared.migratable,
                &directories,
                component_image,
            )
        })();
        if launched.is_err() {
            for job in construction_jobs {
                let _ = kernel.terminate_job(job, -1);
            }
            for vmo in construction_vmos {
                let _ = kernel.release_vmo(vmo);
            }
            close_all(kernel, &construction_handles);
        }
        launched
    }
}

fn directory_dependency(
    dependency: &PackageDirectoryDependency,
) -> kernel::ResolvedDependencyHandle<'_> {
    kernel::ResolvedDependencyHandle {
        package_name: dependency.package_name.as_str(),
        mount_alias: dependency.mount_alias.as_str(),
        export_name: dependency.export_name.as_str(),
        export_path: dependency.export_path.as_str(),
        symbol_prefix: dependency.symbol_prefix.as_str(),
        soname: dependency.soname.as_str(),
        abi_version: dependency.abi_version,
        kind: dependency.kind,
        direct_dependencies: &dependency.direct_dependencies,
        directory: dependency.directory,
    }
}

/// Builds the bounded dependency table used by the native host before a
/// filesystem directory service exists. The immutable images themselves are
/// supplied in the parallel NativeRunnerHost dependency VMO vector.
fn bootfs_dependency_metadata<R: PackageImageResolver>(
    resolver: &R,
    roots: &[crate::manifest::LibraryDependency],
) -> Result<Vec<PackageDirectoryDependency>, LaunchError> {
    fn visit<R: PackageImageResolver>(
        resolver: &R,
        package_name: &str,
        abi_version: u32,
        mount_alias: Option<&str>,
        dependencies: &mut Vec<PackageDirectoryDependency>,
    ) -> Result<(), LaunchError> {
        if dependencies.iter().any(|dependency| {
            dependency.package_name == package_name && dependency.abi_version == abi_version
        }) {
            return Ok(());
        }
        if dependencies.len() >= 64 {
            return Err(LaunchError::PackageImage(PackageImageError::AccessDenied));
        }
        let library = resolver
            .resolve_library(package_name, abi_version)
            .map_err(LaunchError::PackageImage)?;
        let direct_dependencies = library.direct_dependencies.to_vec();
        dependencies.push(PackageDirectoryDependency {
            package_name: library.package_name.to_string(),
            mount_alias: mount_alias.unwrap_or(library.package_name).to_string(),
            export_name: library.export_name.to_string(),
            // The native host reads the parallel VMO for BootFS dependencies.
            export_path: String::new(),
            symbol_prefix: library.symbol_prefix.to_string(),
            soname: library.soname.to_string(),
            abi_version: library.abi_version,
            kind: library.kind,
            direct_dependencies: direct_dependencies.clone(),
            directory: KernelHandle::none(),
        });
        for dependency in direct_dependencies {
            visit(
                resolver,
                &dependency.package_name,
                dependency.abi_version,
                None,
                dependencies,
            )?;
        }
        Ok(())
    }

    let mut dependencies = Vec::new();
    for root in roots {
        visit(
            resolver,
            &root.package_name,
            root.abi_version,
            root.mount_alias.as_deref(),
            &mut dependencies,
        )?;
    }
    Ok(dependencies)
}

fn component_payload_path(process: &Process) -> Option<&str> {
    match process.runner_options.as_ref() {
        Some(crate::manifest::ProcessRunnerOptions::Wasm(options)) => Some(&options.path),
        _ => None,
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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn stop_deadline_is_exactly_five_seconds_and_saturates() {
        assert_eq!(component_stop_deadline(7), 5_000_000_007);
        assert_eq!(component_stop_deadline(u64::MAX - 1), u64::MAX);
    }

    #[test]
    fn stop_escalation_waits_for_deadline_then_terminates_both_jobs() {
        let mut kernel = FakeKernelOps::new();
        let component = KernelHandle { raw: 41 };
        let host = KernelHandle { raw: 42 };
        assert_eq!(
            enforce_stop_deadline(&mut kernel, 99, 100, component, host, -15),
            Ok(false)
        );
        assert!(kernel.operations.is_empty());

        assert_eq!(
            enforce_stop_deadline(&mut kernel, 100, 100, component, host, -15),
            Ok(true)
        );
        assert_eq!(
            kernel.operations,
            vec![
                KernelOperation::TerminateJob {
                    job: component,
                    exit_code: -15,
                },
                KernelOperation::TerminateJob {
                    job: host,
                    exit_code: -15,
                },
            ]
        );
    }

    #[test]
    fn peer_loss_terminates_the_component_failure_boundary() {
        let mut kernel = FakeKernelOps::new();
        let component = KernelHandle { raw: 71 };
        let host = KernelHandle { raw: 72 };
        assert_eq!(
            terminate_runner_boundary(&mut kernel, component, host, -1),
            Ok(())
        );
        assert_eq!(
            kernel.operations,
            vec![
                KernelOperation::TerminateJob {
                    job: component,
                    exit_code: -1,
                },
                KernelOperation::TerminateJob {
                    job: host,
                    exit_code: -1,
                },
            ]
        );
    }
}
