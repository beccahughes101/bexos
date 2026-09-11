use alloc::vec::Vec;
use bexos_usb_host::xhci::CapabilityRegisters;
use bexos_userspace::{
    HardwareResourceKind, Memory, StartupHardwareResource, live_migration::Resource, log,
};

const PAGE_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareError {
    MissingMmio,
    MissingIommu,
    MapFailed,
    InvalidRegisters,
    DmaFailed,
}

#[derive(Debug)]
pub struct DmaAllocation {
    handle: u64,
    va: u64,
    device_address: u64,
    token: u64,
    size: u64,
    owned: bool,
}

impl DmaAllocation {
    fn new(domain: u64, size: u64, permissions: u32) -> Result<Self, HardwareError> {
        if size == 0 || !size.is_multiple_of(PAGE_BYTES) {
            return Err(HardwareError::DmaFailed);
        }
        let handle = Memory::create(size, 2).map_err(|_| HardwareError::DmaFailed)?;
        let va = match Memory::map(handle, size, 6) {
            Ok(va) => va,
            Err(_) => {
                let _ = Memory::close(handle);
                return Err(HardwareError::DmaFailed);
            }
        };
        let (device_address, token) = match Memory::map_dma(domain, handle, 0, size, permissions) {
            Ok(mapped) => mapped,
            Err(_) => {
                let _ = Memory::unmap(va, size);
                let _ = Memory::close(handle);
                return Err(HardwareError::DmaFailed);
            }
        };
        unsafe {
            core::ptr::write_bytes(va as *mut u8, 0, size as usize);
        }
        Ok(Self {
            handle,
            va,
            device_address,
            token,
            size,
            owned: true,
        })
    }

    pub fn adopt(
        handle: u64,
        va: u64,
        device_address: u64,
        token: u64,
        size: u64,
    ) -> Result<Self, HardwareError> {
        if handle == 0
            || va == 0
            || device_address == 0
            || token == 0
            || size == 0
            || !va.is_multiple_of(PAGE_BYTES)
            || !device_address.is_multiple_of(PAGE_BYTES)
            || !size.is_multiple_of(PAGE_BYTES)
        {
            return Err(HardwareError::DmaFailed);
        }
        Ok(Self {
            handle,
            va,
            device_address,
            token,
            size,
            owned: false,
        })
    }

    pub fn marker(device_address: u64) -> Self {
        Self {
            handle: 1,
            va: PAGE_BYTES,
            device_address,
            token: 1,
            size: PAGE_BYTES,
            owned: false,
        }
    }

    pub fn handle(&self) -> u64 {
        self.handle
    }

    pub fn va(&self) -> u64 {
        self.va
    }

    pub fn device_address(&self) -> u64 {
        self.device_address
    }

    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    fn activate(&mut self) {
        self.owned = true;
    }

    fn resources(&self, out: &mut Vec<Resource>) {
        out.push(Resource::Mapping {
            handle: self.handle,
            offset: 0,
            va: self.va,
            size: self.size,
            rights: 6,
        });
        out.push(Resource::Pin(self.token));
    }
}

impl Drop for DmaAllocation {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let _ = Memory::unmap_dma(self.token);
        let _ = Memory::unmap(self.va, self.size);
        let _ = Memory::close(self.handle);
    }
}

#[derive(Debug)]
pub struct Hardware {
    pub mmio_handle: u64,
    pub mmio: u64,
    pub mmio_size: u64,
    pub interrupt: u64,
    pub iommu_domain: u64,
    pub caps: CapabilityRegisters,
    pub generation: u64,
    pub command_ring: DmaAllocation,
    pub event_ring: DmaAllocation,
    pub scratchpad_table: DmaAllocation,
    owned: bool,
}

