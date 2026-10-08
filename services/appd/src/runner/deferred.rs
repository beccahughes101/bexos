//! Use the normal runner's policy and mapping path, but start only after the
//! kernel has quarantined the candidate.
use super::{
    kernel::{
        CreatedChannel, CreatedJob, CreatedProcess, CreatedResourceGroup, CreatedVmar,
        NativeRunnerPrepareRequest, NativeRunnerPrepared,
    },
    *,
};

pub struct Deferred<'a, K: KernelOps> {
    handles: alloc::vec::Vec<u64>,
    construction_handles: alloc::vec::Vec<KernelHandle>,
    constructed_vmars: alloc::vec::Vec<KernelHandle>,
    vmos: alloc::vec::Vec<u64>,
    process: u64,
    started: bool,
    migration: Option<MigrationPlan>,
    migration_begun: bool,
    prepared_thread: Option<KernelHandle>,
    pub kernel: &'a mut K,
    start: Option<(
        KernelHandle,
        KernelHandle,
        u64,
        u64,
        u64,
        Option<KernelHandle>,
    )>,
}

#[derive(Clone, Copy)]
struct MigrationPlan {
    source: u64,
    generation: u64,
    preparation_timeout_ms: u32,
    cutover_timeout_ms: u32,
}

impl<'a, K: KernelOps> Deferred<'a, K> {
    pub fn new(kernel: &'a mut K) -> Self {
        Self {
            kernel,
            start: None,
            handles: alloc::vec::Vec::new(),
            construction_handles: alloc::vec::Vec::new(),
            constructed_vmars: alloc::vec::Vec::new(),
            vmos: alloc::vec::Vec::new(),
            process: 0,
            started: false,
            migration: None,
            migration_begun: false,
            prepared_thread: None,
        }
    }

    pub fn new_migration(
        kernel: &'a mut K,
        source: u64,
        generation: u64,
        preparation_timeout_ms: u32,
        cutover_timeout_ms: u32,
    ) -> Self {
        let mut deferred = Self::new(kernel);
        deferred.migration = Some(MigrationPlan {
            source,
            generation,
            preparation_timeout_ms,
            cutover_timeout_ms,
        });
        deferred
    }

