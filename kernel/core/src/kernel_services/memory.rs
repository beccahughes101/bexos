use crate::memory::{PAGE_SIZE, align_up};

use super::handle::ObjectKind;
use super::vmar::{
    VMAR_FLAG_CAN_MAP_EXECUTE, VMAR_FLAG_CAN_MAP_SPECIFIC, VMAR_FLAG_CAN_MAP_WRITE, VMAR_FLAGS_ALL,
    VmarRecord, rights_to_vmar_flags, validate_vmar_flags, vmar_flags_to_rights,
};
use super::{
    ControlPlane, Handle, HardwareAccess, KernelServiceStatus, MapResult, RIGHT_ADMIN,
    RIGHT_DUPLICATE, RIGHT_EXECUTE, RIGHT_MAP, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE,
};

pub const VMO_FLAG_RESIZABLE: u32 = 0x0000_0001;
pub const VMO_FLAG_CONTIGUOUS_PHYS: u32 = 0x0000_0002;
pub const VMO_FLAG_CACHE_POLICY_WB: u32 = 0x0000_0004;
pub const VMO_FLAG_CACHE_POLICY_UC: u32 = 0x0000_0008;
const FIRST_USER_VADDR: u64 = bexos_boot::USER_START;
const SYNTHETIC_PHYS_BASE: u64 = 0x5000_0000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VmoBackingKind {
    LazyAnonymous,
    Contiguous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VmoRecord {
    pub id: u64,
    pub size_bytes: u64,
    pub flags: u32,
    pub parent_vmo: Option<u64>,
    pub parent_offset: u64,
    pub charged_resource_group_id: Option<u32>,
    pub backing: VmoBackingKind,
    pub frames: Vec<Option<u64>>,
}

impl VmoRecord {
    pub fn frame_for_offset(&self, offset: u64) -> Option<u64> {
        let frame_index = (offset / PAGE_SIZE as u64) as usize;
        self.frames.get(frame_index).copied().flatten()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingRecord {
    pub vm_space_id: u64,
    pub vmar_id: u64,
    pub vmo_id: u64,
    pub vmo_offset: u64,
    pub size_bytes: u64,
    pub vaddr: u64,
    pub rights: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameRecord {
    phys: u64,
    refs: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VmoTable {
    entries: Vec<VmoRecord>,
    next_vmo_id: u64,
    next_phys: u64,
    free_frames: Vec<u64>,
    frame_refs: Vec<FrameRecord>,
}

impl VmoTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_vmo_id: 1,
            next_phys: SYNTHETIC_PHYS_BASE,
            free_frames: Vec::new(),
            frame_refs: Vec::new(),
        }
    }

    pub fn insert(
        &mut self,
        size_bytes: u64,
        flags: u32,
        parent_vmo: Option<u64>,
        parent_offset: u64,
        charged_resource_group_id: Option<u32>,
    ) -> Result<u64, KernelServiceStatus> {
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let frame_count = pages_for_size(size_bytes)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(frame_count)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        if let Some(parent_id) = parent_vmo {
            let parent = self
                .get(parent_id)
                .ok_or(KernelServiceStatus::InvalidHandle)?;
            let first = (parent_offset / PAGE_SIZE as u64) as usize;
            let end = first
                .checked_add(frame_count)
                .ok_or(KernelServiceStatus::InvalidArgs)?;
            let selected = parent
                .frames
                .get(first..end)
                .ok_or(KernelServiceStatus::InvalidArgs)?
                .to_vec();
            for frame in &selected {
                if let Some(frame) = *frame {
                    self.retain_frame(frame)?;
                }
            }
            frames = selected;
        } else if flags & VMO_FLAG_CONTIGUOUS_PHYS == 0 {
            frames.resize(frame_count, None);
        } else {
            for _ in 0..frame_count {
                match self.allocate_frame() {
                    Ok(frame) => frames.push(Some(frame)),
                    Err(status) => {
                        for frame in frames.into_iter().flatten() {
                            self.release_frame(frame);
                        }
                        return Err(status);
                    }
                }
            }
        }

        let id = self.next_vmo_id;
        self.next_vmo_id = self.next_vmo_id.saturating_add(1);
        self.entries.push(VmoRecord {
            id,
            size_bytes,
            flags,
            parent_vmo,
            parent_offset,
            charged_resource_group_id,
            backing: if flags & VMO_FLAG_CONTIGUOUS_PHYS != 0 {
                VmoBackingKind::Contiguous
            } else {
                VmoBackingKind::LazyAnonymous
            },
            frames,
        });
        Ok(id)
    }

    pub fn get(&self, id: u64) -> Option<VmoRecord> {
        self.entries.iter().find(|vmo| vmo.id == id).cloned()
    }

    pub fn remove(&mut self, id: u64) -> Result<VmoRecord, KernelServiceStatus> {
        let index = self
            .entries
            .iter()
            .position(|vmo| vmo.id == id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        let vmo = self.entries.swap_remove(index);
        for frame in vmo.frames.iter().copied().flatten() {
            self.release_frame(frame);
        }
        Ok(vmo)
    }

    pub fn free_frame_count(&self) -> usize {
        self.free_frames.len()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn allocate_frame(&mut self) -> Result<u64, KernelServiceStatus> {
        let frame = self.free_frames.pop().unwrap_or_else(|| {
            let frame = self.next_phys;
            self.next_phys = self.next_phys.saturating_add(PAGE_SIZE as u64);
            frame
        });
        self.frame_refs
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        self.frame_refs.push(FrameRecord {
            phys: frame,
            refs: 1,
        });
        Ok(frame)
    }

    fn retain_frame(&mut self, frame: u64) -> Result<(), KernelServiceStatus> {
        let record = self
            .frame_refs
            .iter_mut()
            .find(|record| record.phys == frame)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        record.refs = record
            .refs
            .checked_add(1)
            .ok_or(KernelServiceStatus::NoMemory)?;
        Ok(())
    }

    fn release_frame(&mut self, frame: u64) {
        if let Some(index) = self
            .frame_refs
            .iter()
            .position(|record| record.phys == frame)
        {
            if self.frame_refs[index].refs > 1 {
                self.frame_refs[index].refs -= 1;
            } else {
                self.frame_refs.swap_remove(index);
                self.free_frames.push(frame);
            }
        }
    }

    pub fn frame_ref_count(&self, frame: u64) -> usize {
        self.frame_refs
            .iter()
            .find(|record| record.phys == frame)
            .map_or(0, |record| record.refs)
    }

    pub fn resolve_cow_page(
        &mut self,
        vmo_id: u64,
        page_index: usize,
    ) -> Result<(u64, Option<u64>), KernelServiceStatus> {
        let old = self
            .entries
            .iter()
            .find(|vmo| vmo.id == vmo_id)
            .and_then(|vmo| vmo.frames.get(page_index))
            .copied()
            .flatten()
            .ok_or(KernelServiceStatus::InvalidArgs)?;
        if self.frame_ref_count(old) <= 1 {
            return Ok((old, None));
        }
        let new = self.allocate_frame()?;
        let vmo = self
            .entries
            .iter_mut()
            .find(|vmo| vmo.id == vmo_id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        vmo.frames[page_index] = Some(new);
        self.release_frame(old);
        Ok((new, Some(old)))
    }
}

impl Default for VmoTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappingTable {
    entries: Vec<MappingRecord>,
    next_vaddr: u64,
}

impl MappingTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_vaddr: FIRST_USER_VADDR,
        }
    }

    pub fn insert(
        &mut self,
        vm_space_id: u64,
        vmar: VmarRecord,
        vmo_id: u64,
        vmo_offset: u64,
        size_bytes: u64,
        requested_vaddr: u64,
        rights: u32,
        reserved_children: &[VmarRecord],
    ) -> Result<u64, KernelServiceStatus> {
        let vaddr = if requested_vaddr == 0 {
            self.allocate_vaddr_in_vmar(vmar, size_bytes, reserved_children)?
        } else {
            requested_vaddr
        };
        if !vmar.contains_range(vaddr, size_bytes) {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        if reserved_children
            .iter()
            .any(|child| overlaps(child.base, child.size_bytes, vaddr, size_bytes))
        {
            return Err(KernelServiceStatus::AlreadyExists);
        }
        if self.entries.iter().any(|mapping| {
            mapping.vm_space_id == vm_space_id
                && overlaps(mapping.vaddr, mapping.size_bytes, vaddr, size_bytes)
        }) {
            return Err(KernelServiceStatus::AlreadyExists);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        self.entries.push(MappingRecord {
            vm_space_id,
            vmar_id: vmar.id,
            vmo_id,
            vmo_offset,
            size_bytes,
            vaddr,
            rights,
        });
        Ok(vaddr)
    }

    pub fn remove(&mut self, vm_space_id: u64, vaddr: u64, size_bytes: u64) -> KernelServiceStatus {
        if let Some(index) = self.entries.iter().position(|mapping| {
            mapping.vm_space_id == vm_space_id
                && mapping.vaddr == vaddr
                && mapping.size_bytes == size_bytes
        }) {
            self.entries.swap_remove(index);
            return KernelServiceStatus::Ok;
        }
        KernelServiceStatus::InvalidHandle
    }

    pub fn remove_in_vmar(
        &mut self,
        vm_space_id: u64,
        vmar_ids: &[u64],
        vaddr: u64,
        size_bytes: u64,
    ) -> KernelServiceStatus {
        if let Some(index) = self.entries.iter().position(|mapping| {
            mapping.vm_space_id == vm_space_id
                && vmar_ids.contains(&mapping.vmar_id)
                && mapping.vaddr == vaddr
                && mapping.size_bytes == size_bytes
        }) {
            self.entries.swap_remove(index);
            return KernelServiceStatus::Ok;
        }
        KernelServiceStatus::InvalidHandle
    }

    pub fn remove_for_vmo(&mut self, vmo_id: u64) -> usize {
        let mut removed = 0;
        self.entries.retain(|mapping| {
            let keep = mapping.vmo_id != vmo_id;
            if !keep {
                removed += 1;
            }
            keep
        });
        removed
    }

    pub fn remove_for_vmars(&mut self, vmar_ids: &[u64]) -> usize {
        let mut removed = 0;
        self.entries.retain(|mapping| {
            let keep = !vmar_ids.contains(&mapping.vmar_id);
            if !keep {
                removed += 1;
            }
            keep
        });
        removed
    }

    pub fn overlaps_in_vm_space(&self, vm_space_id: u64, base: u64, size_bytes: u64) -> bool {
        self.entries.iter().any(|mapping| {
            mapping.vm_space_id == vm_space_id
                && overlaps(mapping.vaddr, mapping.size_bytes, base, size_bytes)
        })
    }

    pub fn covers_range(&self, vm_space_id: u64, vaddr: u64, size_bytes: u64, rights: u32) -> bool {
        let Some(end) = vaddr.checked_add(size_bytes) else {
            return false;
        };
        let mut position = vaddr;
        while position < end {
            let Some(mapping) = self.entries.iter().find(|mapping| {
                mapping.vm_space_id == vm_space_id
                    && position >= mapping.vaddr
                    && position < mapping.vaddr.saturating_add(mapping.size_bytes)
                    && mapping.rights & rights == rights
            }) else {
                return false;
            };
            position = core::cmp::min(end, mapping.vaddr.saturating_add(mapping.size_bytes));
        }
        true
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn allocate_vaddr_in_vmar(
        &mut self,
        vmar: VmarRecord,
        size_bytes: u64,
        reserved_children: &[VmarRecord],
    ) -> Result<u64, KernelServiceStatus> {
        let mut selected = if vmar.flags & super::vmar::VMAR_FLAG_COMPACT != 0 {
            vmar.base
        } else {
            self.next_vaddr.max(vmar.base)
        };
        selected = align_up(selected as usize, PAGE_SIZE) as u64;
        while vmar.contains_range(selected, size_bytes) {
            if !self.overlaps_in_vm_space(vmar.vm_space_id, selected, size_bytes)
                && !reserved_children
                    .iter()
                    .any(|child| overlaps(child.base, child.size_bytes, selected, size_bytes))
            {
                self.next_vaddr =
                    align_up(selected.saturating_add(size_bytes) as usize, PAGE_SIZE) as u64;
                return Ok(selected);
            }
            let next_mapping = self
                .entries
                .iter()
                .filter(|mapping| {
                    mapping.vm_space_id == vmar.vm_space_id
                        && mapping.vaddr <= selected
                        && selected < mapping.vaddr.saturating_add(mapping.size_bytes)
                })
                .map(|mapping| mapping.vaddr.saturating_add(mapping.size_bytes))
                .max();
            let next_child = reserved_children
                .iter()
                .filter(|child| {
                    child.base <= selected && selected < child.base.saturating_add(child.size_bytes)
                })
                .map(|child| child.base.saturating_add(child.size_bytes))
                .max();
            let next = next_mapping
                .into_iter()
                .chain(next_child)
                .max()
                .unwrap_or_else(|| selected.saturating_add(PAGE_SIZE as u64));
            selected = align_up(next as usize, PAGE_SIZE) as u64;
        }
        Err(KernelServiceStatus::NoMemory)
    }
}

impl Default for MappingTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlPlane {
    pub fn create_vmo(
        &mut self,
        size_bytes: u64,
        flags: u32,
    ) -> Result<Handle, KernelServiceStatus> {
        let size = checked_page_size(size_bytes)?;
        validate_vmo_flags(flags)?;
        if hardware_restricted_vmo(flags)
            && self.current_hardware_access()? != HardwareAccess::Direct
        {
            return Err(KernelServiceStatus::AccessDenied);
        }
        let (process, _) = self.ensure_current_process()?;
        let resource_group_id = self.process_for_handle(process)?.resource_group_id;
        if !hardware_restricted_vmo(flags) {
            self.resource_groups
                .charge_memory(resource_group_id, size)?;
        }
        let charged = (!hardware_restricted_vmo(flags)).then_some(resource_group_id);
        let id = match self.vmos.insert(size, flags, None, 0, charged) {
            Ok(id) => id,
            Err(status) => {
                if charged.is_some() {
                    let _ = self.resource_groups.release_memory(resource_group_id, size);
                }
                return Err(status);
            }
        };
        match self.handles.insert(
            id,
            ObjectKind::Vmo,
            RIGHT_READ | RIGHT_WRITE | RIGHT_EXECUTE | RIGHT_MAP | RIGHT_DUPLICATE | RIGHT_TRANSFER,
            0,
            None,
        ) {
            Ok(handle) => Ok(handle),
            Err(status) => {
                let _ = self.vmos.remove(id);
                if let Some(group_id) = charged {
                    let _ = self.resource_groups.release_memory(group_id, size);
                }
                Err(status)
            }
        }
    }

    pub fn map_vmo(
        &mut self,
        vmo: Handle,
        offset: u64,
        size_bytes: u64,
        target_vaddr: u64,
        requested_rights: u32,
    ) -> MapResult {
        let Ok((_process, vm_space)) = self.ensure_current_process() else {
            return map_status(KernelServiceStatus::InvalidHandle);
        };
        self.map_vmo_in_vm_space(
            vm_space,
            vmo,
            offset,
            size_bytes,
            target_vaddr,
            requested_rights,
        )
    }

    pub fn map_vmo_in_vm_space(
        &mut self,
        vm_space: Handle,
        vmo: Handle,
        offset: u64,
        size_bytes: u64,
        target_vaddr: u64,
        requested_rights: u32,
    ) -> MapResult {
        let Ok(vm_space_record) = self.vm_space_for_handle(vm_space) else {
            return map_status(KernelServiceStatus::InvalidHandle);
        };
        let Some(root_vmar) = self.vmars.root_for_vm_space(vm_space_record.id) else {
            return map_status(KernelServiceStatus::InvalidHandle);
        };
        let requested_flags = rights_to_vmar_flags(requested_rights) | VMAR_FLAG_CAN_MAP_SPECIFIC;
        let vmar_offset = if target_vaddr == 0 {
            0
        } else if target_vaddr < root_vmar.base {
            return map_status(KernelServiceStatus::InvalidArgs);
        } else {
            target_vaddr - root_vmar.base
        };
        self.map_vmo_in_vmar_record(
            root_vmar,
            vmo,
            offset,
            vmar_offset,
            size_bytes,
            requested_flags,
        )
    }

    pub fn create_sub_vmar(
        &mut self,
        parent_vmar: Handle,
        offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<(Handle, u64), KernelServiceStatus> {
        let parent = self.vmar_for_handle(parent_vmar, RIGHT_MAP)?;
        let offset = if flags & super::vmar::VMAR_FLAG_ALLOCATE != 0 {
            if offset != 0 {
                return Err(KernelServiceStatus::InvalidArgs);
            }
            super::vmar::validate_vmar_size(size_bytes)?;
            let children = self.vmars.active_children(parent.id);
            self.mappings
                .allocate_vaddr_in_vmar(parent, size_bytes, &children)?
                - parent.base
        } else {
            offset
        };
        let Some(base) = parent.base.checked_add(offset) else {
            return Err(KernelServiceStatus::InvalidArgs);
        };
        if self
            .mappings
            .overlaps_in_vm_space(parent.vm_space_id, base, size_bytes)
        {
            return Err(KernelServiceStatus::AlreadyExists);
        }
        let child = self.vmars.insert_child(parent, offset, size_bytes, flags)?;
        let handle = self.handles.insert(
            child.id,
            ObjectKind::Vmar,
            RIGHT_MAP
                | RIGHT_TRANSFER
                | vmar_flags_to_rights(child.flags)
                | if child.flags & VMAR_FLAG_CAN_MAP_SPECIFIC != 0 {
                    RIGHT_ADMIN
                } else {
                    0
                },
            0,
            None,
        )?;
        Ok((handle, child.base))
    }

    pub fn map_vmo_in_vmar(
        &mut self,
        vmar: Handle,
        vmo: Handle,
        vmo_offset: u64,
        vmar_offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> MapResult {
        let Ok(vmar_record) = self.vmar_for_handle(vmar, RIGHT_MAP) else {
            return map_status(KernelServiceStatus::InvalidHandle);
        };
        self.map_vmo_in_vmar_record(vmar_record, vmo, vmo_offset, vmar_offset, size_bytes, flags)
    }

    fn map_vmo_in_vmar_record(
        &mut self,
        vmar_record: VmarRecord,
        vmo: Handle,
        offset: u64,
        vmar_offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> MapResult {
        if let Err(status) = validate_vmar_flags(flags) {
            return map_status(status);
        }
        if vmar_record.destroyed {
            return map_status(KernelServiceStatus::InvalidHandle);
        }
        if flags & VMAR_FLAGS_ALL == 0 {
            return map_status(KernelServiceStatus::InvalidArgs);
        }
        if flags & (VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE)
            == VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE
        {
            return map_status(KernelServiceStatus::AccessDenied);
        }
        if flags & !vmar_record.flags & VMAR_FLAGS_ALL != 0 {
            return map_status(KernelServiceStatus::AccessDenied);
        }
        if vmar_offset != 0 && vmar_record.flags & VMAR_FLAG_CAN_MAP_SPECIFIC == 0 {
            return map_status(KernelServiceStatus::AccessDenied);
        }
        let Some(record) = self.handles.get(vmo.raw) else {
            return map_status(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Vmo || !record.has_rights(RIGHT_MAP) {
            return map_status(KernelServiceStatus::AccessDenied);
        }
        let requested_rights = vmar_flags_to_rights(flags);
        if requested_rights & (RIGHT_WRITE | RIGHT_EXECUTE) == RIGHT_WRITE | RIGHT_EXECUTE {
            return map_status(KernelServiceStatus::AccessDenied);
        }
        if requested_rights & !record.rights != 0 {
            return map_status(KernelServiceStatus::AccessDenied);
        }
        let Ok(size) = checked_page_size(size_bytes) else {
            return map_status(KernelServiceStatus::InvalidArgs);
        };
        if offset % PAGE_SIZE as u64 != 0 || vmar_offset % PAGE_SIZE as u64 != 0 {
            return map_status(KernelServiceStatus::InvalidArgs);
        }
        let Some(vmo_record) = self.vmos.get(record.object_id) else {
            return map_status(KernelServiceStatus::InvalidHandle);
        };
        if offset
            .checked_add(size)
            .is_none_or(|end| end > vmo_record.size_bytes)
        {
            return map_status(KernelServiceStatus::InvalidArgs);
        }
        let target_vaddr = if vmar_offset == 0 {
            0
        } else {
            match vmar_record.base.checked_add(vmar_offset) {
                Some(vaddr) => vaddr,
                None => return map_status(KernelServiceStatus::InvalidArgs),
            }
        };
        if target_vaddr != 0 && !vmar_record.contains_range(target_vaddr, size) {
            return map_status(KernelServiceStatus::InvalidArgs);
        }
        if target_vaddr != 0
            && self
                .vmars
                .active_children_overlap(vmar_record.id, target_vaddr, size)
        {
            return map_status(KernelServiceStatus::AlreadyExists);
        }
        let reserved_children = self.vmars.active_children(vmar_record.id);
        match self.mappings.insert(
            vmar_record.vm_space_id,
            vmar_record,
            record.object_id,
            offset,
            size,
            target_vaddr,
            requested_rights,
            &reserved_children,
        ) {
            Ok(mapped_vaddr) => MapResult {
                status: KernelServiceStatus::Ok,
                mapped_vaddr,
            },
            Err(status) => map_status(status),
        }
    }

    pub fn unmap(&mut self, vaddr: u64, size_bytes: u64) -> KernelServiceStatus {
        let Ok((_process, vm_space)) = self.ensure_current_process() else {
            return KernelServiceStatus::InvalidHandle;
        };
        self.unmap_in_vm_space(vm_space, vaddr, size_bytes)
    }

    pub fn commit_range(&mut self, vaddr: u64, size_bytes: u64) -> KernelServiceStatus {
        if size_bytes == 0 {
            return KernelServiceStatus::InvalidArgs;
        }
        let Ok((_process, vm_space)) = self.ensure_current_process() else {
            return KernelServiceStatus::InvalidHandle;
        };
        let Ok(vm_space_record) = self.vm_space_for_handle(vm_space) else {
            return KernelServiceStatus::InvalidHandle;
        };
        if self
            .mappings
            .covers_range(vm_space_record.id, vaddr, size_bytes, RIGHT_WRITE)
        {
            KernelServiceStatus::Ok
        } else {
            KernelServiceStatus::AccessDenied
        }
    }

    pub fn unmap_in_vm_space(
        &mut self,
        vm_space: Handle,
        vaddr: u64,
        size_bytes: u64,
    ) -> KernelServiceStatus {
        let Ok(vm_space_record) = self.vm_space_for_handle(vm_space) else {
            return KernelServiceStatus::InvalidHandle;
        };
        let Some(root_vmar) = self.vmars.root_for_vm_space(vm_space_record.id) else {
            return KernelServiceStatus::InvalidHandle;
        };
        self.unmap_in_vmar_record(root_vmar, vaddr, size_bytes)
    }

    pub fn unmap_in_vmar(
        &mut self,
        vmar: Handle,
        vaddr: u64,
        size_bytes: u64,
    ) -> KernelServiceStatus {
        let Ok(vmar_record) = self.vmar_for_handle(vmar, RIGHT_MAP) else {
            return KernelServiceStatus::InvalidHandle;
        };
        self.unmap_in_vmar_record(vmar_record, vaddr, size_bytes)
    }

    fn unmap_in_vmar_record(
        &mut self,
        vmar_record: VmarRecord,
        vaddr: u64,
        size_bytes: u64,
    ) -> KernelServiceStatus {
        let Ok(size) = checked_page_size(size_bytes) else {
            return KernelServiceStatus::InvalidArgs;
        };
        if vaddr % PAGE_SIZE as u64 != 0 {
            return KernelServiceStatus::InvalidArgs;
        }
        if vmar_record.destroyed || !vmar_record.contains_range(vaddr, size) {
            return KernelServiceStatus::InvalidArgs;
        }
        let ids = self.vmars.subtree_ids(vmar_record.id);
        self.mappings
            .remove_in_vmar(vmar_record.vm_space_id, &ids, vaddr, size)
    }

    pub fn destroy_vmar(&mut self, vmar: Handle) -> KernelServiceStatus {
        let Some(record) = self.handles.get(vmar.raw) else {
            return KernelServiceStatus::InvalidHandle;
        };
        if record.kind != ObjectKind::Vmar {
            return KernelServiceStatus::InvalidHandle;
        }
        match self.vmars.destroy_subtree(record.object_id) {
            Ok(ids) => {
                let _ = self.handles.remove(vmar.raw);
                let _ = self.mappings.remove_for_vmars(&ids);
                KernelServiceStatus::Ok
            }
            Err(status) => status,
        }
    }

    pub fn root_vmar_for_vm_space(
        &self,
        vm_space: Handle,
    ) -> Result<VmarRecord, KernelServiceStatus> {
        let vm_space_record = self.vm_space_for_handle(vm_space)?;
        self.vmars
            .root_for_vm_space(vm_space_record.id)
            .ok_or(KernelServiceStatus::InvalidHandle)
    }

    pub fn root_vmar_handle_for_vm_space(
        &self,
        vm_space: Handle,
    ) -> Result<Handle, KernelServiceStatus> {
        let root = self.root_vmar_for_vm_space(vm_space)?;
        self.handles
            .entry_for_object(root.id, ObjectKind::Vmar)
            .map(|record| Handle {
                raw: record.handle_id,
            })
            .ok_or(KernelServiceStatus::InvalidHandle)
    }

    pub fn vmar_for_handle(
        &self,
        vmar: Handle,
        rights: u32,
    ) -> Result<VmarRecord, KernelServiceStatus> {
        let Some(record) = self.handles.get(vmar.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Vmar || !record.has_rights(rights) {
            return Err(KernelServiceStatus::AccessDenied);
        }
        self.vmars
            .get(record.object_id)
            .filter(|vmar| !vmar.destroyed)
            .ok_or(KernelServiceStatus::InvalidHandle)
    }

    pub fn clone_vmo(
        &mut self,
        parent_vmo: Handle,
        offset: u64,
        size_bytes: u64,
    ) -> Result<Handle, KernelServiceStatus> {
        let Some(record) = self.handles.get(parent_vmo.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Vmo || !record.has_rights(RIGHT_DUPLICATE) {
            return Err(KernelServiceStatus::AccessDenied);
        }
        let size = checked_page_size(size_bytes)?;
        if offset % PAGE_SIZE as u64 != 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let Some(parent) = self.vmos.get(record.object_id) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if offset
            .checked_add(size)
            .is_none_or(|end| end > parent.size_bytes)
        {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let id = self
            .vmos
            .insert(size, parent.flags, Some(parent.id), offset, None)?;
        self.handles.insert(
            id,
            ObjectKind::Vmo,
            record.rights,
            record.owner_task_id,
            None,
        )
    }

    pub fn release_vmo(&mut self, vmo: Handle) -> KernelServiceStatus {
        let Some(record) = self.handles.remove(vmo.raw) else {
            return KernelServiceStatus::InvalidHandle;
        };
        if record.kind != ObjectKind::Vmo {
            return KernelServiceStatus::InvalidHandle;
        }
        let _ = self.mappings.remove_for_vmo(record.object_id);
        match self.vmos.remove(record.object_id) {
            Ok(vmo) => {
                if let Some(group_id) = vmo.charged_resource_group_id {
                    let _ = self
                        .resource_groups
                        .release_memory(group_id, vmo.size_bytes);
                }
                KernelServiceStatus::Ok
            }
            Err(status) => status,
        }
    }
}

const fn hardware_restricted_vmo(flags: u32) -> bool {
    flags & (VMO_FLAG_CONTIGUOUS_PHYS | VMO_FLAG_CACHE_POLICY_UC) != 0
}

fn checked_page_size(size_bytes: u64) -> Result<u64, KernelServiceStatus> {
    if size_bytes == 0 || size_bytes > usize::MAX as u64 {
        return Err(KernelServiceStatus::InvalidArgs);
    }
    Ok(align_up(size_bytes as usize, PAGE_SIZE) as u64)
}

fn pages_for_size(size_bytes: u64) -> Result<usize, KernelServiceStatus> {
    let size = checked_page_size(size_bytes)?;
    Ok((size / PAGE_SIZE as u64) as usize)
}

fn validate_vmo_flags(flags: u32) -> Result<(), KernelServiceStatus> {
    if flags & (VMO_FLAG_CACHE_POLICY_WB | VMO_FLAG_CACHE_POLICY_UC)
        == VMO_FLAG_CACHE_POLICY_WB | VMO_FLAG_CACHE_POLICY_UC
    {
        Err(KernelServiceStatus::InvalidArgs)
    } else if flags
        & !(VMO_FLAG_RESIZABLE
            | VMO_FLAG_CONTIGUOUS_PHYS
            | VMO_FLAG_CACHE_POLICY_WB
            | VMO_FLAG_CACHE_POLICY_UC)
        != 0
    {
        Err(KernelServiceStatus::InvalidArgs)
    } else {
        Ok(())
    }
}

const fn overlaps(a_start: u64, a_len: u64, b_start: u64, b_len: u64) -> bool {
    let a_end = a_start.saturating_add(a_len);
    let b_end = b_start.saturating_add(b_len);
    a_start < b_end && b_start < a_end
}

const fn map_status(status: KernelServiceStatus) -> MapResult {
    MapResult {
        status,
        mapped_vaddr: 0,
    }
}
use alloc::vec::Vec;