impl Hardware {
    pub fn connect(resources: &[StartupHardwareResource]) -> Result<Self, HardwareError> {
        let mmio = resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::Mmio)
            .ok_or(HardwareError::MissingMmio)?;
        let iommu = resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::IommuDomain)
            .ok_or(HardwareError::MissingIommu)?;
        let interrupt = resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::Interrupt)
            .map_or(0, |resource| resource.handle);
        let va = Memory::map(mmio.handle, mmio.length, 6).map_err(|_| HardwareError::MapFailed)?;
        let window = unsafe {
            core::slice::from_raw_parts(va as *const u8, mmio.length.min(PAGE_BYTES) as usize)
        };
        let Some(caps) = CapabilityRegisters::parse(window) else {
            let _ = Memory::unmap(va, mmio.length);
            return Err(HardwareError::InvalidRegisters);
        };
        let command_ring = match DmaAllocation::new(iommu.handle, PAGE_BYTES, 6) {
            Ok(allocation) => allocation,
            Err(error) => {
                let _ = Memory::unmap(va, mmio.length);
                return Err(error);
            }
        };
        let event_ring = match DmaAllocation::new(iommu.handle, PAGE_BYTES, 6) {
            Ok(allocation) => allocation,
            Err(error) => {
                let _ = Memory::unmap(va, mmio.length);
                return Err(error);
            }
        };
        let scratchpad_table = match DmaAllocation::new(
            iommu.handle,
            u64::from(caps.scratchpads.max(1)) * PAGE_BYTES,
            6,
        ) {
            Ok(allocation) => allocation,
            Err(error) => {
                let _ = Memory::unmap(va, mmio.length);
                return Err(error);
            }
        };
        log(&alloc::format!(
            "xhcid: xHCI caps slots={} ports={} scratchpads={} intrs={}\n",
            caps.max_slots,
            caps.max_ports,
            caps.scratchpads,
            caps.max_interrupters
        ));
        Ok(Self {
            mmio_handle: mmio.handle,
            mmio: va,
            mmio_size: mmio.length,
            interrupt,
            iommu_domain: iommu.handle,
            caps,
            generation: 1,
            command_ring,
            event_ring,
            scratchpad_table,
            owned: true,
        })
    }

    pub fn adopt(
        mmio_handle: u64,
        mmio: u64,
        mmio_size: u64,
        interrupt: u64,
        iommu_domain: u64,
        caps: CapabilityRegisters,
        generation: u64,
        rings: [DmaAllocation; 3],
    ) -> Self {
        let [command_ring, event_ring, scratchpad_table] = rings;
        Self {
            mmio_handle,
            mmio,
            mmio_size,
            interrupt,
            iommu_domain,
            caps,
            generation,
            command_ring,
            event_ring,
            scratchpad_table,
            owned: false,
        }
    }

    pub fn adopt_markers(
        mmio_handle: u64,
        mmio: u64,
        mmio_size: u64,
        interrupt: u64,
        iommu_domain: u64,
        caps: CapabilityRegisters,
        generation: u64,
        markers: [u64; 3],
    ) -> Self {
        Self::adopt(
            mmio_handle,
            mmio,
            mmio_size,
            interrupt,
            iommu_domain,
            caps,
            generation,
            markers.map(DmaAllocation::marker),
        )
    }

    pub fn activate(&mut self) {
        self.owned = true;
        self.command_ring.activate();
        self.event_ring.activate();
        self.scratchpad_table.activate();
    }

    pub fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![
            Resource::Handle(self.mmio_handle),
            Resource::Handle(self.iommu_domain),
            Resource::Mapping {
                handle: self.mmio_handle,
                offset: 0,
                va: self.mmio,
                size: self.mmio_size,
                rights: 6,
            },
        ];
        if self.interrupt != 0 {
            out.push(Resource::Handle(self.interrupt));
        }
        self.command_ring.resources(&mut out);
        self.event_ring.resources(&mut out);
        self.scratchpad_table.resources(&mut out);
        out
    }

    pub fn ready_to_migrate(&self) -> bool {
        self.mmio_handle != 0
            && self.iommu_domain != 0
            && self.command_ring.device_address() != 0
            && self.event_ring.device_address() != 0
            && self.scratchpad_table.device_address() != 0
    }
}

impl Drop for Hardware {
    fn drop(&mut self) {
        if self.owned && self.mmio != 0 && self.mmio_size != 0 {
            let _ = Memory::unmap(self.mmio, self.mmio_size);
        }
    }
}
