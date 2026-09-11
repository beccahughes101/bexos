use crate::memory::PAGE_SIZE;

use super::KernelServiceStatus;

pub const VMAR_FLAG_CAN_MAP_READ: u32 = 0x0000_0001;
pub const VMAR_FLAG_CAN_MAP_WRITE: u32 = 0x0000_0002;
pub const VMAR_FLAG_CAN_MAP_EXECUTE: u32 = 0x0000_0004;
pub const VMAR_FLAG_CAN_MAP_SPECIFIC: u32 = 0x0000_0008;
pub const VMAR_FLAG_COMPACT: u32 = 0x0000_0010;
pub const VMAR_FLAG_ALLOCATE: u32 = 0x0000_0020;
pub const VMAR_FLAGS_ALL: u32 =
    VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE;
pub const VMAR_VALID_FLAGS: u32 =
    VMAR_FLAGS_ALL | VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_COMPACT | VMAR_FLAG_ALLOCATE;

pub const USER_VMAR_BASE: u64 = bexos_boot::USER_START;
pub const USER_VMAR_SIZE: u64 = 0x0000_8000_0000_0000 - USER_VMAR_BASE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VmarRecord {
    pub id: u64,
    pub vm_space_id: u64,
    pub parent_vmar_id: Option<u64>,
    pub base: u64,
    pub size_bytes: u64,
    pub flags: u32,
    pub destroyed: bool,
}

impl VmarRecord {
    pub const fn is_root(&self) -> bool {
        self.parent_vmar_id.is_none()
    }

