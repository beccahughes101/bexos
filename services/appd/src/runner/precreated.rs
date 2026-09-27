//! Kernel facade used by the fixed native host.  The host may map and start a
//! process supplied by appd, but it cannot mint a second process or alter the
//! immutable component-job policy.
use super::kernel::{
    ComponentStartRequest, CreatedChannel, CreatedJob, CreatedProcess, CreatedResourceGroup,
    CreatedVmar, KernelError, KernelHandle, KernelOps, ResourceGroupLimits,
};
use crate::HardwareAccessTier;

pub struct PrecreatedKernel<'a, K: KernelOps> {
    kernel: &'a mut K,
    job: Option<CreatedJob>,
    process: Option<CreatedProcess>,
    startup_arg: Option<KernelHandle>,
}

impl<'a, K: KernelOps> PrecreatedKernel<'a, K> {
    pub fn new(
        kernel: &'a mut K,
        job: CreatedJob,
        process: CreatedProcess,
        startup_arg: Option<KernelHandle>,
    ) -> Self {
        Self {
            kernel,
            job: Some(job),
            process: Some(process),
            startup_arg,
        }
    }
}

impl<K: KernelOps> KernelOps for PrecreatedKernel<'_, K> {
    fn supports_shared_library_vmos(&self) -> bool {
        self.kernel.supports_shared_library_vmos()
    }
    fn open_resource_group(&mut self, name: &str) -> Result<CreatedResourceGroup, KernelError> {
        self.kernel.open_resource_group(name)
    }
    fn create_resource_group_v2(
        &mut self,
        name: &str,
        parent: KernelHandle,
        limits: ResourceGroupLimits,
    ) -> Result<CreatedResourceGroup, KernelError> {
        self.kernel.create_resource_group_v2(name, parent, limits)
    }
    fn create_resource_group(
        &mut self,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<CreatedResourceGroup, KernelError> {
        self.kernel
            .create_resource_group(name, cpu_shares, memory_limit_pages)
    }
    fn create_process(
        &mut self,
        _name: &str,
        _resource_group_id: u32,
        _package_id: &str,
        _hardware_access: HardwareAccessTier,
        _realtime_scheduling: bool,
    ) -> Result<CreatedProcess, KernelError> {
        Err(KernelError::AccessDenied)
    }
    fn create_component_job(
        &mut self,
        _name: &str,
        _resource_group_id: u32,
        _package_id: &str,
        _hardware_access: HardwareAccessTier,
        _realtime_scheduling: bool,
        max_processes: u16,
    ) -> Result<CreatedJob, KernelError> {
        if max_processes != 1 {
            return Err(KernelError::AccessDenied);
        }
        self.job.take().ok_or(KernelError::AccessDenied)
    }
    fn create_process_in_job(
        &mut self,
        _job: KernelHandle,
        _name: &str,
    ) -> Result<CreatedProcess, KernelError> {
        self.process.take().ok_or(KernelError::AccessDenied)
    }
    fn create_vmo(&mut self, size: u64, flags: u32) -> Result<KernelHandle, KernelError> {
        self.kernel.create_vmo(size, flags)
    }
    fn create_vmo_from_bytes(&mut self, bytes: &[u8]) -> Result<KernelHandle, KernelError> {
        self.kernel.create_vmo_from_bytes(bytes)
    }
    fn release_vmo(&mut self, vmo: KernelHandle) -> Result<(), KernelError> {
        self.kernel.release_vmo(vmo)
    }
    fn map_in_vm_space(
        &mut self,
        space: KernelHandle,
        vmo: KernelHandle,
        offset: u64,
        size: u64,
        address: u64,
        rights: u32,
    ) -> Result<u64, KernelError> {
        self.kernel
            .map_in_vm_space(space, vmo, offset, size, address, rights)
    }
    fn create_sub_vmar(
        &mut self,
        parent: KernelHandle,
        offset: u64,
        size: u64,
        flags: u32,
    ) -> Result<CreatedVmar, KernelError> {
        self.kernel.create_sub_vmar(parent, offset, size, flags)
    }
    fn map_vmo_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vmo: KernelHandle,
        vmo_offset: u64,
        vmar_offset: u64,
        size: u64,
        flags: u32,
    ) -> Result<u64, KernelError> {
        self.kernel
            .map_vmo_in_vmar(vmar, vmo, vmo_offset, vmar_offset, size, flags)
    }
    fn unmap_in_vmar(
        &mut self,
        vmar: KernelHandle,
        address: u64,
        size: u64,
    ) -> Result<(), KernelError> {
        self.kernel.unmap_in_vmar(vmar, address, size)
    }
    fn destroy_vmar(&mut self, vmar: KernelHandle) -> Result<(), KernelError> {
        self.kernel.destroy_vmar(vmar)
    }
    fn close_handle(&mut self, handle: KernelHandle) -> Result<(), KernelError> {
        if handle.is_none() {
            Ok(())
        } else {
            self.kernel.close_handle(handle)
        }
    }
    fn create_channel(&mut self) -> Result<CreatedChannel, KernelError> {
        if let Some(remote) = self.startup_arg.take() {
            Ok(CreatedChannel {
                local: KernelHandle::none(),
                remote,
            })
        } else {
            self.kernel.create_channel()
        }
    }
    fn duplicate_handle(
        &mut self,
        handle: KernelHandle,
        rights: u32,
    ) -> Result<KernelHandle, KernelError> {
        self.kernel.duplicate_handle(handle, rights)
    }
    fn send_component_start(
        &mut self,
        channel: KernelHandle,
        request: &ComponentStartRequest<'_>,
    ) -> Result<(), KernelError> {
        self.kernel.send_component_start(channel, request)
    }
    fn send_runner_startup_handles(
        &mut self,
        channel: KernelHandle,
        bytes: &[u8],
        modules: &[KernelHandle],
    ) -> Result<(), KernelError> {
        self.kernel
            .send_runner_startup_handles(channel, bytes, modules)
    }
    fn start_thread_in_process(
        &mut self,
        process: KernelHandle,
        space: KernelHandle,
        entry: u64,
        stack: u64,
        thread_pointer: u64,
        arg: Option<KernelHandle>,
    ) -> Result<KernelHandle, KernelError> {
        self.kernel
            .start_thread_in_process(process, space, entry, stack, thread_pointer, arg)
    }
    fn terminate_process(
        &mut self,
        process: KernelHandle,
        exit_code: i32,
    ) -> Result<(), KernelError> {
        self.kernel.terminate_process(process, exit_code)
    }
    fn terminate_job(&mut self, job: KernelHandle, exit_code: i32) -> Result<(), KernelError> {
        self.kernel.terminate_job(job, exit_code)
    }
    fn kick_restricted_thread(&mut self, thread: KernelHandle) -> Result<(), KernelError> {
        self.kernel.kick_restricted_thread(thread)
    }
}
