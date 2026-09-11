//! Use the normal runner's policy and mapping path, but start only after the
//! kernel has quarantined the candidate.
use super::{
    kernel::{CreatedChannel, CreatedProcess, CreatedResourceGroup, CreatedVmar},
    *,
};

pub struct Deferred<'a, K: KernelOps> {
    handles: alloc::vec::Vec<u64>,
    construction_handles: alloc::vec::Vec<KernelHandle>,
    constructed_vmars: alloc::vec::Vec<KernelHandle>,
    vmos: alloc::vec::Vec<u64>,
    process: u64,
    started: bool,
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
        }
    }
    pub fn start(mut self) -> Result<KernelHandle, KernelError> {
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
    fn start_thread_in_process(
        &mut self,
        p: KernelHandle,
        s: KernelHandle,
        e: u64,
        stack: u64,
        tp: u64,
        arg: Option<KernelHandle>,
    ) -> Result<KernelHandle, KernelError> {
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