    pub fn start(mut self) -> Result<KernelHandle, KernelError> {
        if self.migration.is_some() {
            let thread = self.prepared_thread.ok_or(KernelError::InvalidArgs)?;
            self.started = true;
            return Ok(thread);
        }
        let (p, s, e, stack, tp, arg) = self.start.ok_or(KernelError::InvalidArgs)?;
        let thread = self
            .kernel
            .start_thread_in_process(p, s, e, stack, tp, arg)?;
        self.started = true;
        Ok(thread)
    }
}
impl<K: KernelOps> KernelOps for Deferred<'_, K> {
    fn send_runner_startup(
        &mut self,
        channel: KernelHandle,
        bytes: &[u8],
        module: KernelHandle,
    ) -> Result<(), KernelError> {
        self.kernel.send_runner_startup(channel, bytes, module)?;
        self.vmos.retain(|h| *h != module.raw);
        Ok(())
    }

    fn send_runner_startup_handles(
        &mut self,
        channel: KernelHandle,
        bytes: &[u8],
        modules: &[KernelHandle],
    ) -> Result<(), KernelError> {
        self.kernel
            .send_runner_startup_handles(channel, bytes, modules)?;
        for module in modules {
            self.vmos.retain(|h| *h != module.raw);
        }
        Ok(())
    }

    fn open_resource_group(&mut self, name: &str) -> Result<CreatedResourceGroup, KernelError> {
        let group = self.kernel.open_resource_group(name)?;
        self.handles.push(group.handle.raw);
        Ok(group)
    }

    fn create_resource_group_v2(
        &mut self,
        name: &str,
        parent_group: KernelHandle,
        limits: super::kernel::ResourceGroupLimits,
    ) -> Result<CreatedResourceGroup, KernelError> {
        let group = self
            .kernel
            .create_resource_group_v2(name, parent_group, limits)?;
        self.handles.push(group.handle.raw);
        Ok(group)
    }

    fn create_resource_group(
        &mut self,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<CreatedResourceGroup, KernelError> {
        let group = self
            .kernel
            .create_resource_group(name, cpu_shares, memory_limit_pages)?;
        self.handles.push(group.handle.raw);
        Ok(group)
    }

    fn release_vmo(&mut self, vmo: KernelHandle) -> Result<(), KernelError> {
        self.kernel.release_vmo(vmo)?;
        self.vmos.retain(|h| *h != vmo.raw);
        Ok(())
    }
    fn create_process(
        &mut self,
        n: &str,
        g: u32,
        p: &str,
        h: HardwareAccessTier,
        realtime_scheduling: bool,
    ) -> Result<CreatedProcess, KernelError> {
        let p = self
            .kernel
            .create_process(n, g, p, h, realtime_scheduling)?;
        self.process = p.process.raw;
        self.handles.extend([p.process.raw, p.address_space.raw]);
        self.construction_handles.push(p.root_vmar);
        Ok(p)
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
        let job = self.kernel.create_component_job(
            name,
            resource_group_id,
            package_id,
            hardware_access,
            realtime_scheduling,
            max_processes,
        )?;
        self.handles.push(job.job.raw);
        Ok(job)
    }
    fn create_process_in_job(
        &mut self,
        job: KernelHandle,
        name: &str,
    ) -> Result<CreatedProcess, KernelError> {
        let process = self.kernel.create_process_in_job(job, name)?;
        if self.process == 0 {
            self.process = process.process.raw;
            if let Some(migration) = self.migration {
                bexos_userspace::migration::begin(
                    migration.source,
                    self.process,
                    migration.generation,
                    migration.preparation_timeout_ms,
                    migration.cutover_timeout_ms,
                )
                .map_err(|_| KernelError::InvalidArgs)?;
                self.migration_begun = true;
            }
        }
        self.handles
            .extend([process.process.raw, process.address_space.raw]);
        self.construction_handles.push(process.root_vmar);
        Ok(process)
    }
    fn create_vmo(&mut self, size: u64, flags: u32) -> Result<KernelHandle, KernelError> {
        let h = self.kernel.create_vmo(size, flags)?;
        self.vmos.push(h.raw);
        Ok(h)
    }
    fn create_vmo_from_bytes(&mut self, bytes: &[u8]) -> Result<KernelHandle, KernelError> {
        let h = self.kernel.create_vmo_from_bytes(bytes)?;
        self.vmos.push(h.raw);
        Ok(h)
    }
    fn map_in_vm_space(
        &mut self,
        s: KernelHandle,
        h: KernelHandle,
        off: u64,
        size: u64,
        va: u64,
        rights: u32,
    ) -> Result<u64, KernelError> {
        self.kernel.map_in_vm_space(s, h, off, size, va, rights)
    }
    fn create_sub_vmar(
        &mut self,
        parent_vmar: KernelHandle,
        offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<CreatedVmar, KernelError> {
        let vmar = self
            .kernel
            .create_sub_vmar(parent_vmar, offset, size_bytes, flags)?;
        self.construction_handles.push(vmar.vmar);
        self.constructed_vmars.push(vmar.vmar);
        Ok(vmar)
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
        self.kernel
            .map_vmo_in_vmar(vmar, vmo, vmo_offset, vmar_offset, size_bytes, flags)
    }
    fn unmap_in_vmar(
        &mut self,
        vmar: KernelHandle,
        vaddr: u64,
        size_bytes: u64,
    ) -> Result<(), KernelError> {
        self.kernel.unmap_in_vmar(vmar, vaddr, size_bytes)
    }
    fn destroy_vmar(&mut self, vmar: KernelHandle) -> Result<(), KernelError> {
        self.kernel.destroy_vmar(vmar)?;
        self.constructed_vmars.retain(|h| *h != vmar);
        Ok(())
    }
    fn close_handle(&mut self, handle: KernelHandle) -> Result<(), KernelError> {
        self.kernel.close_handle(handle)?;
        // The normal ELF runner closes construction handles before deferred
        // thread startup. Forget them immediately: their slots may be reused
        // for the thread or another live capability before this guard drops.
        self.construction_handles.retain(|h| *h != handle);
        self.constructed_vmars.retain(|h| *h != handle);
        self.handles.retain(|h| *h != handle.raw);
        self.vmos.retain(|h| *h != handle.raw);
        if self.process == handle.raw {
            self.process = 0;
        }
        Ok(())
    }
    fn create_channel(&mut self) -> Result<CreatedChannel, KernelError> {
        let c = self.kernel.create_channel()?;
        self.handles.extend([c.local.raw, c.remote.raw]);
        Ok(c)
    }

    fn duplicate_handle(
        &mut self,
        handle: KernelHandle,
        rights: u32,
    ) -> Result<KernelHandle, KernelError> {
        let duplicate = self.kernel.duplicate_handle(handle, rights)?;
        self.handles.push(duplicate.raw);
        Ok(duplicate)
    }

    fn prepare_native_runner(
        &mut self,
        host: KernelHandle,
        request: &NativeRunnerPrepareRequest<'_>,
    ) -> Result<NativeRunnerPrepared, KernelError> {
        let prepared = self.kernel.prepare_native_runner(host, request)?;
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
        .chain(
            request
                .dependencies
                .iter()
                .map(|dependency| dependency.directory),
        )
        .chain(request.dependency_images.iter().copied())
        {
            self.handles.retain(|candidate| *candidate != handle.raw);
            self.construction_handles
                .retain(|candidate| *candidate != handle);
            self.vmos.retain(|candidate| *candidate != handle.raw);
        }
        self.handles.push(prepared.main_thread.raw);
        if let Some((linker_data, _)) = prepared.runtime_linker_data {
            self.vmos.push(linker_data.raw);
        }
        self.prepared_thread = Some(prepared.main_thread);
        Ok(prepared)
    }

    fn send_component_start(
        &mut self,
        channel: KernelHandle,
        request: &ComponentStartRequest<'_>,
    ) -> Result<(), KernelError> {
        self.kernel.send_component_start(channel, request)?;
        for handle in [
            Some(request.startup),
            Some(request.job),
            Some(request.controller),
            Some(request.events),
            request.package_dir,
            request.package_image,
        ]
        .into_iter()
        .flatten()
        .chain(
            request
                .dependencies
                .iter()
                .map(|dependency| dependency.directory),
        ) {
            self.handles.retain(|candidate| *candidate != handle.raw);
            self.construction_handles
                .retain(|candidate| *candidate != handle);
            self.vmos.retain(|candidate| *candidate != handle.raw);
        }
        Ok(())
    }

    fn start_thread_in_process(
        &mut self,
        p: KernelHandle,
        s: KernelHandle,
        e: u64,
        stack: u64,
        tp: u64,
        arg: Option<KernelHandle>,
    ) -> Result<KernelHandle, KernelError> {
        if self.migration.is_some() {
            let thread = self
                .kernel
                .start_thread_in_process(p, s, e, stack, tp, arg)?;
            self.handles.push(thread.raw);
            return Ok(thread);
        }
        if self.start.is_some() {
            return Err(KernelError::InvalidArgs);
        }
        self.start = Some((p, s, e, stack, tp, arg));
        Ok(KernelHandle::none())
    }
}

impl<K: KernelOps> Drop for Deferred<'_, K> {
    fn drop(&mut self) {
        if !self.started {
            if self.migration_begun {
                let _ = bexos_userspace::migration::abort();
            }
            if self.process != 0 {
                let _ = bexos_userspace::migration::discard_candidate(self.process);
            }
            for h in self.constructed_vmars.iter().rev() {
                let _ = self.kernel.destroy_vmar(*h);
            }
            for h in &self.handles {
                let _ = bexos_userspace::Memory::close(*h);
            }
        }
        for h in &self.construction_handles {
            let _ = self.kernel.close_handle(*h);
        }
        // Mappings retain the backing references; the coordinator must not keep
        // private BSS/stack VMOs alive after the old process is reclaimed.
        for h in &self.vmos {
            let _ = bexos_userspace::Memory::close(*h);
        }
    }
}
