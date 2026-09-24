use crate::device_registry::{
    DeviceNodeState, DeviceRegistryError, DriverBinding, RegisteredDeviceNode,
};
use crate::driver_manager::DriverIndex;
use crate::manifest::{Manifest, Process};
use crate::platform_config::{DriverPolicy, HardwareAccessTier, PackageIdentity, RunnerPolicy};
use crate::runner::{
    KernelError, KernelOps, LaunchError, LaunchRequest, LaunchResult, PackageImageResolver,
    PackageTrustTier, ResourceGroupLimits, RunnerRegistry, hardware_access_for_driver,
};
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

const RESOURCE_GROUP_SYSTEM: u32 = 1;
const RESOURCE_GROUP_FOREGROUND: u32 = 2;
const RESOURCE_GROUP_BACKGROUND: u32 = 3;
const RESOURCE_GROUP_DRIVER: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupClass {
    Automatic { wave: u32 },
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessRef<'a> {
    pub manifest: &'a Manifest,
    pub process: &'a Process,
}

impl<'a> ProcessRef<'a> {
    pub fn startup_class(self) -> StartupClass {
        match self.process.wave {
            Some(wave) => StartupClass::Automatic { wave },
            None => StartupClass::Manual,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupWave<'a> {
    pub wave: u32,
    pub processes: Vec<ProcessRef<'a>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StartupPlan<'a> {
    pub waves: Vec<StartupWave<'a>>,
    pub manual: Vec<ProcessRef<'a>>,
}

impl<'a> StartupPlan<'a> {
    pub fn build(manifests: &'a [Manifest]) -> Self {
        let mut plan = Self::default();

        for manifest in manifests {
            for process in &manifest.processes {
                if process.shell_role != crate::manifest::ShellRole::None {
                    continue;
                }
                if process.service && manifest.process_has_lazy_exposures(&process.name) {
                    continue;
                }
                let process_ref = ProcessRef { manifest, process };
                match process.wave {
                    Some(wave) => plan.push_automatic(wave, process_ref),
                    None => plan.manual.push(process_ref),
                }
            }
        }

        plan.waves.sort_by_key(|wave| wave.wave);
        plan
    }

    pub fn find_manual(&self, package_name: &str, process_name: &str) -> Option<ProcessRef<'a>> {
        self.manual.iter().copied().find(|process_ref| {
            process_ref.manifest.package_name == package_name
                && process_ref.process.name == process_name
        })
    }

    fn push_automatic(&mut self, wave: u32, process_ref: ProcessRef<'a>) {
        if let Some(existing) = self.waves.iter_mut().find(|entry| entry.wave == wave) {
            existing.processes.push(process_ref);
            return;
        }

        self.waves.push(StartupWave {
            wave,
            processes: vec![process_ref],
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchedProcess<'a> {
    pub process_ref: ProcessRef<'a>,
    pub result: LaunchResult,
    pub hardware_access: HardwareAccessTier,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CreatedResourceGroupBinding {
    package_name: String,
    name: String,
    id: u32,
    handle: crate::runner::KernelHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessError {
    NotReady,
    TimedOut,
    PeerClosed,
}

pub trait ReadinessGate {
    fn wait_ready(&mut self, launched: &LaunchedProcess<'_>) -> Result<(), ReadinessError>;

    fn registered_device_nodes(&self) -> &[RegisteredDeviceNode] {
        &[]
    }

    fn begin_device_binding(
        &mut self,
        _node_id: u64,
        _package_id: String,
        _process_name: String,
        _hardware_access: HardwareAccessTier,
    ) -> Result<(), DeviceRegistryError> {
        Ok(())
    }

    fn finish_device_binding(
        &mut self,
        _node_id: u64,
        _binding: DriverBinding,
    ) -> Result<(), DeviceRegistryError> {
        Ok(())
    }

    fn fail_device_binding(
        &mut self,
        _node_id: u64,
        _package_id: String,
        _process_name: String,
    ) -> Result<(), DeviceRegistryError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImmediateReadiness;

impl ReadinessGate for ImmediateReadiness {
    fn wait_ready(&mut self, _launched: &LaunchedProcess<'_>) -> Result<(), ReadinessError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupError {
    Launch {
        package_name: String,
        process_name: String,
        source: LaunchError,
    },
    Readiness {
        package_name: String,
        process_name: String,
        source: ReadinessError,
    },
    ManualProcessNotFound {
        package_name: String,
        process_name: String,
    },
    DeviceRegistry {
        node_id: u64,
        source: DeviceRegistryError,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppdWaveOrchestrator {
    registry: RunnerRegistry,
}

impl AppdWaveOrchestrator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry(registry: RunnerRegistry) -> Self {
        Self { registry }
    }

    pub fn plan<'a>(&self, manifests: &'a [Manifest]) -> StartupPlan<'a> {
        StartupPlan::build(manifests)
    }

    pub fn launch_automatic<'a, K, R, G>(
        &self,
        manifests: &'a [Manifest],
        trust_tier: PackageTrustTier,
        _resource_group_id: u32,
        kernel: &mut K,
        resolver: &R,
        readiness: &mut G,
    ) -> Result<Vec<LaunchedProcess<'a>>, StartupError>
    where
        K: KernelOps,
        R: PackageImageResolver,
        G: ReadinessGate,
    {
        let plan = self.plan(manifests);
        let mut launched = Vec::new();
        let created_groups =
            create_manifest_resource_groups(manifests, kernel).map_err(|source| {
                StartupError::Launch {
                    package_name: String::new(),
                    process_name: String::new(),
                    source,
                }
            })?;

        for wave in plan.waves {
            let wave_start = launched.len();
            for process_ref in wave.processes {
                launched.push(self.launch_process(
                    process_ref,
                    trust_tier,
                    default_identity(process_ref.manifest, trust_tier),
                    None,
                    HardwareAccessTier::None,
                    resolve_resource_group_id(
                        process_ref,
                        default_identity(process_ref.manifest, trust_tier),
                        HardwareAccessTier::None,
                        None,
                        &created_groups,
                    )?,
                    kernel,
                    resolver,
                )?);
            }

            for launched_process in &launched[wave_start..] {
                readiness.wait_ready(launched_process).map_err(|source| {
                    StartupError::Readiness {
                        package_name: launched_process.process_ref.manifest.package_name.clone(),
                        process_name: launched_process.process_ref.process.name.clone(),
                        source,
                    }
                })?;
            }
        }

        Ok(launched)
    }

    pub fn launch_automatic_with_policy<'a, K, R, G, I>(
        &self,
        manifests: &'a [Manifest],
        runner_policy: &'a RunnerPolicy,
        driver_policy: &DriverPolicy,
        mut identity_for_manifest: I,
        _resource_group_id: u32,
        kernel: &mut K,
        resolver: &R,
        readiness: &mut G,
    ) -> Result<Vec<LaunchedProcess<'a>>, StartupError>
    where
        K: KernelOps,
        R: PackageImageResolver,
        G: ReadinessGate,
        I: FnMut(&'a Manifest) -> PackageIdentity<'a>,
    {
        let plan = self.plan(manifests);
        let driver_index = DriverIndex::build(manifests);
        let mut launched = Vec::new();
        let created_groups =
            create_manifest_resource_groups(manifests, kernel).map_err(|source| {
                StartupError::Launch {
                    package_name: String::new(),
                    process_name: String::new(),
                    source,
                }
            })?;

        for wave in plan.waves {
            let wave_start = launched.len();
            for process_ref in wave.processes {
                if !process_ref.manifest.bind_rules.is_empty() {
                    continue;
                }
                let identity = identity_for_manifest(process_ref.manifest);
                let hardware_access = if identity.is_driver {
                    hardware_access_for_driver(driver_policy.evaluate_driver(identity)).map_err(
                        |source| StartupError::Launch {
                            package_name: process_ref.manifest.package_name.clone(),
                            process_name: process_ref.process.name.clone(),
                            source,
                        },
                    )?
                } else {
                    HardwareAccessTier::None
                };
                let result = self.launch_process(
                    process_ref,
                    identity.trust_tier,
                    identity,
                    Some(runner_policy),
                    hardware_access,
                    resolve_resource_group_id(
                        process_ref,
                        identity,
                        hardware_access,
                        None,
                        &created_groups,
                    )?,
                    kernel,
                    resolver,
                );
                match result {
                    Ok(process) => launched.push(process),
                    Err(_) if optional_graphics(process_ref.manifest) => {}
                    Err(error) => return Err(error),
                }
            }

            let mut index = wave_start;
            while index < launched.len() {
                if let Err(source) = readiness.wait_ready(&launched[index]) {
                    if optional_graphics(launched[index].process_ref.manifest) {
                        let failed = launched.remove(index);
                        let _ = kernel.terminate_process(failed.result.process_handle, -8);
                        for handle in [
                            failed.result.process_handle,
                            failed.result.address_space_handle,
                            failed.result.main_thread_handle,
                            failed.result.service_manager_handle,
                        ] {
                            let _ = kernel.close_handle(handle);
                        }
                        continue;
                    }
                    return Err(StartupError::Readiness {
                        package_name: launched[index].process_ref.manifest.package_name.clone(),
                        process_name: launched[index].process_ref.process.name.clone(),
                        source,
                    });
                }
                index += 1;
            }

            self.bind_available_drivers(
                &driver_index,
                wave.wave.saturating_add(1),
                runner_policy,
                driver_policy,
                &mut identity_for_manifest,
                &created_groups,
                kernel,
                resolver,
                readiness,
                &mut launched,
            )?;
        }

        Ok(launched)
    }

    fn bind_available_drivers<'a, K, R, G, I>(
        &self,
        driver_index: &DriverIndex<'a>,
        max_wave: u32,
        runner_policy: &'a RunnerPolicy,
        driver_policy: &DriverPolicy,
        identity_for_manifest: &mut I,
        created_groups: &[CreatedResourceGroupBinding],
        kernel: &mut K,
        resolver: &R,
        readiness: &mut G,
        launched: &mut Vec<LaunchedProcess<'a>>,
    ) -> Result<(), StartupError>
    where
        K: KernelOps,
        R: PackageImageResolver,
        G: ReadinessGate,
        I: FnMut(&'a Manifest) -> PackageIdentity<'a>,
    {
        let nodes = readiness.registered_device_nodes().to_vec();
        for node in nodes {
            match node.state {
                DeviceNodeState::Unbound | DeviceNodeState::BindFailed { .. } => {}
                _ => continue,
            }
            let Some(candidate) =
                driver_index.best_match_for_registered(&node, driver_policy, |manifest| {
                    identity_for_manifest(manifest)
                })
            else {
                continue;
            };
            if candidate.process.wave.is_some_and(|wave| wave > max_wave) {
                continue;
            }

            readiness
                .begin_device_binding(
                    node.info.node_id,
                    candidate.manifest.package_name.clone(),
                    candidate.process.name.clone(),
                    candidate.hardware_access,
                )
                .map_err(|source| StartupError::DeviceRegistry {
                    node_id: node.info.node_id,
                    source,
                })?;

            let process_ref = ProcessRef {
                manifest: candidate.manifest,
                process: candidate.process,
            };
            let identity = identity_for_manifest(candidate.manifest);
            let launched_process = match self.launch_process(
                process_ref,
                identity.trust_tier,
                identity,
                Some(runner_policy),
                candidate.hardware_access,
                resolve_resource_group_id(
                    process_ref,
                    identity,
                    candidate.hardware_access,
                    None,
                    created_groups,
                )?,
                kernel,
                resolver,
            ) {
                Ok(launched_process) => launched_process,
                Err(error) => {
                    readiness
                        .fail_device_binding(
                            node.info.node_id,
                            candidate.manifest.package_name.clone(),
                            candidate.process.name.clone(),
                        )
                        .map_err(|source| StartupError::DeviceRegistry {
                            node_id: node.info.node_id,
                            source,
                        })?;
                    if optional_graphics(candidate.manifest) {
                        continue;
                    }
                    return Err(error);
                }
            };

            if let Err(source) = readiness.wait_ready(&launched_process) {
                if optional_graphics(candidate.manifest) {
                    let _ = kernel.terminate_process(launched_process.result.process_handle, -8);
                    let _ = readiness.fail_device_binding(
                        node.info.node_id,
                        candidate.manifest.package_name.clone(),
                        candidate.process.name.clone(),
                    );
                    continue;
                }
                return Err(StartupError::Readiness {
                    package_name: candidate.manifest.package_name.clone(),
                    process_name: candidate.process.name.clone(),
                    source,
                });
            }
            readiness
                .finish_device_binding(
                    node.info.node_id,
                    DriverBinding {
                        package_id: candidate.manifest.package_name.clone(),
                        process_name: candidate.process.name.clone(),
                        process_handle: Some(bexos_kernel_core::ipc::Capability {
                            object_id: launched_process.result.process_handle.raw,
                            rights: 0,
                        }),
                        manager_channel: Some(bexos_kernel_core::ipc::Capability {
                            object_id: launched_process.result.service_manager_handle.raw,
                            rights: 0,
                        }),
                        lifecycle_channel: None,
                    },
                )
                .map_err(|source| StartupError::DeviceRegistry {
                    node_id: node.info.node_id,
                    source,
                })?;
            launched.push(launched_process);
        }
        Ok(())
    }

    pub fn launch_manual<'a, K, R>(
        &self,
        manifests: &'a [Manifest],
        package_name: &str,
        process_name: &str,
        trust_tier: PackageTrustTier,
        _resource_group_id: u32,
        kernel: &mut K,
        resolver: &R,
    ) -> Result<LaunchedProcess<'a>, StartupError>
    where
        K: KernelOps,
        R: PackageImageResolver,
    {
        let plan = self.plan(manifests);
        let created_groups =
            create_manifest_resource_groups(manifests, kernel).map_err(|source| {
                StartupError::Launch {
                    package_name: package_name.to_string(),
                    process_name: process_name.to_string(),
                    source,
                }
            })?;
        let process_ref = plan
            .find_manual(package_name, process_name)
            .ok_or_else(|| StartupError::ManualProcessNotFound {
                package_name: package_name.to_string(),
                process_name: process_name.to_string(),
            })?;

        self.launch_process(
            process_ref,
            trust_tier,
            default_identity(process_ref.manifest, trust_tier),
            None,
            HardwareAccessTier::None,
            resolve_resource_group_id(
                process_ref,
                default_identity(process_ref.manifest, trust_tier),
                HardwareAccessTier::None,
                None,
                &created_groups,
            )?,
            kernel,
            resolver,
        )
    }

    fn launch_process<'a, K, R>(
        &self,
        process_ref: ProcessRef<'a>,
        trust_tier: PackageTrustTier,
        identity: PackageIdentity<'a>,
        runner_policy: Option<&'a RunnerPolicy>,
        hardware_access: HardwareAccessTier,
        resource_group_id: u32,
        kernel: &mut K,
        resolver: &R,
    ) -> Result<LaunchedProcess<'a>, StartupError>
    where
        K: KernelOps,
        R: PackageImageResolver,
    {
        let result = self
            .registry
            .launch(
                &LaunchRequest {
                    manifest: process_ref.manifest,
                    process: process_ref.process,
                    trust_tier,
                    identity,
                    runner_policy,
                    hardware_access,
                    realtime_scheduling: crate::runner::realtime_scheduling_for(
                        process_ref.manifest,
                        process_ref.process,
                        identity,
                        runner_policy,
                    ),
                    resource_group_id,
                },
                kernel,
                resolver,
            )
            .map_err(|source| StartupError::Launch {
                package_name: process_ref.manifest.package_name.clone(),
                process_name: process_ref.process.name.clone(),
                source,
            })?;

        Ok(LaunchedProcess {
            process_ref,
            result,
            hardware_access,
        })
    }
}

fn create_manifest_resource_groups<K: KernelOps>(
    manifests: &[Manifest],
    kernel: &mut K,
) -> Result<Vec<CreatedResourceGroupBinding>, LaunchError> {
    let mut groups: Vec<CreatedResourceGroupBinding> = Vec::new();
    for manifest in manifests {
        let mut pending: Vec<_> = manifest.resource_groups.iter().collect();
        while !pending.is_empty() {
            let before = pending.len();
            let mut index = 0;
            while index < pending.len() {
                let group = pending[index];
                let parent_name = group.parent.as_deref().unwrap_or("foreground");
                let parent = if let Some(id) = builtin_resource_group_id(parent_name) {
                    Some(kernel.open_resource_group(parent_name).map(|mut group| {
                        group.id = id;
                        group
                    }))
                } else {
                    groups
                        .iter()
                        .find(|existing| {
                            existing.package_name == manifest.package_name
                                && existing.name == parent_name
                        })
                        .map(|existing| {
                            Ok(crate::runner::CreatedResourceGroup {
                                id: existing.id,
                                handle: existing.handle,
                            })
                        })
                };
                let Some(parent) = parent else {
                    index += 1;
                    continue;
                };
                let parent = parent.map_err(|source| LaunchError::Kernel {
                    operation: "open_resource_group",
                    source,
                })?;
                let limits = manifest_resource_group_limits(group)?;
                let created = kernel
                    .create_resource_group_v2(&group.name, parent.handle, limits)
                    .map_err(|source| LaunchError::Kernel {
                        operation: "create_resource_group_v2",
                        source,
                    })?;
                groups.push(CreatedResourceGroupBinding {
                    package_name: manifest.package_name.clone(),
                    name: group.name.clone(),
                    id: created.id,
                    handle: created.handle,
                });
                pending.remove(index);
            }
            if pending.len() == before {
                return Err(LaunchError::Kernel {
                    operation: "validate_resource_group_parent",
                    source: KernelError::InvalidArgs,
                });
            }
        }
    }
    Ok(groups)
}

fn manifest_resource_group_limits(
    group: &crate::manifest::ResourceGroup,
) -> Result<ResourceGroupLimits, LaunchError> {
    if group.name.is_empty()
        || group.cpu.max_utilization_permille > 1000
        || group.gpu.max_render_budget_percent > 100
        || (group.memory.high_watermark_bytes != 0
            && group.memory.low_watermark_bytes > group.memory.high_watermark_bytes)
    {
        return Err(LaunchError::Kernel {
            operation: "validate_resource_group_limits",
            source: KernelError::InvalidArgs,
        });
    }
    if group.cpu_shares != 0 && group.cpu.weight != 0 && group.cpu_shares != group.cpu.weight {
        return Err(LaunchError::Kernel {
            operation: "validate_resource_group_cpu_legacy_conflict",
            source: KernelError::InvalidArgs,
        });
    }
    let legacy_high = group.memory_limit_pages.saturating_mul(4096);
    if legacy_high != 0
        && group.memory.high_watermark_bytes != 0
        && legacy_high != group.memory.high_watermark_bytes
    {
        return Err(LaunchError::Kernel {
            operation: "validate_resource_group_memory_legacy_conflict",
            source: KernelError::InvalidArgs,
        });
    }
    Ok(ResourceGroupLimits {
        cpu_weight: group.cpu.weight.max(group.cpu_shares).max(1),
        max_cpu_utilization_permille: group.cpu.max_utilization_permille as u16,
        allow_realtime: group.cpu.allow_realtime,
        memory_low_watermark_bytes: group.memory.low_watermark_bytes,
        memory_high_watermark_bytes: if group.memory.high_watermark_bytes != 0 {
            group.memory.high_watermark_bytes
        } else {
            legacy_high
        },
        max_render_budget_percent: group.gpu.max_render_budget_percent as u8,
        max_vram_bytes: group.gpu.max_vram_bytes,
    })
}

fn resolve_resource_group_id(
    process_ref: ProcessRef<'_>,
    identity: PackageIdentity<'_>,
    hardware_access: HardwareAccessTier,
    fallback_group_id: Option<u32>,
    created_groups: &[CreatedResourceGroupBinding],
) -> Result<u32, StartupError> {
    if let Some(name) = process_ref.process.resource_group.as_deref() {
        if let Some(group) = created_groups.iter().find(|group| {
            group.package_name == process_ref.manifest.package_name && group.name == name
        }) {
            return Ok(group.id);
        }
        if let Some(id) = builtin_resource_group_id(name) {
            return Ok(id);
        }
        return Err(StartupError::Launch {
            package_name: process_ref.manifest.package_name.clone(),
            process_name: process_ref.process.name.clone(),
            source: LaunchError::Kernel {
                operation: "resolve_resource_group",
                source: KernelError::InvalidArgs,
            },
        });
    }

    if let Some(id) = fallback_group_id {
        if id != 0 {
            return Ok(id);
        }
    }

    Ok(default_resource_group_id(identity, hardware_access))
}

fn builtin_resource_group_id(name: &str) -> Option<u32> {
    match name {
        "system" => Some(RESOURCE_GROUP_SYSTEM),
        "foreground" => Some(RESOURCE_GROUP_FOREGROUND),
        "background" => Some(RESOURCE_GROUP_BACKGROUND),
        "driver" => Some(RESOURCE_GROUP_DRIVER),
        _ => None,
    }
}

fn default_resource_group_id(
    identity: PackageIdentity<'_>,
    hardware_access: HardwareAccessTier,
) -> u32 {
    if identity.trust_tier == PackageTrustTier::PlatformCore {
        RESOURCE_GROUP_SYSTEM
    } else if hardware_access == HardwareAccessTier::Direct {
        RESOURCE_GROUP_DRIVER
    } else {
        RESOURCE_GROUP_FOREGROUND
    }
}

fn default_identity<'a>(
    manifest: &'a Manifest,
    trust_tier: PackageTrustTier,
) -> PackageIdentity<'a> {
    PackageIdentity {
        package_id: &manifest.package_name,
        signer: "",
        trust_tier,
        is_driver: false,
    }
}

/// Boot graphics is deliberately nonessential: a failed display must not stop disk startup.
fn optional_graphics(manifest: &Manifest) -> bool {
    matches!(
        manifest.package_name.as_str(),
        "bexos.service.splashd" | "bexos.service.scened" | "bexos.driver.display.virtio_gpu"
    )
}