    pub fn contains_range(&self, base: u64, size_bytes: u64) -> bool {
        base.checked_add(size_bytes).is_some_and(|end| {
            base >= self.base && end <= self.base.saturating_add(self.size_bytes)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VmarTable {
    entries: Vec<VmarRecord>,
    next_vmar_id: u64,
}

impl VmarTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_vmar_id: 1,
        }
    }

    pub fn insert_root(&mut self, vm_space_id: u64) -> Result<u64, KernelServiceStatus> {
        if vm_space_id == 0 || self.root_for_vm_space(vm_space_id).is_some() {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        self.insert_record(
            vm_space_id,
            None,
            USER_VMAR_BASE,
            USER_VMAR_SIZE,
            VMAR_FLAGS_ALL | VMAR_FLAG_CAN_MAP_SPECIFIC,
        )
    }

    pub fn insert_child(
        &mut self,
        parent: VmarRecord,
        offset: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<VmarRecord, KernelServiceStatus> {
        validate_vmar_flags(flags)?;
        validate_vmar_size(size_bytes)?;
        if parent.destroyed || offset % PAGE_SIZE as u64 != 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        if flags & !parent.flags & VMAR_FLAGS_ALL != 0 {
            return Err(KernelServiceStatus::AccessDenied);
        }
        let base = parent
            .base
            .checked_add(offset)
            .ok_or(KernelServiceStatus::InvalidArgs)?;
        if !parent.contains_range(base, size_bytes) {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        if self.entries.iter().any(|entry| {
            entry.parent_vmar_id == Some(parent.id)
                && !entry.destroyed
                && overlaps(entry.base, entry.size_bytes, base, size_bytes)
        }) {
            return Err(KernelServiceStatus::AlreadyExists);
        }
        let id =
            self.insert_record(parent.vm_space_id, Some(parent.id), base, size_bytes, flags)?;
        self.get(id).ok_or(KernelServiceStatus::InvalidHandle)
    }

    pub fn get(&self, id: u64) -> Option<VmarRecord> {
        self.entries.iter().find(|vmar| vmar.id == id).copied()
    }

    pub fn root_for_vm_space(&self, vm_space_id: u64) -> Option<VmarRecord> {
        self.entries
            .iter()
            .find(|vmar| vmar.vm_space_id == vm_space_id && vmar.parent_vmar_id.is_none())
            .copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_descendant_or_self(&self, ancestor_id: u64, candidate_id: u64) -> bool {
        let mut current = Some(candidate_id);
        while let Some(id) = current {
            if id == ancestor_id {
                return true;
            }
            current = self.get(id).and_then(|vmar| vmar.parent_vmar_id);
        }
        false
    }

    pub fn active_children_overlap(&self, parent_id: u64, base: u64, size_bytes: u64) -> bool {
        self.entries.iter().any(|entry| {
            entry.parent_vmar_id == Some(parent_id)
                && !entry.destroyed
                && overlaps(entry.base, entry.size_bytes, base, size_bytes)
        })
    }

    pub fn active_children(&self, parent_id: u64) -> Vec<VmarRecord> {
        let mut children = Vec::new();
        for entry in &self.entries {
            if entry.parent_vmar_id == Some(parent_id) && !entry.destroyed {
                children.push(*entry);
            }
        }
        children
    }

    pub fn destroy_subtree(&mut self, root_id: u64) -> Result<Vec<u64>, KernelServiceStatus> {
        let root = self
            .get(root_id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        if root.destroyed {
            return Err(KernelServiceStatus::InvalidHandle);
        }
        if root.is_root() {
            return Err(KernelServiceStatus::AccessDenied);
        }
        let ids = self.subtree_ids(root_id);
        for id in ids.iter().copied() {
            if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                entry.destroyed = true;
            }
        }
        Ok(ids)
    }

    pub fn subtree_ids(&self, root_id: u64) -> Vec<u64> {
        let mut ids = Vec::new();
        ids.push(root_id);
        let mut index = 0;
        while index < ids.len() {
            let parent = ids[index];
            for entry in &self.entries {
                if entry.parent_vmar_id == Some(parent) && !entry.destroyed {
                    ids.push(entry.id);
                }
            }
            index += 1;
        }
        ids
    }

    fn insert_record(
        &mut self,
        vm_space_id: u64,
        parent_vmar_id: Option<u64>,
        base: u64,
        size_bytes: u64,
        flags: u32,
    ) -> Result<u64, KernelServiceStatus> {
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_vmar_id;
        self.next_vmar_id = self.next_vmar_id.saturating_add(1);
        self.entries.push(VmarRecord {
            id,
            vm_space_id,
            parent_vmar_id,
            base,
            size_bytes,
            flags,
            destroyed: false,
        });
        Ok(id)
    }
}

impl Default for VmarTable {
    fn default() -> Self {
        Self::new()
    }
}

pub fn validate_vmar_flags(flags: u32) -> Result<(), KernelServiceStatus> {
    if flags & !VMAR_VALID_FLAGS != 0 {
        return Err(KernelServiceStatus::InvalidArgs);
    }
    Ok(())
}

pub fn validate_vmar_size(size_bytes: u64) -> Result<(), KernelServiceStatus> {
    if size_bytes == 0 || size_bytes % PAGE_SIZE as u64 != 0 {
        return Err(KernelServiceStatus::InvalidArgs);
    }
    Ok(())
}

pub const fn rights_to_vmar_flags(rights: u32) -> u32 {
    let mut flags = 0;
    if rights & super::RIGHT_READ != 0 {
        flags |= VMAR_FLAG_CAN_MAP_READ;
    }
    if rights & super::RIGHT_WRITE != 0 {
        flags |= VMAR_FLAG_CAN_MAP_WRITE;
    }
    if rights & super::RIGHT_EXECUTE != 0 {
        flags |= VMAR_FLAG_CAN_MAP_EXECUTE;
    }
    flags
}

pub const fn vmar_flags_to_rights(flags: u32) -> u32 {
    let mut rights = 0;
    if flags & VMAR_FLAG_CAN_MAP_READ != 0 {
        rights |= super::RIGHT_READ;
    }
    if flags & VMAR_FLAG_CAN_MAP_WRITE != 0 {
        rights |= super::RIGHT_WRITE;
    }
    if flags & VMAR_FLAG_CAN_MAP_EXECUTE != 0 {
        rights |= super::RIGHT_EXECUTE;
    }
    rights
}

pub const fn overlaps(a_start: u64, a_len: u64, b_start: u64, b_len: u64) -> bool {
    let a_end = a_start.saturating_add(a_len);
    let b_end = b_start.saturating_add(b_len);
    a_start < b_end && b_start < a_end
}

use alloc::vec::Vec;
