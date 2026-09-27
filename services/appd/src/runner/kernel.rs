use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use kernel_fidl::{
    ChannelControlCreateChannelRequest, ChannelControlPublicClient, FidlTransport, FidlWireError,
    HandleRef, HardwareAccess, Rights, Status, SystemPrivilegedBexosSystemPrivilegedClient,
    SystemPrivilegedCreateComponentJobRequest, SystemPrivilegedCreateProcessInJobRequest,
    SystemPrivilegedCreateProcessRequest, SystemPrivilegedCreateResourceGroupRequest,
    SystemPrivilegedCreateResourceGroupV2Request, SystemPrivilegedOpenResourceGroupRequest,
    SystemPrivilegedPublicClient, SystemPrivilegedStartDelegatedProcessRequest,
    SystemPrivilegedTerminateJobRequest, SystemPrivilegedTerminateProcessRequest,
    VirtualMemoryCreateSubVmarRequest, VirtualMemoryCreateVmoRequest,
    VirtualMemoryDestroyVmarRequest, VirtualMemoryMapInVmSpaceRequest, VirtualMemoryMapVmoRequest,
    VirtualMemoryPublicClient, VirtualMemoryUnmapVmarRequest, VmarFlags, VmoFlags,
};

use super::{PackageLibraryDependency, PackageLibraryKind};
use crate::platform_config::{ComponentRunnerProviderKind, HardwareAccessTier};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KernelHandle {
    pub raw: u64,
}

impl KernelHandle {
    pub const fn none() -> Self {
        Self { raw: 0 }
    }

    pub const fn is_none(self) -> bool {
        self.raw == 0
    }
}

impl From<HandleRef> for KernelHandle {
    fn from(handle: HandleRef) -> Self {
        Self { raw: handle.raw }
    }
}

