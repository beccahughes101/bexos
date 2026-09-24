use super::*;
use bexos_boot::{PAGE, USER_END, USER_START, page_round};
use bexos_time_abi::{TIME_PAGE_MAGIC, TIME_PAGE_SIZE, TIME_PAGE_VERSION, TimePageSnapshot};

// DMA mappings and physical pins have separate tables. Keep their opaque
// tokens disjoint so migration cannot resolve a DMA token as another pin.
pub(super) const DMA_TOKEN_TAG: u64 = 1 << 63;

pub(super) fn dma_token_index(token: u64) -> Result<usize> {
    if token & DMA_TOKEN_TAG == 0 {
        return Err(Status::ErrInvalidHandle);
    }
    (token & !DMA_TOKEN_TAG)
        .checked_sub(1)
        .map(|index| index as usize)
        .ok_or(Status::ErrInvalidHandle)
}

pub trait Backend {
    /// Revoke any monitor registration before a physical pin releases its VMO.
    /// Backends without a separate protected domain have nothing to revoke.
    fn revoke_shared_pin(&mut self, _token: u64) {}
    fn trace_retirement(&self, _process: usize, _elapsed_ms: [u64; 4]) {}
    fn monotonic_ms(&self) -> Option<u64> {
        None
    }
    fn monotonic_ns(&self) -> Option<u64> {
        self.monotonic_ms().and_then(|ms| ms.checked_mul(1_000_000))
    }
    fn allocate(&mut self, pages: u64) -> Result<u64>;
    fn release(&mut self, base: u64, pages: u64);
    fn new_space(&mut self) -> Result<u64>;
    fn destroy_space(&mut self, root: u64);
    fn map_page(&mut self, root: u64, va: u64, pa: u64, rights: u32, device: bool) -> Result<()>;
    fn unmap_page(&mut self, root: u64, va: u64);
    fn flush_mappings(&mut self) {}
    fn flush_page(&mut self, _asid: u16, _va: u64) {
        self.flush_mappings();
    }
    fn read(&self, pa: u64, out: &mut [u8]);
    fn write(&mut self, pa: u64, bytes: &[u8]);
    fn zero(&mut self, pa: u64, bytes: u64) {
        let zeros = [0u8; PAGE as usize];
        let mut offset = 0;
        while offset < bytes {
            let count = (bytes - offset).min(PAGE);
            self.write(pa + offset, &zeros[..count as usize]);
            offset += count;
        }
    }
    fn free_pages(&self) -> u64;
    fn reused_pages(&self) -> u64;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VmoBacking {
    Contiguous {
        base: u64,
    },
    LazyAnonymous {
        pages: Vec<Option<u64>>,
    },
    /// Host-owned PCI shared RAM. Never released to the guest page allocator.
    SharedDevice {
        base: u64,
    },
}

pub struct Vmo {
    pub size: u64,
    pub refs: usize,
    pub device: bool,
    pub bootfs: bool,
    pub backing: VmoBacking,
}
impl Vmo {
    pub fn device_mapping(&self) -> bool {
        self.device && !matches!(self.backing, VmoBacking::SharedDevice { .. })
    }
}
#[derive(Clone, Copy)]
pub struct Vmar {
    pub owner: usize,
    pub parent: Option<usize>,
    pub base: u64,
    pub size: u64,
    pub rights: u32,
    pub refs: usize,
    pub destroyed: bool,
}
#[derive(Clone, Copy)]
pub struct Mapping {
    pub vmo: usize,
    pub vmar: usize,
    pub offset: u64,
    pub va: u64,
    pub size: u64,
    pub rights: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacKeyMaterial {
    pub lo: u64,
    pub hi: u64,
}

impl PacKeyMaterial {
    pub const fn zero() -> Self {
        Self { lo: 0, hi: 0 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuDomain {
    pub owner: usize,
    pub stream_id: u64,
    pub address_width: u8,
    pub refs: usize,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaMapping {
    pub owner: usize,
    pub domain: usize,
    pub vmo: usize,
    pub offset: u64,
    pub length: u64,
    pub permissions: u32,
    pub device_address: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressSpaceSwitch {
    pub root_table_phys: u64,
    pub asid: u16,
    pub userspace_pac_key: PacKeyMaterial,
}

impl<B: Backend> Runtime<B> {
    fn ensure_zero_page(&mut self) -> Result<u64> {
        if let Some(base) = self.zero_page {
            return Ok(base);
        }
        let base = self.backend.allocate(1)?;
        self.zero_page = Some(base);
        self.changed(super::incremental::META, 0);
        Ok(base)
    }

    pub fn create_vmo(&mut self, size: u64, flags: u32) -> Result<u64> {
        if size == 0 || size > 64 * 1024 * 1024 || flags & !15 != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if flags & 2 != 0
            && (self.processes[self.current].hardware != 2
                && !self.has_authority(super::handover::AUTH_APP_MANAGER)
                || self.processes[self.current].quarantined)
        {
            return Err(Status::ErrAccessDenied);
        }
        let size = page_round(size).ok_or(Status::ErrInvalidArgs)?;
        if flags & 2 != 0 {
            let base = self.backend.allocate(size / PAGE)?;
            Ok(self.import_vmo(base, size, false, false, ALL_MEMORY))
        } else {
            self.ensure_zero_page()?;
            // A user-controlled reservation must report allocation pressure to
            // its caller instead of panicking the kernel on metadata allocation.
            let count = (size / PAGE) as usize;
            let mut pages = Vec::new();
            pages
                .try_reserve_exact(count)
                .map_err(|_| Status::ErrNoMemory)?;
            pages.resize(count, None);
            let id = self.insert_vmo(Vmo {
                size,
                refs: 0,
                device: false,
                bootfs: false,
                backing: VmoBacking::LazyAnonymous { pages },
            });
            self.changed(VMO, id);
            Ok(self.grant(self.current, Object::Vmo(id), ALL_MEMORY))
        }
    }
    pub fn import_vmo(
        &mut self,
        base: u64,
        size: u64,
        device: bool,
        bootfs: bool,
        rights: u32,
    ) -> u64 {
        let id = self.insert_vmo(Vmo {
            size,
            refs: 0,
            device,
            bootfs,
            backing: VmoBacking::Contiguous { base },
        });
        if bootfs {
            self.bootfs_pages += size / PAGE;
        }
        self.grant(self.current, Object::Vmo(id), rights)
    }
    pub fn physical_vmo(&mut self, base: u64, size: u64) -> Result<u64> {
        let p = &self.processes[self.current];
        if p.hardware != 2 || p.quarantined {
            return Err(Status::ErrAccessDenied);
        }
        let end = base.checked_add(size).ok_or(Status::ErrInvalidArgs)?;
        let allowed = match p.package.as_str() {
            "bexos.driver.pci_root"
            | "bexos.driver.network.virtio_net"
            | "bexos.driver.display.virtio_gpu"
            | "bexos.driver.input.virtio"
            | "bexos.driver.serial.virtio_console" => {
                if cfg!(bexos_arch_x86_64) {
                    base >= 0xb000_0000 && end <= 0xfec0_0000
                } else {
                    (base >= 0x3f00_0000 && end <= 0x4000_0000)
                        || (base >= 0x1000_0000 && end <= 0x3eff_0000)
                }
            }
            "bexos.driver.uart.pl011" | "bexos.driver.debugd" => {
                !cfg!(bexos_arch_x86_64) && base == 0x0900_0000 && size == PAGE
            }
            "bexos.driver.rtc.pl031" => base == 0x0901_0000 && size == PAGE,
            _ => false,
        };
        if !allowed || size == 0 || base % PAGE != 0 || size % PAGE != 0 {
            return Err(Status::ErrAccessDenied);
        }
        Ok(self.import_vmo(
            base,
            size,
            true,
            false,
            READ | WRITE | MAP | TRANSFER | DUPLICATE,
        ))
    }

    pub fn shared_device_vmo(&mut self, base: u64, size: u64) -> Result<u64> {
        // Only the granted GPU driver may map its host-visible RAM aperture
        // with normal memory attributes. MMIO register VMOs stay device memory.
        if self.processes[self.current].package != "bexos.driver.display.virtio_gpu" {
            return Err(Status::ErrAccessDenied);
        }
        let handle = self.physical_vmo(base, size)?;
        let id = self.vmo_for(handle, READ | WRITE)?;
        self.vmos[id].as_mut().unwrap().backing = VmoBacking::SharedDevice { base };
        self.changed(VMO, id);
        Ok(handle)
    }
    pub fn shared_device_vmo_idle(&self, handle: u64) -> Result<bool> {
        let p = &self.processes[self.current];
        if p.package != "bexos.driver.display.virtio_gpu" || p.hardware != 2 || p.quarantined {
            return Err(Status::ErrAccessDenied);
        }
        let id = self.vmo_for(handle, READ | WRITE)?;
        let v = self.vmos[id].as_ref().ok_or(Status::ErrInvalidHandle)?;
        if !v.device || !matches!(v.backing, VmoBacking::SharedDevice { .. }) {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(v.refs == 1)
    }

    pub fn vmo_page_phys(
        &mut self,
        id: usize,
        offset: u64,
        for_write: bool,
    ) -> Result<(u64, bool)> {
        if offset % PAGE != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let page_index = (offset / PAGE) as usize;
        let Some(vmo) = self.vmos.get(id).and_then(|v| v.as_ref()) else {
            return Err(Status::ErrInvalidHandle);
        };
        match &vmo.backing {
            VmoBacking::Contiguous { base } | VmoBacking::SharedDevice { base } => {
                Ok((*base + offset, false))
            }
            VmoBacking::LazyAnonymous { pages } => {
                let Some(page) = pages.get(page_index) else {
                    return Err(Status::ErrInvalidArgs);
                };
                if let Some(base) = page {
                    return Ok((*base, false));
                }
                if !for_write {
                    return Ok((self.ensure_zero_page()?, true));
                }
                let new_page = self.backend.allocate(1)?;
                let zeros = [0u8; PAGE as usize];
                self.backend.write(new_page, &zeros);
                let vmo = self.vmos[id].as_mut().ok_or(Status::ErrInvalidHandle)?;
                match &mut vmo.backing {
                    VmoBacking::LazyAnonymous { pages } => {
                        if let Some(slot) = pages.get_mut(page_index) {
                            *slot = Some(new_page);
                        } else {
                            self.backend.release(new_page, 1);
                            return Err(Status::ErrInvalidArgs);
                        }
                    }
                    VmoBacking::Contiguous { .. } | VmoBacking::SharedDevice { .. } => {
                        self.backend.release(new_page, 1);
                        return self.vmo_page_phys(id, offset, for_write);
                    }
                }
                self.changed(VMO, id);
                Ok((new_page, false))
            }
        }
    }
    pub fn vmo_for(&self, h: u64, rights: u32) -> Result<usize> {
        let Object::Vmo(id) = self.capability(h, rights)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        Ok(id)
    }

    pub(super) fn read_vmo_object(&self, id: usize, offset: u64, out: &mut [u8]) -> Result<()> {
        let vmo = self
            .vmos
            .get(id)
            .and_then(Option::as_ref)
            .ok_or(Status::ErrInvalidHandle)?;
        if vmo.device
            || offset
                .checked_add(out.len() as u64)
                .is_none_or(|end| end > vmo.size)
        {
            return Err(Status::ErrAccessDenied);
        }
        let mut done = 0;
        while done < out.len() {
            let position = offset + done as u64;
            let page_offset = position & !(PAGE - 1);
            let within_page = (position & (PAGE - 1)) as usize;
            let count = (out.len() - done).min(PAGE as usize - within_page);
            let (physical, zero) = self
                .vmo_page_phys_read_only(id, page_offset)
                .ok_or(Status::ErrInvalidHandle)?;
            if zero {
                out[done..done + count].fill(0);
            } else {
                self.backend
                    .read(physical + within_page as u64, &mut out[done..done + count]);
            }
            done += count;
        }
        Ok(())
    }

    pub(super) fn write_vmo_object(&mut self, id: usize, offset: u64, bytes: &[u8]) -> Result<()> {
        let vmo = self
            .vmos
            .get(id)
            .and_then(Option::as_ref)
            .ok_or(Status::ErrInvalidHandle)?;
        if vmo.device
            || offset
                .checked_add(bytes.len() as u64)
                .is_none_or(|end| end > vmo.size)
        {
            return Err(Status::ErrAccessDenied);
        }
        let mut done = 0;
        while done < bytes.len() {
            let position = offset + done as u64;
            let page_offset = position & !(PAGE - 1);
            let within_page = (position & (PAGE - 1)) as usize;
            let count = (bytes.len() - done).min(PAGE as usize - within_page);
            let (physical, _) = self.vmo_page_phys(id, page_offset, true)?;
            self.backend
                .write(physical + within_page as u64, &bytes[done..done + count]);
            done += count;
        }
        self.changed(VMO, id);
        Ok(())
    }
    pub fn insert_vmar(
        &mut self,
        owner: usize,
        parent: Option<usize>,
        base: u64,
        size: u64,
        rights: u32,
    ) -> Result<usize> {
        if size == 0 || size % PAGE != 0 || base % PAGE != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if base.checked_add(size).is_none_or(|end| end > USER_END) || base < USER_START {
            return Err(Status::ErrInvalidArgs);
        }
        if let Some(parent_id) = parent {
            let parent = self
                .vmars
                .get(parent_id)
                .and_then(|v| v.as_ref())
                .ok_or(Status::ErrInvalidHandle)?;
            if parent.owner != owner
                || parent.destroyed
                || rights & !(parent.rights | TRANSFER | ADMIN) != 0
                || !range_contains(parent.base, parent.size, base, size)
            {
                return Err(Status::ErrAccessDenied);
            }
            if self.vmars.iter().flatten().any(|v| {
                v.parent == Some(parent_id) && !v.destroyed && overlaps(v.base, v.size, base, size)
            }) {
                return Err(Status::ErrAlreadyExists);
            }
            if self.processes[owner]
                .mappings
                .iter()
                .any(|m| overlaps(m.va, m.size, base, size))
            {
                return Err(Status::ErrAlreadyExists);
            }
        }
        let id = self.vmars.len();
        self.vmars.push(Some(Vmar {
            owner,
            parent,
            base,
            size,
            rights,
            refs: 0,
            destroyed: false,
        }));
        self.changed(VMAR, id);
        Ok(id)
    }
    pub fn root_vmar(&self, space: u64) -> Result<u64> {
        let Object::Space(pid) = self.capability(space, MAP)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        Ok(self
            .handles
            .iter()
            .enumerate()
            .find_map(|(index, cap)| {
                cap.and_then(|cap| match cap.object {
                    Object::Vmar(id) if id == self.processes[pid].root_vmar => {
                        Some(index as u64 + 1)
                    }
                    _ => None,
                })
            })
            .ok_or(Status::ErrInvalidHandle)?)
    }

    pub fn root_vmar_construction_handle(&mut self, space: u64) -> Result<u64> {
        let Object::Space(pid) = self.capability(space, MAP)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let root_vmar = self.processes[pid].root_vmar;
        Ok(self.grant(
            self.current,
            Object::Vmar(root_vmar),
            READ | WRITE | EXECUTE | MAP | ADMIN | TRANSFER,
        ))
    }
    pub fn heap_vmar_handle(&self) -> Result<u64> {
        let heap = self.processes[self.current].heap_vmar;
        self.handles
            .iter()
            .enumerate()
            .find_map(|(index, cap)| {
                cap.and_then(|cap| {
                    if cap.owner == self.current && cap.object == Object::Vmar(heap) {
                        Some(index as u64 + 1)
                    } else {
                        None
                    }
                })
            })
            .ok_or(Status::ErrInvalidHandle)
    }
    pub fn create_sub_vmar(
        &mut self,
        parent: u64,
        offset: u64,
        size: u64,
        flags: u32,
    ) -> Result<(u64, u64)> {
        let Object::Vmar(parent_id) = self.capability(parent, MAP)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let parent_vmar = self.vmars[parent_id]
            .as_ref()
            .ok_or(Status::ErrInvalidHandle)?;
        let base = if flags & 0x20 != 0 {
            if offset != 0 {
                return Err(Status::ErrInvalidArgs);
            }
            self.allocate_in_vmar(parent_id, size)?
        } else {
            parent_vmar
                .base
                .checked_add(offset)
                .ok_or(Status::ErrInvalidArgs)?
        };
        let mut rights = MAP | TRANSFER | vmar_flags_to_rights(flags)?;
        if flags & 0x0000_0008 != 0 {
            rights |= ADMIN;
        }
        let id = self.insert_vmar(parent_vmar.owner, Some(parent_id), base, size, rights)?;
        Ok((self.grant(self.current, Object::Vmar(id), rights), base))
    }
    pub fn map_vmo_in_vmar(
        &mut self,
        vmar: u64,
        h: u64,
        offset: u64,
        vmar_offset: u64,
        size: u64,
        flags: u32,
    ) -> Result<u64> {
        let Object::Vmar(vmar_id) = self.capability(vmar, MAP)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let rights = vmar_flags_to_rights(flags)?;
        self.map_in_vmar(vmar_id, h, offset, vmar_offset, size, rights)
    }
    pub fn map_in_vmar(
        &mut self,
        vmar_id: usize,
        h: u64,
        offset: u64,
        size_offset: u64,
        size: u64,
        rights: u32,
    ) -> Result<u64> {
        let vmar = *self
            .vmars
            .get(vmar_id)
            .and_then(|v| v.as_ref())
            .ok_or(Status::ErrInvalidHandle)?;
        if vmar.destroyed || rights & !vmar.rights & (READ | WRITE | EXECUTE) != 0 {
            return Err(Status::ErrAccessDenied);
        }
        let mut va = if size_offset == 0 {
            self.allocate_in_vmar(vmar_id, size)?
        } else {
            vmar.base
                .checked_add(size_offset)
                .ok_or(Status::ErrInvalidArgs)?
        };
        self.map_at(vmar.owner, vmar_id, h, offset, size, &mut va, rights)
    }
    pub fn map(
        &mut self,
        space: Option<u64>,
        h: u64,
        offset: u64,
        size: u64,
        mut va: u64,
        rights: u32,
    ) -> Result<u64> {
        let pid = match space {
            Some(s) => match self.capability(s, MAP)?.object {
                Object::Space(id) => id,
                _ => return Err(Status::ErrInvalidHandle),
            },
            None => self.current,
        };
        let vmar = self.processes[pid].root_vmar;
        if va == 0 {
            va = self.allocate_in_vmar(vmar, size)?;
        }
        self.map_at(pid, vmar, h, offset, size, &mut va, rights)
    }

    fn map_at(
        &mut self,
        pid: usize,
        vmar: usize,
        h: u64,
        offset: u64,
        size: u64,
        va: &mut u64,
        rights: u32,
    ) -> Result<u64> {
        let id = self.vmo_for(h, MAP | rights)?;
        if rights & !(READ | WRITE | EXECUTE) != 0
            || rights & READ == 0
            || rights & (WRITE | EXECUTE) == WRITE | EXECUTE
            || size == 0
            || size % PAGE != 0
            || offset % PAGE != 0
        {
            return Err(Status::ErrInvalidArgs);
        }
        let vmar_record = self
            .vmars
            .get(vmar)
            .and_then(|v| v.as_ref())
            .ok_or(Status::ErrInvalidHandle)?;
        if vmar_record.owner != pid
            || vmar_record.destroyed
            || !range_contains(vmar_record.base, vmar_record.size, *va, size)
        {
            return Err(Status::ErrInvalidArgs);
        }
        if rights & !vmar_record.rights & (READ | WRITE | EXECUTE) != 0 {
            return Err(Status::ErrAccessDenied);
        }
        let v = self.vmos[id].as_ref().unwrap();
        if offset.checked_add(size).is_none_or(|n| n > v.size) {
            return Err(Status::ErrInvalidArgs);
        }
        let device = v.device_mapping();
        if v.device && (rights & EXECUTE != 0 || self.processes[pid].quarantined) {
            return Err(Status::ErrAccessDenied);
        }
        let end = (*va).checked_add(size).ok_or(Status::ErrInvalidArgs)?;
        if *va < USER_START || end > USER_END || *va % PAGE != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if self.processes[pid]
            .mappings
            .iter()
            .any(|m| *va < m.va + m.size && end > m.va)
        {
            return Err(Status::ErrAlreadyExists);
        }
        if self.vmars.iter().flatten().any(|child| {
            child.parent == Some(vmar)
                && !child.destroyed
                && overlaps(child.base, child.size, *va, size)
        }) {
            return Err(Status::ErrAlreadyExists);
        }
        let root = self.processes[pid].root;
        if self.processes[pid].exited || root == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        for off in (0..size).step_by(PAGE as usize) {
            let (pa, zero_page) = self.vmo_page_phys(id, offset + off, false)?;
            let page_rights = if zero_page { rights & !WRITE } else { rights };
            if let Err(e) = self
                .backend
                .map_page(root, *va + off, pa, page_rights, device)
            {
                for rollback in (0..off).step_by(PAGE as usize) {
                    self.backend.unmap_page(root, *va + rollback);
                }
                self.backend.flush_mappings();
                return Err(e);
            }
        }
        self.backend.flush_mappings();
        self.vmos[id].as_mut().unwrap().refs += 1;
        self.changed(VMO, id);
        self.processes[pid].mappings.push(Mapping {
            vmo: id,
            vmar,
            offset,
            va: *va,
            size,
            rights,
        });
        if *va == self.processes[pid].next_va {
            self.processes[pid].next_va = end + PAGE;
        }
        self.changed(PROCESS, pid);
        Ok(*va)
    }
    pub fn unmap(&mut self, space: Option<u64>, va: u64, size: u64) -> Result<()> {
        let pid = match space {
            Some(s) => match self.capability(s, MAP)?.object {
                Object::Space(id) => id,
                _ => return Err(Status::ErrInvalidHandle),
            },
            None => self.current,
        };
        self.unmap_matching(pid, &[], va, size)
    }
    fn unmap_matching(&mut self, pid: usize, vmars: &[usize], va: u64, size: u64) -> Result<()> {
        let i = self.processes[pid]
            .mappings
            .iter()
            .position(|m| {
                m.va == va && m.size == size && (vmars.is_empty() || vmars.contains(&m.vmar))
            })
            .ok_or(Status::ErrInvalidArgs)?;
        let map = self.processes[pid].mappings.swap_remove(i);
        if map.va.saturating_add(map.size).saturating_add(PAGE) == self.processes[pid].next_va {
            self.processes[pid].next_va = map.va;
        }
        self.changed(PROCESS, pid);
        let root = self.processes[pid].root;
        for off in (0..size).step_by(PAGE as usize) {
            self.backend.unmap_page(root, va + off);
        }
        self.backend.flush_mappings();
        self.release_vmo(map.vmo);
        Ok(())
    }
    pub fn unmap_in_vmar(&mut self, vmar: u64, va: u64, size: u64) -> Result<()> {
        let Object::Vmar(vmar_id) = self.capability(vmar, MAP)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let owner = self.vmars[vmar_id]
            .as_ref()
            .ok_or(Status::ErrInvalidHandle)?
            .owner;
        let ids = self.vmar_subtree(vmar_id);
        self.unmap_matching(owner, &ids, va, size)
    }
    pub fn destroy_vmar(&mut self, vmar: u64) -> Result<()> {
        let Object::Vmar(vmar_id) = self.capability(vmar, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if self.vmars[vmar_id]
            .as_ref()
            .is_some_and(|v| v.parent.is_none())
        {
            return Err(Status::ErrAccessDenied);
        }
        let owner = self.vmars[vmar_id]
            .as_ref()
            .ok_or(Status::ErrInvalidHandle)?
            .owner;
        let ids = self.vmar_subtree(vmar_id);
        let maps: Vec<_> = self.processes[owner]
            .mappings
            .iter()
            .filter(|m| ids.contains(&m.vmar))
            .map(|m| (m.va, m.size))
            .collect();
        for (va, size) in maps {
            self.unmap_matching(owner, &ids, va, size)?;
        }
        for id in ids {
            if let Some(v) = &mut self.vmars[id] {
                v.destroyed = true;
                self.changed(VMAR, id);
            }
        }
        let dead_handles: Vec<_> = self
            .handles
            .iter()
            .enumerate()
            .filter_map(|(index, cap)| {
                cap.and_then(|cap| match cap.object {
                    Object::Vmar(id) if self.vmars[id].as_ref().is_some_and(|v| v.destroyed) => {
                        Some((index, id))
                    }
                    _ => None,
                })
            })
            .collect();
        for (handle, id) in dead_handles {
            self.handles[handle] = None;
            self.changed(HANDLE, handle);
            self.release_vmar(id);
        }
        Ok(())
    }
    pub(super) fn release_vmar(&mut self, id: usize) {
        self.changed(VMAR, id);
        if let Some(vmar) = &mut self.vmars[id] {
            if vmar.refs > 0 {
                vmar.refs -= 1;
            }
        }
    }
    fn allocate_in_vmar(&self, vmar_id: usize, size: u64) -> Result<u64> {
        let vmar = self
            .vmars
            .get(vmar_id)
            .and_then(|v| v.as_ref())
            .ok_or(Status::ErrInvalidHandle)?;
        if size == 0 || size % PAGE != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut va = vmar.base;
        while range_contains(vmar.base, vmar.size, va, size) {
            let mut next = va;
            for mapping in &self.processes[vmar.owner].mappings {
                if overlaps(mapping.va, mapping.size, va, size) {
                    next = next.max(
                        mapping
                            .va
                            .checked_add(mapping.size)
                            .ok_or(Status::ErrNoMemory)?,
                    );
                }
            }
            for child in self.vmars.iter().flatten() {
                if child.parent == Some(vmar_id)
                    && !child.destroyed
                    && overlaps(child.base, child.size, va, size)
                {
                    next = next.max(
                        child
                            .base
                            .checked_add(child.size)
                            .ok_or(Status::ErrNoMemory)?,
                    );
                }
            }
            if next == va {
                return Ok(va);
            }
            va = page_round(next).ok_or(Status::ErrNoMemory)?;
        }
        Err(Status::ErrNoMemory)
    }
    fn vmar_subtree(&self, root: usize) -> Vec<usize> {
        let mut ids = Vec::new();
        ids.push(root);
        let mut index = 0;
        while index < ids.len() {
            let parent = ids[index];
            for (id, vmar) in self.vmars.iter().enumerate() {
                if vmar.is_some_and(|v| v.parent == Some(parent) && !v.destroyed) {
                    ids.push(id);
                }
            }
            index += 1;
        }
        ids
    }
    pub(super) fn release_vmo(&mut self, id: usize) {
        self.changed(VMO, id);
        let v = self.vmos[id].as_mut().unwrap();
        v.refs -= 1;
        if v.refs == 0 {
            let device = v.device;
            // The cached time page ID is a weak reference. The last userspace
            // mapping/handle can disappear when a short-lived command exits.
            if self.time_page_vmo == Some(id) {
                self.time_page_vmo = None;
                self.changed(super::incremental::META, 0);
            }
            if self.defer_reclamation && !device {
                self.defer_vmo_reclamation(id);
                return;
            }
            let v = self.vmos[id].take().unwrap();
            // Scrub private pages before returning them to the allocator. This
            // is especially important for committed service handovers: the
            // retired source may contain authentication tokens while an
            // aborted handover must leave the still-live source untouched.
            match v.backing {
                VmoBacking::Contiguous { base } if !v.device => {
                    self.backend.zero(base, v.size);
                    self.backend.release(base, v.size / PAGE);
                }
                VmoBacking::LazyAnonymous { pages } => {
                    let mut pages = pages.into_iter().flatten().peekable();
                    while let Some(base) = pages.next() {
                        let mut count = 1;
                        while pages.peek() == Some(&(base + count * PAGE)) {
                            pages.next();
                            count += 1;
                        }
                        // A committed heap chunk commonly owns consecutive
                        // frames. Scrub the run before releasing any of it,
                        // avoiding a cache barrier and allocator call per page.
                        self.backend.zero(base, count * PAGE);
                        self.backend.release(base, count);
                    }
                }
                _ => {}
            }
            if v.bootfs {
                self.bootfs_pages -= v.size / PAGE;
                self.reclaimed_pages += v.size / PAGE;
            }
        }
    }
    pub fn pin(&mut self, h: u64) -> Result<(u64, u64)> {
        let process = &self.processes[self.current];
        let secure_monitor = process.authority & crate::runtime::handover::AUTH_SECURE_MONITOR != 0;
        if (!secure_monitor && process.hardware != 2) || process.quarantined {
            return Err(Status::ErrAccessDenied);
        }
        if !secure_monitor && self.process_has_iommu_domain(self.current) {
            return Err(Status::ErrAccessDenied);
        }
        let id = self.vmo_for(h, READ | WRITE)?;
        let v = self.vmos[id].as_mut().unwrap();
        if v.device || v.bootfs {
            return Err(Status::ErrAccessDenied);
        }
        let base = self.materialize_contiguous_vmo(id)?;
        self.vmos[id].as_mut().unwrap().refs += 1;
        self.changed(VMO, id);
        self.pins.push(Some((self.current, id)));
        self.changed(PIN, self.pins.len() - 1);
        Ok((base, self.pins.len() as u64))
    }

    fn materialize_contiguous_vmo(&mut self, id: usize) -> Result<u64> {
        let vmo = self.vmos[id].as_ref().ok_or(Status::ErrInvalidHandle)?;
        match &vmo.backing {
            VmoBacking::Contiguous { base } => return Ok(*base),
            VmoBacking::SharedDevice { .. } => return Err(Status::ErrAccessDenied),
            VmoBacking::LazyAnonymous { pages } => {
                let new_base = self.backend.allocate(vmo.size / PAGE)?;
                for (page_index, page) in pages.iter().enumerate() {
                    let mut bytes = [0u8; PAGE as usize];
                    if let Some(source) = page {
                        self.backend.read(*source, &mut bytes);
                    }
                    self.backend
                        .write(new_base + page_index as u64 * PAGE, &bytes);
                }
                let root_updates = self.mappings_for_vmo(id);
                if let Some(vmo) = self.vmos[id].as_mut() {
                    if let VmoBacking::LazyAnonymous { pages } = core::mem::replace(
                        &mut vmo.backing,
                        VmoBacking::Contiguous { base: new_base },
                    ) {
                        for page in pages.into_iter().flatten() {
                            self.backend.release(page, 1);
                        }
                    }
                }
                for (root, va, offset, size, rights, device) in root_updates {
                    for off in (0..size).step_by(PAGE as usize) {
                        self.backend.unmap_page(root, va + off);
                        self.backend.map_page(
                            root,
                            va + off,
                            new_base + offset + off,
                            rights,
                            device,
                        )?;
                    }
                }
                self.backend.flush_mappings();
                self.changed(VMO, id);
                Ok(new_base)
            }
        }
    }

    pub fn materialize_vmo_for_kernel_slice(&mut self, id: usize) -> Result<u64> {
        let vmo = self
            .vmos
            .get(id)
            .and_then(|v| v.as_ref())
            .ok_or(Status::ErrInvalidHandle)?;
        if vmo.device {
            return Err(Status::ErrAccessDenied);
        }
        self.materialize_contiguous_vmo(id)
    }

    fn mappings_for_vmo(&self, id: usize) -> Vec<(u64, u64, u64, u64, u32, bool)> {
        let mut out = Vec::new();
        let device = self
            .vmos
            .get(id)
            .and_then(|v| v.as_ref())
            .is_some_and(Vmo::device_mapping);
        for process in &self.processes {
            for mapping in &process.mappings {
                if mapping.vmo == id {
                    out.push((
                        process.root,
                        mapping.va,
                        mapping.offset,
                        mapping.size,
                        mapping.rights,
                        device,
                    ));
                }
            }
        }
        out
    }

    pub fn create_iommu_domain(&mut self, stream_id: u64, address_width: u8) -> Result<u64> {
        let app_manager = self.has_authority(super::handover::AUTH_APP_MANAGER);
        let process = &self.processes[self.current];
        if (!app_manager && process.hardware != 2) || process.quarantined {
            return Err(Status::ErrAccessDenied);
        }
        if stream_id == 0 || !(32..=48).contains(&address_width) {
            return Err(Status::ErrInvalidArgs);
        }
        if self
            .iommu_domains
            .iter()
            .flatten()
            .any(|domain| !domain.closed && domain.stream_id == stream_id)
        {
            return Err(Status::ErrAlreadyExists);
        }
        let id = self.iommu_domains.len();
        self.iommu_domains.push(Some(IommuDomain {
            owner: self.current,
            stream_id,
            address_width,
            refs: 0,
            closed: false,
        }));
        Ok(self.grant(
            self.current,
            Object::IommuDomain(id),
            READ | WRITE | TRANSFER | DUPLICATE,
        ))
    }

    pub fn map_dma(
        &mut self,
        domain: u64,
        vmo: u64,
        offset: u64,
        length: u64,
        permissions: u32,
    ) -> Result<(u64, u64)> {
        let Object::IommuDomain(domain_id) = self.capability(domain, WRITE)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let Some(domain) = self.iommu_domains.get(domain_id).and_then(|d| *d) else {
            return Err(Status::ErrInvalidHandle);
        };
        if domain.closed {
            return Err(Status::ErrAccessDenied);
        }
        if length == 0
            || !offset.is_multiple_of(PAGE)
            || !length.is_multiple_of(PAGE)
            || permissions & (READ | WRITE) == 0
            || permissions & !(READ | WRITE) != 0
        {
            return Err(Status::ErrInvalidArgs);
        }
        // Device access must not exceed the VMO capability supplied by the caller.
        let id = self.vmo_for(vmo, permissions)?;
        let v = self.vmos[id].as_ref().ok_or(Status::ErrInvalidHandle)?;
        if v.device || v.bootfs || offset.checked_add(length).is_none_or(|end| end > v.size) {
            return Err(Status::ErrAccessDenied);
        }
        let base = self.materialize_contiguous_vmo(id)?;
        let device_address = base.checked_add(offset).ok_or(Status::ErrInvalidArgs)?;
        if device_address
            .checked_add(length)
            .is_none_or(|end| end > (1u64 << core::cmp::min(domain.address_width, 63)))
        {
            return Err(Status::ErrInvalidArgs);
        }
        if self.dma_mappings.iter().flatten().any(|mapping| {
            mapping.domain == domain_id
                && overlaps(
                    mapping.device_address,
                    mapping.length,
                    device_address,
                    length,
                )
        }) {
            return Err(Status::ErrAlreadyExists);
        }
        let v = self.vmos[id].as_mut().ok_or(Status::ErrInvalidHandle)?;
        v.refs += 1;
        self.changed(VMO, id);
        let token = self.dma_mappings.len();
        self.dma_mappings.push(Some(DmaMapping {
            owner: self.current,
            domain: domain_id,
            vmo: id,
            offset,
            length,
            permissions,
            device_address,
        }));
        self.changed(incremental::DMA_MAPPING, token);
        Ok((device_address, DMA_TOKEN_TAG | (token as u64 + 1)))
    }

    pub fn unmap_dma(&mut self, token: u64) -> Result<()> {
        let index = dma_token_index(token)?;
        let mapping = self
            .dma_mappings
            .get_mut(index)
            .and_then(|mapping| mapping.take())
            .ok_or(Status::ErrInvalidHandle)?;
        if mapping.owner != self.current {
            self.dma_mappings[index] = Some(mapping);
            return Err(Status::ErrAccessDenied);
        }
        self.changed(incremental::DMA_MAPPING, index);
        self.release_vmo(mapping.vmo);
        Ok(())
    }

    pub(super) fn release_iommu_domain(&mut self, id: usize) {
        let Some(mut domain) = self
            .iommu_domains
            .get_mut(id)
            .and_then(|domain| domain.take())
        else {
            return;
        };
        domain.refs = domain.refs.saturating_sub(1);
        self.changed(incremental::IOMMU_DOMAIN, id);
        if domain.refs > 0 {
            self.iommu_domains[id] = Some(domain);
            return;
        }
        for index in 0..self.dma_mappings.len() {
            if self.dma_mappings[index].is_some_and(|mapping| mapping.domain == id) {
                if let Some(mapping) = self.dma_mappings[index].take() {
                    self.changed(incremental::DMA_MAPPING, index);
                    self.release_vmo(mapping.vmo);
                }
            }
        }
    }

    fn process_has_iommu_domain(&self, owner: usize) -> bool {
        self.iommu_domains.iter().enumerate().any(|(id, domain)| {
            domain.is_some_and(|domain| {
                !domain.closed
                    && self
                        .handles
                        .iter()
                        .flatten()
                        .any(|cap| cap.owner == owner && cap.object == Object::IommuDomain(id))
            })
        })
    }
    pub fn unpin(&mut self, token: u64) -> Result<()> {
        let index = token.checked_sub(1).ok_or(Status::ErrInvalidHandle)? as usize;
        let (owner, id) = self
            .pins
            .get(index)
            .and_then(|p| *p)
            .ok_or(Status::ErrInvalidHandle)?;
        if owner != self.current {
            return Err(Status::ErrAccessDenied);
        }
        self.backend.revoke_shared_pin(token);
        self.pins[index] = None;
        self.changed(PIN, index);
        self.release_vmo(id);
        Ok(())
    }

    /// Resolve only the calling process's live, ordinary-RAM pin. Neither a
    /// physical address nor secure-monitor authority alone establishes memory
    /// ownership. Pin identities survive a handover and are never reused.
    pub fn secure_pin_for_range(&self, address: u64, length: u64) -> Result<u64> {
        let end = address.checked_add(length).ok_or(Status::ErrInvalidArgs)?;
        if length == 0 || (address | length) & (PAGE - 1) != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        self.secure_pin_authority()?;
        self.pins
            .iter()
            .enumerate()
            .find_map(|(index, pin)| {
                let (owner, id) = (*pin)?;
                if owner != self.current {
                    return None;
                }
                let vmo = self.vmos.get(id)?.as_ref()?;
                let VmoBacking::Contiguous { base } = vmo.backing else {
                    return None;
                };
                (!vmo.device
                    && !vmo.bootfs
                    && address >= base
                    && base.checked_add(vmo.size).is_some_and(|limit| end <= limit))
                .then_some(index as u64 + 1)
            })
            .ok_or(Status::ErrAccessDenied)
    }

    pub fn validate_secure_pin(&self, token: u64) -> Result<()> {
        self.secure_pin_authority()?;
        let index = usize::try_from(token.checked_sub(1).ok_or(Status::ErrInvalidHandle)?)
            .map_err(|_| Status::ErrInvalidHandle)?;
        let (owner, id) = self
            .pins
            .get(index)
            .and_then(|pin| *pin)
            .ok_or(Status::ErrInvalidHandle)?;
        if owner != self.current {
            return Err(Status::ErrAccessDenied);
        }
        let vmo = self
            .vmos
            .get(id)
            .and_then(Option::as_ref)
            .ok_or(Status::ErrInvalidHandle)?;
        if vmo.device || vmo.bootfs || !matches!(vmo.backing, VmoBacking::Contiguous { .. }) {
            return Err(Status::ErrAccessDenied);
        }
        Ok(())
    }

    fn secure_pin_authority(&self) -> Result<()> {
        let process = &self.processes[self.current];
        if process.quarantined || process.authority & handover::AUTH_SECURE_MONITOR == 0 {
            return Err(Status::ErrAccessDenied);
        }
        Ok(())
    }
    pub fn valid_range(&self, pid: usize, va: u64, len: u64, rights: u32) -> bool {
        if len == 0 {
            return true;
        }
        let Some(end) = va.checked_add(len) else {
            return false;
        };
        let mut pos = va;
        while pos < end {
            let Some(m) = self.processes[pid]
                .mappings
                .iter()
                .find(|m| pos >= m.va && pos < m.va + m.size && m.rights & rights == rights)
            else {
                return false;
            };
            pos = core::cmp::min(end, m.va + m.size);
        }
        true
    }
    pub fn copy_from_user(&self, va: u64, out: &mut [u8]) -> Result<()> {
        if !self.valid_range(self.current, va, out.len() as u64, READ) {
            return Err(Status::ErrAccessDenied);
        }
        let mut done = 0;
        while done < out.len() {
            let pos = va + done as u64;
            let m = self.processes[self.current]
                .mappings
                .iter()
                .find(|m| pos >= m.va && pos < m.va + m.size)
                .unwrap();
            let n = core::cmp::min(out.len() - done, (m.va + m.size - pos) as usize);
            let page_offset = (m.offset + pos - m.va) & !(PAGE - 1);
            let within_page = ((m.offset + pos - m.va) & (PAGE - 1)) as usize;
            let n = n.min(PAGE as usize - within_page);
            let (pa, zero_page) = self
                .vmo_page_phys_read_only(m.vmo, page_offset)
                .ok_or(Status::ErrInvalidHandle)?;
            if zero_page {
                out[done..done + n].fill(0);
            } else {
                self.backend
                    .read(pa + within_page as u64, &mut out[done..done + n]);
            }
            done += n;
        }
        Ok(())
    }
    pub fn copy_to_user(&mut self, va: u64, bytes: &[u8]) -> Result<()> {
        if !self.valid_range(self.current, va, bytes.len() as u64, WRITE) {
            return Err(Status::ErrAccessDenied);
        }
        let mut done = 0;
        while done < bytes.len() {
            let pos = va + done as u64;
            let m = *self.processes[self.current]
                .mappings
                .iter()
                .find(|m| pos >= m.va && pos < m.va + m.size)
                .unwrap();
            let n = core::cmp::min(bytes.len() - done, (m.va + m.size - pos) as usize);
            let page_offset = (m.offset + pos - m.va) & !(PAGE - 1);
            let within_page = ((m.offset + pos - m.va) & (PAGE - 1)) as usize;
            let n = n.min(PAGE as usize - within_page);
            let (pa, _) = self.vmo_page_phys(m.vmo, page_offset, true)?;
            self.backend
                .write(pa + within_page as u64, &bytes[done..done + n]);
            self.remap_committed_page(m, page_offset, pa)?;
            done += n;
        }
        Ok(())
    }

    fn vmo_page_phys_read_only(&self, id: usize, offset: u64) -> Option<(u64, bool)> {
        let vmo = self.vmos.get(id)?.as_ref()?;
        match &vmo.backing {
            VmoBacking::Contiguous { base } | VmoBacking::SharedDevice { base } => {
                Some((*base + offset, false))
            }
            VmoBacking::LazyAnonymous { pages } => {
                let page = pages.get((offset / PAGE) as usize).copied().flatten();
                match page {
                    Some(base) => Some((base, false)),
                    None => Some((self.zero_page?, true)),
                }
            }
        }
    }

    fn remap_committed_page(&mut self, mapping: Mapping, page_offset: u64, pa: u64) -> Result<()> {
        let device = self.vmos[mapping.vmo]
            .as_ref()
            .is_some_and(Vmo::device_mapping);
        // Anonymous VMOs are shared objects. Committing a page must replace
        // every alias of the initial zero page, including read-only mappings
        // in other processes, while preserving each mapping's permissions.
        for process in &self.processes {
            for alias in &process.mappings {
                if alias.vmo != mapping.vmo
                    || page_offset < alias.offset
                    || page_offset >= alias.offset + alias.size
                {
                    continue;
                }
                let va = alias.va + page_offset - alias.offset;
                self.backend.unmap_page(process.root, va);
                self.backend
                    .map_page(process.root, va, pa, alias.rights, device)?;
                self.backend.flush_page(process.asid, va);
            }
        }
        Ok(())
    }

    pub fn commit_user_write_fault(&mut self, va: u64) -> Result<()> {
        let page_va = va & !(PAGE - 1);
        let Some(mapping) = self.processes[self.current]
            .mappings
            .iter()
            .copied()
            .find(|m| page_va >= m.va && page_va < m.va + m.size)
        else {
            return Err(Status::ErrAccessDenied);
        };
        if mapping.rights & WRITE == 0 {
            return Err(Status::ErrAccessDenied);
        }
        let page_offset = mapping.offset + page_va - mapping.va;
        let (pa, _) = self.vmo_page_phys(mapping.vmo, page_offset, true)?;
        self.remap_committed_page(mapping, page_offset, pa)
    }

    pub fn commit_user_range(&mut self, va: u64, len: u64) -> Result<()> {
        if len == 0
            || va.checked_add(len).is_none()
            || !self.valid_range(self.current, va, len, WRITE)
        {
            return Err(Status::ErrAccessDenied);
        }
        let start = va & !(PAGE - 1);
        let end = va
            .checked_add(len)
            .and_then(|value| value.checked_add(PAGE - 1))
            .map(|value| value & !(PAGE - 1))
            .ok_or(Status::ErrInvalidArgs)?;
        let pid = self.current;
        let batch = self.processes[pid]
            .mappings
            .iter()
            .copied()
            .find(|mapping| {
                start >= mapping.va
                    && end <= mapping.va.saturating_add(mapping.size)
                    && mapping.rights & WRITE != 0
            })
            .and_then(|mapping| {
                let first = ((mapping.offset + start - mapping.va) / PAGE) as usize;
                let count = ((end - start) / PAGE) as usize;
                let untouched = self.vmos[mapping.vmo]
                    .as_ref()
                    .and_then(|vmo| match &vmo.backing {
                        VmoBacking::LazyAnonymous { pages } => pages.get(first..first + count),
                        VmoBacking::Contiguous { .. } | VmoBacking::SharedDevice { .. } => None,
                    })
                    .is_some_and(|pages| pages.iter().all(Option::is_none));
                // The bulk path only updates this mapping. Shared VMOs use
                // the per-page path below to update every existing alias.
                let exclusive = self
                    .processes
                    .iter()
                    .flat_map(|p| &p.mappings)
                    .filter(|alias| alias.vmo == mapping.vmo)
                    .count()
                    == 1;
                (untouched && exclusive).then_some((mapping, first, count))
            });
        if let Some((mapping, first, count)) = batch {
            const MAX_COMMIT_RUN_PAGES: usize = 64;
            let root = self.processes[pid].root;
            let mut committed = 0;
            while committed < count {
                let mut run = core::cmp::min(MAX_COMMIT_RUN_PAGES, count - committed);
                let base = loop {
                    match self.backend.allocate(run as u64) {
                        Ok(base) => break base,
                        Err(_) if run > 1 => run = core::cmp::max(1, run / 2),
                        Err(status) => return Err(status),
                    }
                };
                let vmo = self.vmos[mapping.vmo]
                    .as_mut()
                    .ok_or(Status::ErrInvalidHandle)?;
                let VmoBacking::LazyAnonymous { pages } = &mut vmo.backing else {
                    self.backend.release(base, run as u64);
                    return Err(Status::ErrInvalidHandle);
                };
                for (index, slot) in pages[first + committed..first + committed + run]
                    .iter_mut()
                    .enumerate()
                {
                    *slot = Some(base + index as u64 * PAGE);
                }
                for index in 0..run {
                    self.backend.map_page(
                        root,
                        start + (committed + index) as u64 * PAGE,
                        base + index as u64 * PAGE,
                        mapping.rights,
                        false,
                    )?;
                }
                committed += run;
            }
            self.changed(VMO, mapping.vmo);
            self.backend.flush_mappings();
            return Ok(());
        }
        for page_va in (start..end).step_by(PAGE as usize) {
            let mapping = self.processes[pid]
                .mappings
                .iter()
                .copied()
                .find(|mapping| page_va >= mapping.va && page_va < mapping.va + mapping.size)
                .ok_or(Status::ErrAccessDenied)?;
            let page_offset = mapping.offset + page_va - mapping.va;
            let (pa, _) = self.vmo_page_phys(mapping.vmo, page_offset, true)?;
            self.remap_committed_page(mapping, page_offset, pa)?;
        }
        self.backend.flush_mappings();
        Ok(())
    }

    pub fn get_time_ns(&self, clock_type: kernel_fidl::ClockType) -> Result<u64> {
        match clock_type {
            kernel_fidl::ClockType::Monotonic | kernel_fidl::ClockType::BootTime => {
                self.backend.monotonic_ns().ok_or(Status::ErrInvalidArgs)
            }
            kernel_fidl::ClockType::Realtime => self
                .backend
                .monotonic_ns()
                .ok_or(Status::ErrInvalidArgs)
                .and_then(|nanos| {
                    self.realtime_slew
                        .realtime_at(nanos)
                        .ok_or(Status::ErrInvalidArgs)
                }),
        }
    }

    pub fn adjust_clock(
        &mut self,
        clock_type: kernel_fidl::ClockType,
        offset_delta_ns: i64,
        slew_rate_ppm: i32,
        ticks: u64,
        frequency_hz: u64,
    ) -> Result<()> {
        if clock_type != kernel_fidl::ClockType::Realtime {
            return Err(Status::ErrInvalidArgs);
        }
        let monotonic_ns = self.backend.monotonic_ns().ok_or(Status::ErrInvalidArgs)?;
        self.realtime_slew = self
            .realtime_slew
            .adjust(monotonic_ns, offset_delta_ns, slew_rate_ppm)
            .map_err(|_| Status::ErrInvalidArgs)?;
        self.update_time_page(ticks, frequency_hz);
        self.changed(super::incremental::META, 0);
        Ok(())
    }

    pub fn get_vdso_time_page(&mut self, ticks: u64, frequency_hz: u64) -> Result<u64> {
        let vmo_id = match self.time_page_vmo {
            Some(id) => id,
            None => {
                let base = self.backend.allocate(1)?;
                let id = self.insert_vmo(Vmo {
                    size: PAGE,
                    refs: 0,
                    device: false,
                    bootfs: false,
                    backing: VmoBacking::Contiguous { base },
                });
                self.time_page_vmo = Some(id);
                self.changed(super::incremental::VMO, id);
                self.changed(super::incremental::META, 0);
                id
            }
        };
        self.update_time_page(ticks, frequency_hz);
        Ok(self.grant(
            self.current,
            Object::Vmo(vmo_id),
            READ | MAP | DUPLICATE | TRANSFER,
        ))
    }

    pub fn update_time_page(&mut self, ticks: u64, frequency_hz: u64) {
        let Some(vmo_id) = self.time_page_vmo else {
            return;
        };
        let Some(vmo) = self.vmos.get(vmo_id).and_then(|v| v.as_ref()) else {
            return;
        };
        let monotonic_ns = self.backend.monotonic_ns().unwrap_or(0);
        let snapshot = TimePageSnapshot {
            base_ticks: ticks,
            tick_frequency_hz: frequency_hz.max(1),
            base_monotonic_ns: monotonic_ns,
            realtime_offset_ns: self.realtime_slew.realtime_offset_ns,
            slew_start_monotonic_ns: self.realtime_slew.slew_start_monotonic_ns,
            slew_remaining_ns: self.realtime_slew.slew_remaining_ns,
            slew_rate_ppm: self.realtime_slew.slew_rate_ppm,
        };
        let VmoBacking::Contiguous { base } = vmo.backing else {
            return;
        };
        write_time_page(&mut self.backend, base, snapshot);
    }
}

fn write_time_page<B: Backend>(backend: &mut B, base: u64, snapshot: TimePageSnapshot) {
    let mut current_sequence = [0; 4];
    backend.read(base + 16, &mut current_sequence);
    let sequence = u32::from_le_bytes(current_sequence);
    write_at(backend, base, 0, &TIME_PAGE_MAGIC.to_le_bytes());
    write_at(backend, base, 8, &TIME_PAGE_VERSION.to_le_bytes());
    write_at(backend, base, 12, &(TIME_PAGE_SIZE as u32).to_le_bytes());
    write_at(backend, base, 16, &(sequence | 1).to_le_bytes());
    write_at(backend, base, 20, &0u32.to_le_bytes());
    write_at(backend, base, 24, &snapshot.base_ticks.to_le_bytes());
    write_at(
        backend,
        base,
        32,
        &snapshot.tick_frequency_hz.max(1).to_le_bytes(),
    );
    write_at(backend, base, 40, &snapshot.base_monotonic_ns.to_le_bytes());
    write_at(
        backend,
        base,
        48,
        &snapshot.realtime_offset_ns.to_le_bytes(),
    );
    write_at(
        backend,
        base,
        56,
        &snapshot.slew_start_monotonic_ns.to_le_bytes(),
    );
    write_at(backend, base, 64, &snapshot.slew_remaining_ns.to_le_bytes());
    write_at(backend, base, 72, &snapshot.slew_rate_ppm.to_le_bytes());
    write_at(backend, base, 76, &0u32.to_le_bytes());
    write_at(
        backend,
        base,
        16,
        &((sequence.wrapping_add(2)) & !1).to_le_bytes(),
    );
}

fn write_at<B: Backend>(backend: &mut B, base: u64, offset: u64, bytes: &[u8]) {
    backend.write(base + offset, bytes);
}

pub(super) fn overlaps(a_base: u64, a_size: u64, b_base: u64, b_size: u64) -> bool {
    a_base < b_base.saturating_add(b_size) && b_base < a_base.saturating_add(a_size)
}

pub(super) fn range_contains(base: u64, size: u64, child_base: u64, child_size: u64) -> bool {
    child_base >= base
        && child_base
            .checked_add(child_size)
            .is_some_and(|end| end <= base.saturating_add(size))
}

fn vmar_flags_to_rights(flags: u32) -> Result<u32> {
    const CAN_MAP_READ: u32 = 0x0000_0001;
    const CAN_MAP_WRITE: u32 = 0x0000_0002;
    const CAN_MAP_EXECUTE: u32 = 0x0000_0004;
    const CAN_MAP_SPECIFIC: u32 = 0x0000_0008;
    const COMPACT: u32 = 0x0000_0010;
    if flags & !(CAN_MAP_READ | CAN_MAP_WRITE | CAN_MAP_EXECUTE | CAN_MAP_SPECIFIC | COMPACT | 0x20)
        != 0
    {
        return Err(Status::ErrInvalidArgs);
    }
    let mut rights = 0;
    if flags & CAN_MAP_READ != 0 {
        rights |= READ;
    }
    if flags & CAN_MAP_WRITE != 0 {
        rights |= WRITE;
    }
    if flags & CAN_MAP_EXECUTE != 0 {
        rights |= EXECUTE;
    }
    if rights == 0 {
        return Err(Status::ErrAccessDenied);
    }
    Ok(rights)
}