impl From<KernelHandle> for HandleRef {
    fn from(handle: KernelHandle) -> Self {
        Self { raw: handle.raw }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatedProcess {
    pub process: KernelHandle,
    pub address_space: KernelHandle,
    pub root_vmar: KernelHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatedJob {
    pub job: KernelHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatedVmar {
    pub vmar: KernelHandle,
    pub base_address: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatedChannel {
    pub local: KernelHandle,
    pub remote: KernelHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatedResourceGroup {
    pub id: u32,
    pub handle: KernelHandle,
}

#[derive(Clone, Copy, Debug)]
pub struct ComponentStartRequest<'a> {
    pub resolved_url: &'a str,
    pub runner: &'a str,
    pub program_type_url: &'a str,
    pub program: &'a [u8],
    pub service: bool,
    pub migratable: bool,
    pub package_dir: Option<KernelHandle>,
    pub dependencies: &'a [ResolvedDependencyHandle<'a>],
    pub startup: KernelHandle,
    pub job: KernelHandle,
    pub controller: KernelHandle,
    pub events: KernelHandle,
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedDependencyHandle<'a> {
    pub package_name: &'a str,
    pub mount_alias: &'a str,
    pub export_name: &'a str,
    pub export_path: &'a str,
    pub symbol_prefix: &'a str,
    pub soname: &'a str,
    pub abi_version: u32,
    pub kind: PackageLibraryKind,
    pub direct_dependencies: &'a [PackageLibraryDependency],
    pub directory: KernelHandle,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeRunnerPrepareRequest<'a> {
    pub provider_kind: ComponentRunnerProviderKind,
    pub provider_package: &'a str,
    pub provider_path: &'a str,
    pub provider_package_dir: Option<KernelHandle>,
    pub provider_image: Option<KernelHandle>,
    pub provider_image_size: u64,
    pub dependencies: &'a [ResolvedDependencyHandle<'a>],
    pub dependency_images: &'a [KernelHandle],
    pub dependency_image_sizes: &'a [u64],
    pub target_process: KernelHandle,
    pub target_address_space: KernelHandle,
    pub target_root_vmar: KernelHandle,
    pub target_job: KernelHandle,
    pub runner: KernelHandle,
    pub events: KernelHandle,
    pub events_reply: KernelHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceGroupLimits {
    pub cpu_weight: u32,
    pub max_cpu_utilization_permille: u16,
    pub allow_realtime: bool,
    pub memory_low_watermark_bytes: u64,
    pub memory_high_watermark_bytes: u64,
    pub max_render_budget_percent: u8,
    pub max_vram_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelError {
    InvalidHandle,
    AccessDenied,
    NoMemory,
    BufferTooSmall,
    PeerClosed,
    TimedOut,
    AlreadyExists,
    InvalidArgs,
    ResourceExhausted,
    Transport,
}

pub trait KernelOps {
    /// Whether VMO handles remain valid across successive launches through
    /// this kernel connection. The guest loader uses this to share immutable
    /// relocated library segments between processes.
    fn supports_shared_library_vmos(&self) -> bool {
        false
    }

    fn open_resource_group(&mut self, name: &str) -> Result<CreatedResourceGroup, KernelError>;

    fn create_resource_group_v2(
        &mut self,
        name: &str,
        parent_group: KernelHandle,
        limits: ResourceGroupLimits,
    ) -> Result<CreatedResourceGroup, KernelError>;

    fn create_resource_group(
        &mut self,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<CreatedResourceGroup, KernelError>;

    fn create_process(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
    ) -> Result<CreatedProcess, KernelError>;

    fn create_component_job(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
        max_processes: u16,
    ) -> Result<CreatedJob, KernelError>;

    fn create_process_in_job(
        &mut self,
        job: KernelHandle,
        name: &str,
    ) -> Result<CreatedProcess, KernelError>;

    fn create_vmo(&mut self, size_bytes: u64, flags: u32) -> Result<KernelHandle, KernelError>;

    fn create_vmo_from_bytes(&mut self, bytes: &[u8]) -> Result<KernelHandle, KernelError>;

    /// Release the creator's reference after mapping private process memory.
    fn release_vmo(&mut self, _vmo: KernelHandle) -> Result<(), KernelError> {
        Ok(())
    }

    fn map_in_vm_space(
        &mut self,
        vm_space: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        size_bytes: u64,
        target_vaddr: u64,
        requested_rights: u32,
    ) -> Result<u64, KernelError>;

    fn create_sub_vmar(
        &mut self,
        parent_vmar: KernelHandle,
        offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<CreatedVmar, KernelError>;

    fn map_vmo_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        vmar_offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<u64, KernelError>;

    fn unmap_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vaddr: u64,
        size_bytes: u64,
    ) -> Result<(), KernelError>;

    fn destroy_vmar(&mut self, vmar: KernelHandle) -> Result<(), KernelError>;

    fn close_handle(&mut self, _handle: KernelHandle) -> Result<(), KernelError> {
        Ok(())
    }

    fn create_channel(&mut self) -> Result<CreatedChannel, KernelError>;

    fn duplicate_handle(
        &mut self,
        handle: KernelHandle,
        rights: u32,
    ) -> Result<KernelHandle, KernelError> {
        bexos_userspace::Memory::duplicate(handle.raw, rights)
            .map(|raw| KernelHandle { raw })
            .map_err(|_| KernelError::AccessDenied)
    }

    fn send_component_start(
        &mut self,
        channel: KernelHandle,
        request: &ComponentStartRequest<'_>,
    ) -> Result<(), KernelError> {
        use component_runner_fidl::{
            ComponentRunnerStartRequest, ComponentStartInfo, DependencyKind, DependencyReference,
            FidlEncode, HandleRef as ComponentHandleRef, ProgramMetadata, ResolvedDependency,
            WireVector,
        };
        let package_dir = request
            .package_dir
            .iter()
            .map(|handle| ComponentHandleRef { raw: handle.raw })
            .collect::<Vec<_>>();
        let direct_dependencies = request
            .dependencies
            .iter()
            .map(|dependency| {
                dependency
                    .direct_dependencies
                    .iter()
                    .map(|direct| DependencyReference {
                        package_name: direct.package_name.as_str(),
                        abi_version: direct.abi_version,
                        soname: direct.soname.as_str(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let dependencies = request
            .dependencies
            .iter()
            .zip(&direct_dependencies)
            .map(|(dependency, direct)| ResolvedDependency {
                package_name: dependency.package_name,
                mount_alias: dependency.mount_alias,
                export_name: dependency.export_name,
                export_path: dependency.export_path,
                symbol_prefix: dependency.symbol_prefix,
                soname: dependency.soname,
                abi_version: dependency.abi_version,
                kind: match dependency.kind {
                    PackageLibraryKind::Native => DependencyKind::Native,
                    PackageLibraryKind::WasmComponent => DependencyKind::WasmComponent,
                },
                direct_dependencies: WireVector::from_slice(direct),
                directory: ComponentHandleRef {
                    raw: dependency.directory.raw,
                },
            })
            .collect::<Vec<_>>();
        let mut bytes = [0u8; 131072];
        bytes[..8].copy_from_slice(&1u64.to_le_bytes());
        let mut handles = [ComponentHandleRef { raw: 0 }; 72];
        let encoded = ComponentRunnerStartRequest {
            start_info: ComponentStartInfo {
                resolved_url: request.resolved_url,
                runner: request.runner,
                program: ProgramMetadata {
                    type_url: request.program_type_url,
                    payload: request.program,
                },
                service: request.service,
                migratable: request.migratable,
                package_dir: &package_dir,
                dependencies: WireVector::from_slice(&dependencies),
                startup: ComponentHandleRef {
                    raw: request.startup.raw,
                },
                job: ComponentHandleRef {
                    raw: request.job.raw,
                },
            },
            controller: ComponentHandleRef {
                raw: request.controller.raw,
            },
            events: ComponentHandleRef {
                raw: request.events.raw,
            },
        }
        .encode(&mut bytes[8..], &mut handles)
        .map_err(|_| KernelError::Transport)?;
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        bexos_userspace::Channel(channel.raw)
            .send(&bytes[..8 + encoded.bytes], &raw_handles)
            .map_err(|_| KernelError::Transport)
    }

    fn prepare_native_runner(
        &mut self,
        _host: KernelHandle,
        _request: &NativeRunnerPrepareRequest<'_>,
    ) -> Result<KernelHandle, KernelError> {
        Err(KernelError::InvalidArgs)
    }

    fn send_runner_startup(
        &mut self,
        channel: KernelHandle,
        bytes: &[u8],
        module: KernelHandle,
    ) -> Result<(), KernelError> {
        self.send_runner_startup_handles(channel, bytes, &[module])
    }

    fn send_runner_startup_handles(
        &mut self,
        channel: KernelHandle,
        bytes: &[u8],
        modules: &[KernelHandle],
    ) -> Result<(), KernelError> {
        let handles: alloc::vec::Vec<_> = modules.iter().map(|module| module.raw).collect();
        bexos_userspace::Channel(channel.raw)
            .send(bytes, &handles)
            .map_err(|_| KernelError::InvalidArgs)
    }

    fn start_thread_in_process(
        &mut self,
        process: KernelHandle,
        vm_space: KernelHandle,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        thread_pointer_vaddr: u64,
        arg_handle: Option<KernelHandle>,
    ) -> Result<KernelHandle, KernelError>;

    fn terminate_process(
        &mut self,
        _process: KernelHandle,
        _exit_code: i32,
    ) -> Result<(), KernelError> {
        Ok(())
    }

    fn terminate_job(&mut self, _job: KernelHandle, _exit_code: i32) -> Result<(), KernelError> {
        Ok(())
    }

    fn kick_restricted_thread(&mut self, thread: KernelHandle) -> Result<(), KernelError> {
        let _ = thread;
        Err(KernelError::InvalidArgs)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelOperation {
    RunnerStartup {
        channel: KernelHandle,
        bytes: Vec<u8>,
        module: KernelHandle,
    },
    RunnerStartupHandles {
        channel: KernelHandle,
        bytes: Vec<u8>,
        modules: Vec<KernelHandle>,
    },
    ComponentStart {
        channel: KernelHandle,
        runner: String,
        program_type_url: String,
        program: Vec<u8>,
        startup: KernelHandle,
        job: KernelHandle,
        controller: KernelHandle,
        events: KernelHandle,
    },
    NativeRunnerPrepare {
        host: KernelHandle,
        provider_kind: ComponentRunnerProviderKind,
        provider_package: String,
        provider_path: String,
        target_job: KernelHandle,
        target_process: KernelHandle,
        runner: KernelHandle,
    },
    DuplicateHandle {
        handle: KernelHandle,
        rights: u32,
    },
    CreateResourceGroup {
        name: String,
        cpu_shares: u32,
        memory_limit_pages: u64,
    },
    CreateProcess {
        name: String,
        resource_group_id: u32,
        package_id: String,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
    },
    CreateComponentJob {
        name: String,
        resource_group_id: u32,
        package_id: String,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
        max_processes: u16,
    },
    CreateProcessInJob {
        job: KernelHandle,
        name: String,
    },
    CreateVmo {
        size_bytes: u64,
        flags: u32,
    },
    CreateVmoFromBytes {
        size_bytes: u64,
    },
    MapInVmSpace {
        vm_space: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        size_bytes: u64,
        target_vaddr: u64,
        requested_rights: u32,
    },
    CreateSubVmar {
        parent_vmar: KernelHandle,
        offset: u64,
        size_bytes: u64,
        flags: u32,
    },
    MapVmoInVmar {
        vmar: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        vmar_offset: u64,
        size_bytes: u64,
        flags: u32,
    },
    UnmapInVmar {
        vmar: KernelHandle,
        vaddr: u64,
        size_bytes: u64,
    },
    DestroyVmar {
        vmar: KernelHandle,
    },
    CloseHandle {
        handle: KernelHandle,
    },
    CreateChannel,
    StartThreadInProcess {
        process: KernelHandle,
        vm_space: KernelHandle,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        thread_pointer_vaddr: u64,
        arg_handle: Option<KernelHandle>,
    },
    TerminateProcess {
        process: KernelHandle,
        exit_code: i32,
    },
    TerminateJob {
        job: KernelHandle,
        exit_code: i32,
    },
    KickRestrictedThread {
        thread: KernelHandle,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FakeKernelOps {
    next_handle: u64,
    pub operations: Vec<KernelOperation>,
    fail_call: Option<usize>,
    calls: usize,
    live_handles: alloc::collections::BTreeSet<u64>,
}

impl Default for FakeKernelOps {
    fn default() -> Self {
        Self {
            next_handle: 1,
            operations: Vec::new(),
            fail_call: None,
            calls: 0,
            live_handles: alloc::collections::BTreeSet::new(),
        }
    }
}

impl FakeKernelOps {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fail one kernel operation to exercise partial process construction.
    pub fn fail_call(&mut self, call: usize) {
        self.fail_call = Some(call);
    }
    pub fn call_count(&self) -> usize {
        self.calls
    }
    pub fn is_handle_live(&self, handle: KernelHandle) -> bool {
        self.live_handles.contains(&handle.raw)
    }
    pub fn live_handle_count(&self) -> usize {
        self.live_handles.len()
    }
    fn checkpoint(&mut self) -> Result<(), KernelError> {
        self.calls += 1;
        if self.fail_call == Some(self.calls) {
            Err(KernelError::NoMemory)
        } else {
            Ok(())
        }
    }
    fn alloc(&mut self) -> KernelHandle {
        let handle = KernelHandle {
            raw: self.next_handle,
        };
        self.next_handle = self.next_handle.saturating_add(1);
        self.live_handles.insert(handle.raw);
        handle
    }
}

impl KernelOps for FakeKernelOps {
    fn prepare_native_runner(
        &mut self,
        host: KernelHandle,
        request: &NativeRunnerPrepareRequest<'_>,
    ) -> Result<KernelHandle, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::NativeRunnerPrepare {
            host,
            provider_kind: request.provider_kind,
            provider_package: request.provider_package.to_string(),
            provider_path: request.provider_path.to_string(),
            target_job: request.target_job,
            target_process: request.target_process,
            runner: request.runner,
        });
        for handle in [
            request.provider_package_dir,
            request.provider_image,
            Some(request.target_job),
            Some(request.target_process),
            Some(request.target_address_space),
            Some(request.target_root_vmar),
            Some(request.runner),
            Some(request.events),
        ]
        .into_iter()
        .flatten()
        {
            self.live_handles.remove(&handle.raw);
        }
        for dependency in request.dependencies {
            self.live_handles.remove(&dependency.directory.raw);
        }
        Ok(self.alloc())
    }

    fn duplicate_handle(
        &mut self,
        handle: KernelHandle,
        rights: u32,
    ) -> Result<KernelHandle, KernelError> {
        self.checkpoint()?;
        self.operations
            .push(KernelOperation::DuplicateHandle { handle, rights });
        Ok(self.alloc())
    }

    fn send_component_start(
        &mut self,
        channel: KernelHandle,
        request: &ComponentStartRequest<'_>,
    ) -> Result<(), KernelError> {
        self.operations.push(KernelOperation::ComponentStart {
            channel,
            runner: request.runner.to_string(),
            program_type_url: request.program_type_url.to_string(),
            program: request.program.to_vec(),
            startup: request.startup,
            job: request.job,
            controller: request.controller,
            events: request.events,
        });
        for handle in [
            &request.startup,
            &request.job,
            &request.controller,
            &request.events,
        ] {
            self.live_handles.remove(&handle.raw);
        }
        Ok(())
    }

    fn send_runner_startup(
        &mut self,
        channel: KernelHandle,
        bytes: &[u8],
        module: KernelHandle,
    ) -> Result<(), KernelError> {
        self.operations.push(KernelOperation::RunnerStartup {
            channel,
            bytes: bytes.to_vec(),
            module,
        });
        Ok(())
    }

    fn send_runner_startup_handles(
        &mut self,
        channel: KernelHandle,
        bytes: &[u8],
        modules: &[KernelHandle],
    ) -> Result<(), KernelError> {
        self.operations.push(KernelOperation::RunnerStartupHandles {
            channel,
            bytes: bytes.to_vec(),
            modules: modules.to_vec(),
        });
        Ok(())
    }

    fn open_resource_group(&mut self, name: &str) -> Result<CreatedResourceGroup, KernelError> {
        self.checkpoint()?;
        let id = match name {
            "system" => 1,
            "foreground" => 2,
            "background" => 3,
            "driver" => 4,
            _ => return Err(KernelError::InvalidArgs),
        };
        Ok(CreatedResourceGroup {
            id,
            handle: KernelHandle { raw: id as u64 },
        })
    }

    fn create_resource_group_v2(
        &mut self,
        name: &str,
        _parent_group: KernelHandle,
        limits: ResourceGroupLimits,
    ) -> Result<CreatedResourceGroup, KernelError> {
        self.checkpoint()?;
        self.create_resource_group(
            name,
            limits.cpu_weight,
            limits.memory_high_watermark_bytes / 4096,
        )
    }

    fn create_resource_group(
        &mut self,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<CreatedResourceGroup, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::CreateResourceGroup {
            name: name.to_string(),
            cpu_shares,
            memory_limit_pages,
        });
        let handle = self.alloc();
        Ok(CreatedResourceGroup {
            id: 1000 + handle.raw as u32,
            handle,
        })
    }

    fn create_process(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
    ) -> Result<CreatedProcess, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::CreateProcess {
            name: name.to_string(),
            resource_group_id,
            package_id: package_id.to_string(),
            hardware_access,
            realtime_scheduling,
        });
        Ok(CreatedProcess {
            process: self.alloc(),
            address_space: self.alloc(),
            root_vmar: self.alloc(),
        })
    }

    fn create_component_job(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
        max_processes: u16,
    ) -> Result<CreatedJob, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::CreateComponentJob {
            name: name.to_string(),
            resource_group_id,
            package_id: package_id.to_string(),
            hardware_access,
            realtime_scheduling,
            max_processes,
        });
        Ok(CreatedJob { job: self.alloc() })
    }

    fn create_process_in_job(
        &mut self,
        job: KernelHandle,
        name: &str,
    ) -> Result<CreatedProcess, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::CreateProcessInJob {
            job,
            name: name.to_string(),
        });
        Ok(CreatedProcess {
            process: self.alloc(),
            address_space: self.alloc(),
            root_vmar: self.alloc(),
        })
    }

    fn create_vmo(&mut self, size_bytes: u64, flags: u32) -> Result<KernelHandle, KernelError> {
        self.checkpoint()?;
        self.operations
            .push(KernelOperation::CreateVmo { size_bytes, flags });
        Ok(self.alloc())
    }

    fn create_vmo_from_bytes(&mut self, bytes: &[u8]) -> Result<KernelHandle, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::CreateVmoFromBytes {
            size_bytes: bytes.len() as u64,
        });
        Ok(self.alloc())
    }

    fn release_vmo(&mut self, handle: KernelHandle) -> Result<(), KernelError> {
        if self.live_handles.remove(&handle.raw) {
            Ok(())
        } else {
            Err(KernelError::InvalidHandle)
        }
    }

    fn map_in_vm_space(
        &mut self,
        vm_space: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        size_bytes: u64,
        target_vaddr: u64,
        requested_rights: u32,
    ) -> Result<u64, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::MapInVmSpace {
            vm_space,
            vmo,
            vmo_offset,
            size_bytes,
            target_vaddr,
            requested_rights,
        });
        Ok(target_vaddr)
    }

    fn create_sub_vmar(
        &mut self,
        parent_vmar: KernelHandle,
        offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<CreatedVmar, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::CreateSubVmar {
            parent_vmar,
            offset,
            size_bytes,
            flags,
        });
        Ok(CreatedVmar {
            vmar: self.alloc(),
            base_address: offset,
        })
    }

    fn map_vmo_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        vmar_offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<u64, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::MapVmoInVmar {
            vmar,
            vmo,
            vmo_offset,
            vmar_offset,
            size_bytes,
            flags,
        });
        Ok(vmar_offset)
    }

    fn unmap_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vaddr: u64,
        size_bytes: u64,
    ) -> Result<(), KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::UnmapInVmar {
            vmar,
            vaddr,
            size_bytes,
        });
        Ok(())
    }

    fn destroy_vmar(&mut self, vmar: KernelHandle) -> Result<(), KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::DestroyVmar { vmar });
        Ok(())
    }

    fn close_handle(&mut self, handle: KernelHandle) -> Result<(), KernelError> {
        self.checkpoint()?;
        self.live_handles.remove(&handle.raw);
        self.operations
            .push(KernelOperation::CloseHandle { handle });
        Ok(())
    }

    fn create_channel(&mut self) -> Result<CreatedChannel, KernelError> {
        self.checkpoint()?;
        self.operations.push(KernelOperation::CreateChannel);
        Ok(CreatedChannel {
            local: self.alloc(),
            remote: self.alloc(),
        })
    }

    fn start_thread_in_process(
        &mut self,
        process: KernelHandle,
        vm_space: KernelHandle,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        thread_pointer_vaddr: u64,
        arg_handle: Option<KernelHandle>,
    ) -> Result<KernelHandle, KernelError> {
        self.checkpoint()?;
        if let Some(handle) = arg_handle {
            self.live_handles.remove(&handle.raw);
        }
        self.operations.push(KernelOperation::StartThreadInProcess {
            process,
            vm_space,
            entry_vaddr,
            stack_top_vaddr,
            thread_pointer_vaddr,
            arg_handle,
        });
        Ok(self.alloc())
    }

    fn terminate_process(
        &mut self,
        process: KernelHandle,
        exit_code: i32,
    ) -> Result<(), KernelError> {
        self.checkpoint()?;
        self.operations
            .push(KernelOperation::TerminateProcess { process, exit_code });
        Ok(())
    }

    fn terminate_job(&mut self, job: KernelHandle, exit_code: i32) -> Result<(), KernelError> {
        self.checkpoint()?;
        self.operations
            .push(KernelOperation::TerminateJob { job, exit_code });
        Ok(())
    }

    fn kick_restricted_thread(&mut self, thread: KernelHandle) -> Result<(), KernelError> {
        self.checkpoint()?;
        self.operations
            .push(KernelOperation::KickRestrictedThread { thread });
        Ok(())
    }
}

pub struct KernelFidlOps<C, V, S> {
    channel: ChannelControlPublicClient<C>,
    memory: VirtualMemoryPublicClient<V>,
    system: SystemPrivilegedBexosSystemPrivilegedClient<S>,
    public_system: SystemPrivilegedPublicClient<S>,
}

impl<C, V, S: Clone> KernelFidlOps<C, V, S> {
    pub fn new(channel_transport: C, memory_transport: V, system_transport: S) -> Self {
        Self {
            channel: ChannelControlPublicClient::new(channel_transport),
            memory: VirtualMemoryPublicClient::new(memory_transport),
            public_system: SystemPrivilegedPublicClient::new(system_transport.clone()),
            system: SystemPrivilegedBexosSystemPrivilegedClient::new(system_transport),
        }
    }
}

impl<C: FidlTransport, V: FidlTransport, S: FidlTransport + Clone> KernelOps
    for KernelFidlOps<C, V, S>
{
    fn supports_shared_library_vmos(&self) -> bool {
        true
    }

    fn kick_restricted_thread(&mut self, thread: KernelHandle) -> Result<(), KernelError> {
        bexos_userspace::restricted::kick(thread.raw).map_err(|_| KernelError::InvalidArgs)
    }

    fn open_resource_group(&mut self, name: &str) -> Result<CreatedResourceGroup, KernelError> {
        let resource_group_id = match name {
            "system" => 1,
            "foreground" => 2,
            "background" => 3,
            "driver" => 4,
            _ => return Err(KernelError::InvalidArgs),
        };
        let mut request_bytes = [0; 16];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response = self.system.open_resource_group(
            &SystemPrivilegedOpenResourceGroupRequest { resource_group_id },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedResourceGroup {
            id: resource_group_id,
            handle: response.group_handle.into(),
        })
    }

    fn create_resource_group_v2(
        &mut self,
        name: &str,
        parent_group: KernelHandle,
        limits: ResourceGroupLimits,
    ) -> Result<CreatedResourceGroup, KernelError> {
        let mut request_bytes = [0; 128];
        let mut response_bytes = [0; 32];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.system.create_resource_group_v2(
            &SystemPrivilegedCreateResourceGroupV2Request {
                name,
                parent_group: parent_group.into(),
                cpu_shares: limits.cpu_weight,
                max_cpu_utilization_permille: limits.max_cpu_utilization_permille,
                allow_realtime: limits.allow_realtime,
                memory_low_watermark_bytes: limits.memory_low_watermark_bytes,
                memory_high_watermark_bytes: limits.memory_high_watermark_bytes,
                max_render_budget_percent: limits.max_render_budget_percent,
                max_vram_bytes: limits.max_vram_bytes,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedResourceGroup {
            id: response.resource_group_id,
            handle: response.group_handle.into(),
        })
    }

    fn release_vmo(&mut self, vmo: KernelHandle) -> Result<(), KernelError> {
        bexos_userspace::Memory::close(vmo.raw).map_err(|_| KernelError::InvalidHandle)
    }

    fn create_resource_group(
        &mut self,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<CreatedResourceGroup, KernelError> {
        let mut request_bytes = [0; 64];
        let mut response_bytes = [0; 32];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.system.create_resource_group(
            &SystemPrivilegedCreateResourceGroupRequest {
                name,
                cpu_shares,
                memory_limit_pages,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedResourceGroup {
            id: response.resource_group_id,
            handle: response.group_handle.into(),
        })
    }

    fn create_process(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
    ) -> Result<CreatedProcess, KernelError> {
        let mut request_bytes = [0; 256];
        let mut response_bytes = [0; 32];
        let mut request_handles = [HandleRef { raw: 0 }; 4];
        let mut response_handles = [HandleRef { raw: 0 }; 4];
        let response = self.system.create_process(
            &SystemPrivilegedCreateProcessRequest {
                name,
                resource_group_id,
                package_id,
                hardware_access: to_fidl_hardware_access(hardware_access),
                realtime_scheduling,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedProcess {
            process: response.process_handle.into(),
            address_space: response.address_space_handle.into(),
            root_vmar: response.root_vmar_handle.into(),
        })
    }

    fn create_component_job(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccessTier,
        realtime_scheduling: bool,
        max_processes: u16,
    ) -> Result<CreatedJob, KernelError> {
        let mut request_bytes = [0; 256];
        let mut response_bytes = [0; 24];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.system.create_component_job(
            &SystemPrivilegedCreateComponentJobRequest {
                name,
                resource_group_id,
                package_id,
                hardware_access: to_fidl_hardware_access(hardware_access),
                realtime_scheduling,
                max_processes,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedJob {
            job: response.job_handle.into(),
        })
    }

    fn create_process_in_job(
        &mut self,
        job: KernelHandle,
        name: &str,
    ) -> Result<CreatedProcess, KernelError> {
        let mut request_bytes = [0; 128];
        let mut response_bytes = [0; 32];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 4];
        let response = self.system.create_process_in_job(
            &SystemPrivilegedCreateProcessInJobRequest {
                job_handle: job.into(),
                name,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedProcess {
            process: response.process_handle.into(),
            address_space: response.address_space_handle.into(),
            root_vmar: response.root_vmar_handle.into(),
        })
    }

    fn create_vmo(&mut self, size_bytes: u64, flags: u32) -> Result<KernelHandle, KernelError> {
        let mut request_bytes = [0; 32];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.memory.create_vmo(
            &VirtualMemoryCreateVmoRequest {
                size_bytes,
                flags: VmoFlags(flags),
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(response.vmo.into())
    }

    fn create_vmo_from_bytes(&mut self, bytes: &[u8]) -> Result<KernelHandle, KernelError> {
        bexos_userspace::Memory::from_bytes(bytes)
            .map(|raw| KernelHandle { raw })
            .map_err(|_| KernelError::InvalidArgs)
    }

    fn map_in_vm_space(
        &mut self,
        vm_space: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        size_bytes: u64,
        target_vaddr: u64,
        requested_rights: u32,
    ) -> Result<u64, KernelError> {
        let mut request_bytes = [0; 64];
        let mut response_bytes = [0; 24];
        let mut request_handles = [HandleRef { raw: 0 }; 4];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.memory.map_in_vm_space(
            &VirtualMemoryMapInVmSpaceRequest {
                vm_space: vm_space.into(),
                vmo: vmo.into(),
                vmo_offset,
                size_bytes,
                target_vaddr,
                requested_rights: Rights(requested_rights),
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(response.mapped_vaddr)
    }

    fn create_sub_vmar(
        &mut self,
        parent_vmar: KernelHandle,
        offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<CreatedVmar, KernelError> {
        let mut request_bytes = [0; 64];
        let mut response_bytes = [0; 32];
        let mut request_handles = [HandleRef { raw: 0 }; 4];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.memory.create_sub_vmar(
            &VirtualMemoryCreateSubVmarRequest {
                parent_vmar: parent_vmar.into(),
                offset,
                size_bytes,
                flags: VmarFlags(flags),
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedVmar {
            vmar: response.sub_vmar.into(),
            base_address: response.base_address,
        })
    }

    fn map_vmo_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        vmar_offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<u64, KernelError> {
        let mut request_bytes = [0; 80];
        let mut response_bytes = [0; 24];
        let mut request_handles = [HandleRef { raw: 0 }; 4];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.memory.map_vmo(
            &VirtualMemoryMapVmoRequest {
                vmar: vmar.into(),
                vmo: vmo.into(),
                vmo_offset,
                vmar_offset,
                size_bytes,
                flags: VmarFlags(flags),
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(response.mapped_vaddr)
    }

    fn unmap_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vaddr: u64,
        size_bytes: u64,
    ) -> Result<(), KernelError> {
        let mut request_bytes = [0; 48];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response = self.memory.unmap_vmar(
            &VirtualMemoryUnmapVmarRequest {
                vmar: vmar.into(),
                vaddr,
                size_bytes,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)
    }

    fn destroy_vmar(&mut self, vmar: KernelHandle) -> Result<(), KernelError> {
        let mut request_bytes = [0; 24];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response = self.memory.destroy_vmar(
            &VirtualMemoryDestroyVmarRequest { vmar: vmar.into() },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)
    }

    fn close_handle(&mut self, handle: KernelHandle) -> Result<(), KernelError> {
        bexos_userspace::Memory::close(handle.raw).map_err(|_| KernelError::InvalidHandle)
    }

    fn create_channel(&mut self) -> Result<CreatedChannel, KernelError> {
        let mut request_bytes = [0; 8];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 4];
        let response = self.channel.create_channel(
            &ChannelControlCreateChannelRequest {},
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(CreatedChannel {
            local: response.local_endpoint.into(),
            remote: response.remote_endpoint.into(),
        })
    }

    fn prepare_native_runner(
        &mut self,
        host: KernelHandle,
        request: &NativeRunnerPrepareRequest<'_>,
    ) -> Result<KernelHandle, KernelError> {
        use component_runner_fidl::{
            DependencyKind, DependencyReference, FidlDecode as ComponentDecode,
            FidlEncode as ComponentEncode, HandleRef as ComponentHandleRef,
            NativeRunnerHostEventsOnPreparedRequest, NativeRunnerHostPrepareRequest,
            NativeRunnerPrepareInfo, NativeRunnerProviderKind, ResolvedDependencyMetadata,
            WireVector,
        };
        let direct_dependencies = request
            .dependencies
            .iter()
            .map(|dependency| {
                dependency
                    .direct_dependencies
                    .iter()
                    .map(|direct| DependencyReference {
                        package_name: direct.package_name.as_str(),
                        abi_version: direct.abi_version,
                        soname: direct.soname.as_str(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let dependencies = request
            .dependencies
            .iter()
            .zip(&direct_dependencies)
            .map(|(dependency, direct)| ResolvedDependencyMetadata {
                package_name: dependency.package_name,
                mount_alias: dependency.mount_alias,
                export_name: dependency.export_name,
                export_path: dependency.export_path,
                symbol_prefix: dependency.symbol_prefix,
                soname: dependency.soname,
                abi_version: dependency.abi_version,
                kind: match dependency.kind {
                    PackageLibraryKind::Native => DependencyKind::Native,
                    PackageLibraryKind::WasmComponent => DependencyKind::WasmComponent,
                },
                direct_dependencies: WireVector::from_slice(direct),
            })
            .collect::<Vec<_>>();
        let dependency_directories = request
            .dependencies
            .iter()
            .filter(|dependency| !dependency.directory.is_none())
            .map(|dependency| ComponentHandleRef {
                raw: dependency.directory.raw,
            })
            .collect::<Vec<_>>();
        let images = request
            .provider_image
            .iter()
            .map(|image| ComponentHandleRef { raw: image.raw })
            .collect::<Vec<_>>();
        let dependency_images = request
            .dependency_images
            .iter()
            .map(|image| ComponentHandleRef { raw: image.raw })
            .collect::<Vec<_>>();
        let mut bytes = [0u8; 131072];
        bytes[..8].copy_from_slice(&1u64.to_le_bytes());
        let mut handles = [ComponentHandleRef { raw: 0 }; 80];
        let encoded = NativeRunnerHostPrepareRequest {
            prepare_info: NativeRunnerPrepareInfo {
                provider_kind: match request.provider_kind {
                    ComponentRunnerProviderKind::DirectElf => NativeRunnerProviderKind::DirectElf,
                    ComponentRunnerProviderKind::ComponentRunner => {
                        NativeRunnerProviderKind::ComponentRunner
                    }
                    ComponentRunnerProviderKind::Unspecified => {
                        return Err(KernelError::InvalidArgs);
                    }
                },
                provider_package: request.provider_package,
                provider_path: request.provider_path,
                provider_package_dir: ComponentHandleRef {
                    raw: request
                        .provider_package_dir
                        .unwrap_or_else(KernelHandle::none)
                        .raw,
                },
                provider_image: &images,
                provider_image_size: request.provider_image_size,
                dependencies: WireVector::from_slice(&dependencies),
                dependency_directories: &dependency_directories,
                dependency_images: &dependency_images,
                dependency_image_sizes: request.dependency_image_sizes,
                target_process: ComponentHandleRef {
                    raw: request.target_process.raw,
                },
                target_address_space: ComponentHandleRef {
                    raw: request.target_address_space.raw,
                },
                target_root_vmar: ComponentHandleRef {
                    raw: request.target_root_vmar.raw,
                },
                target_job: ComponentHandleRef {
                    raw: request.target_job.raw,
                },
                runner: ComponentHandleRef {
                    raw: request.runner.raw,
                },
            },
            events: ComponentHandleRef {
                raw: request.events.raw,
            },
        }
        .encode(&mut bytes[8..], &mut handles)
        .map_err(|_| KernelError::Transport)?;
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        bexos_userspace::Channel(host.raw)
            .send(&bytes[..8 + encoded.bytes], &raw_handles)
            .map_err(|_| KernelError::Transport)?;
        let response = bexos_userspace::Channel(request.events_reply.raw)
            .recv_blocking()
            .map_err(|_| KernelError::PeerClosed)?;
        if response.bytes.len() < 8
            || u64::from_le_bytes(
                response.bytes[..8]
                    .try_into()
                    .map_err(|_| KernelError::Transport)?,
            ) != 1
        {
            return Err(KernelError::Transport);
        }
        let response_handles = response
            .handles
            .iter()
            .map(|raw| ComponentHandleRef { raw: *raw })
            .collect::<Vec<_>>();
        let prepared = NativeRunnerHostEventsOnPreparedRequest::decode(
            &response.bytes[8..],
            &response_handles,
        )
        .map_err(|_| KernelError::Transport)?;
        status_to_result(prepared.status)?;
        Ok(KernelHandle {
            raw: prepared.main_thread.raw,
        })
    }

    fn start_thread_in_process(
        &mut self,
        process: KernelHandle,
        vm_space: KernelHandle,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        thread_pointer_vaddr: u64,
        arg_handle: Option<KernelHandle>,
    ) -> Result<KernelHandle, KernelError> {
        let mut request_bytes = [0; 64];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 4];
        let mut response_handles = [HandleRef { raw: 0 }; 2];
        let response = self.public_system.start_delegated_process(
            &SystemPrivilegedStartDelegatedProcessRequest {
                process_handle: process.into(),
                address_space_handle: vm_space.into(),
                entry_vaddr,
                stack_top_vaddr,
                thread_pointer_vaddr,
                arg_handle: arg_handle.unwrap_or_else(KernelHandle::none).into(),
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)?;
        Ok(response.thread_handle.into())
    }

    fn terminate_process(
        &mut self,
        process: KernelHandle,
        exit_code: i32,
    ) -> Result<(), KernelError> {
        let mut request_bytes = [0; 32];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response = self.system.terminate_process(
            &SystemPrivilegedTerminateProcessRequest {
                process_handle: process.into(),
                exit_code,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)
    }

    fn terminate_job(&mut self, job: KernelHandle, exit_code: i32) -> Result<(), KernelError> {
        let mut request_bytes = [0; 32];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 2];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response = self.public_system.terminate_job(
            &SystemPrivilegedTerminateJobRequest {
                job_handle: job.into(),
                exit_code,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )?;
        status_to_result(response.status)
    }
}

const fn to_fidl_hardware_access(access: HardwareAccessTier) -> HardwareAccess {
    match access {
        HardwareAccessTier::None => HardwareAccess::None,
        HardwareAccessTier::Isolated => HardwareAccess::Isolated,
        HardwareAccessTier::Direct => HardwareAccess::Direct,
    }
}

impl From<FidlWireError> for KernelError {
    fn from(_: FidlWireError) -> Self {
        Self::Transport
    }
}

fn status_to_result(status: Status) -> Result<(), KernelError> {
    match status {
        Status::Ok => Ok(()),
        Status::ErrInvalidHandle => Err(KernelError::InvalidHandle),
        Status::ErrAccessDenied => Err(KernelError::AccessDenied),
        Status::ErrNoMemory => Err(KernelError::NoMemory),
        Status::ErrBufferTooSmall => Err(KernelError::BufferTooSmall),
        Status::ErrPeerClosed => Err(KernelError::PeerClosed),
        Status::ErrTimedOut => Err(KernelError::TimedOut),
        Status::ErrAlreadyExists => Err(KernelError::AlreadyExists),
        Status::ErrInvalidArgs => Err(KernelError::InvalidArgs),
        Status::ErrResourceExhausted => Err(KernelError::ResourceExhausted),
    }
}
