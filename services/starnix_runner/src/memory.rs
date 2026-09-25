use bexos_userspace::Memory;
use bexos_zircon::{AsHandleRef, Vmar, VmarFlags, Vmo};
use starnix_kernel::{EACCES, EFAULT, EINVAL, ENOMEM, ENOTSUP};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    vec::Vec,
};

pub const PAGE_SIZE: u64 = 4096;
const BRK_BASE: u64 = 0x30_0000_0000;
const BRK_LIMIT: u64 = BRK_BASE + 256 * 1024 * 1024;
static NEXT_FUTEX_ID: AtomicU64 = AtomicU64::new(1);

pub fn new_futex_id() -> u64 {
    NEXT_FUTEX_ID.fetch_add(1, Ordering::Relaxed)
}

fn reserve_futex_id(id: u64) {
    NEXT_FUTEX_ID.fetch_max(id.saturating_add(1), Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingKind {
    Image,
    Stack,
    Anonymous,
    File {
        fd: i32,
        file_offset: u64,
        shared: bool,
    },
    SignalTrampoline,
}

fn advance_kind(kind: MappingKind, delta: u64) -> MappingKind {
    match kind {
        MappingKind::File {
            fd,
            file_offset,
            shared,
        } => MappingKind::File {
            fd,
            file_offset: file_offset.saturating_add(delta),
            shared,
        },
        kind => kind,
    }
}

pub struct Mapping {
    pub vmo: Arc<Vmo>,
    pub futex_id: u64,
    pub address: u64,
    pub size: u64,
    pub vmo_offset: u64,
    pub rights: u32,
    pub kind: MappingKind,
    pub(crate) mapped: bool,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        if self.mapped {
            let _ = Vmar::root_self().unmap(self.address, self.size);
        }
    }
}

pub struct AddressSpace {
    mappings: Vec<Mapping>,
    brk: u64,
}

fn round(value: u64) -> Result<u64, i64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|v| v & !(PAGE_SIZE - 1))
        .ok_or(ENOMEM)
}

fn flags(rights: u32, specific: bool) -> VmarFlags {
    let mut value = if specific { VmarFlags::SPECIFIC.0 } else { 0 };
    if rights & 2 != 0 {
        value |= VmarFlags::PERM_READ.0;
    }
    if rights & 4 != 0 {
        value |= VmarFlags::PERM_WRITE.0;
    }
    if rights & 8 != 0 {
        value |= VmarFlags::PERM_EXECUTE.0;
    }
    VmarFlags(value)
}

fn linux_rights(prot: u64) -> Result<u32, i64> {
    if prot & !7 != 0 {
        return Err(EINVAL);
    }
    let mut rights = 0;
    if prot & 1 != 0 {
        rights |= 2;
    }
    if prot & 2 != 0 {
        rights |= 4;
    }
    if prot & 4 != 0 {
        rights |= 8;
    }
    if rights & (4 | 8) == (4 | 8) {
        return Err(EACCES);
    }
    Ok(rights)
}

fn linux_protection(rights: u32) -> u64 {
    u64::from(rights & 2 != 0)
        | (u64::from(rights & 4 != 0) << 1)
        | (u64::from(rights & 8 != 0) << 2)
}

fn remap(
    vmo: Arc<Vmo>,
    futex_id: u64,
    address: u64,
    size: u64,
    offset: u64,
    rights: u32,
    kind: MappingKind,
) -> Result<Mapping, i64> {
    let mapped = Vmar::root_self()
        .map(address, &vmo, offset, size, flags(rights, true))
        .map_err(|_| ENOMEM)?;
    core::mem::forget(mapped);
    Ok(Mapping {
        vmo,
        futex_id,
        address,
        size,
        vmo_offset: offset,
        rights,
        kind,
        mapped: true,
    })
}

impl AddressSpace {
    pub fn new(mappings: Vec<Mapping>) -> Self {
        for mapping in &mappings {
            reserve_futex_id(mapping.futex_id);
        }
        Self {
            mappings,
            brk: BRK_BASE,
        }
    }

    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }

    pub fn futex_key(&self, address: u64) -> Result<(u64, u64), i64> {
        let end = address.checked_add(4).ok_or(EFAULT)?;
        let mapping = self
            .mappings
            .iter()
            .find(|mapping| {
                address >= mapping.address
                    && end <= mapping.address.saturating_add(mapping.size)
                    && mapping.rights & (2 | 4) == (2 | 4)
            })
            .ok_or(EFAULT)?;
        Ok((
            mapping.futex_id,
            mapping
                .vmo_offset
                .checked_add(address - mapping.address)
                .ok_or(EFAULT)?,
        ))
    }

    pub fn fork(&self, share: bool) -> Result<Self, i64> {
        let mut ranges: Vec<(u64, u64, u64)> = Vec::new();
        if !share {
            for mapping in &self.mappings {
                if matches!(mapping.kind, MappingKind::File { shared: true, .. }) {
                    continue;
                }
                let handle = mapping.futex_id;
                let end = mapping.vmo_offset.checked_add(mapping.size).ok_or(ENOMEM)?;
                if let Some((_, base, limit)) = ranges
                    .iter_mut()
                    .find(|(candidate, _, _)| *candidate == handle)
                {
                    *base = (*base).min(mapping.vmo_offset);
                    *limit = (*limit).max(end);
                } else {
                    ranges.push((handle, mapping.vmo_offset, end));
                }
            }
        }
        let snapshots = ranges
            .into_iter()
            .map(|(handle, base, end)| {
                let parent = self
                    .mappings
                    .iter()
                    .find(|mapping| mapping.futex_id == handle)
                    .ok_or(ENOMEM)?;
                let snapshot = Arc::new(
                    parent
                        .vmo
                        .snapshot(base, end.checked_sub(base).ok_or(ENOMEM)?)
                        .map_err(|_| ENOMEM)?,
                );
                Ok((handle, base, snapshot, new_futex_id()))
            })
            .collect::<Result<Vec<_>, i64>>()?;
        let mut mappings = Vec::with_capacity(self.mappings.len());
        for mapping in &self.mappings {
            let shared_mapping =
                share || matches!(mapping.kind, MappingKind::File { shared: true, .. });
            let (vmo, vmo_offset, futex_id) = if shared_mapping {
                (mapping.vmo.clone(), mapping.vmo_offset, mapping.futex_id)
            } else {
                let handle = mapping.futex_id;
                let (_, base, snapshot, futex_id) = snapshots
                    .iter()
                    .find(|(candidate, _, _, _)| *candidate == handle)
                    .ok_or(ENOMEM)?;
                (snapshot.clone(), mapping.vmo_offset - *base, *futex_id)
            };
            mappings.push(Mapping {
                vmo,
                futex_id,
                address: mapping.address,
                size: mapping.size,
                vmo_offset,
                rights: mapping.rights,
                kind: mapping.kind,
                mapped: false,
            });
        }
        Ok(Self {
            mappings,
            brk: self.brk,
        })
    }

    pub fn deactivate(&mut self) -> Result<(), i64> {
        for mapping in &mut self.mappings {
            if mapping.mapped {
                Vmar::root_self()
                    .unmap(mapping.address, mapping.size)
                    .map_err(|_| EINVAL)?;
                mapping.mapped = false;
            }
        }
        Ok(())
    }

    pub fn activate(&mut self) -> Result<(), i64> {
        let mut activated: Vec<usize> = Vec::new();
        for index in 0..self.mappings.len() {
            if self.mappings[index].mapped {
                continue;
            }
            let result = {
                let mapping = &self.mappings[index];
                Vmar::root_self().map(
                    mapping.address,
                    &mapping.vmo,
                    mapping.vmo_offset,
                    mapping.size,
                    flags(mapping.rights, true),
                )
            };
            let Ok(mapped) = result else {
                for index in activated {
                    let old = &mut self.mappings[index];
                    if old.mapped {
                        let _ = Vmar::root_self().unmap(old.address, old.size);
                        old.mapped = false;
                    }
                }
                return Err(ENOMEM);
            };
            core::mem::forget(mapped);
            self.mappings[index].mapped = true;
            activated.push(index);
        }
        Ok(())
    }

    pub fn proc_maps(&self) -> String {
        let mut mappings: Vec<_> = self.mappings.iter().collect();
        mappings.sort_by_key(|mapping| mapping.address);
        let mut output = String::new();
        for mapping in mappings {
            let read = if mapping.rights & 2 != 0 { 'r' } else { '-' };
            let write = if mapping.rights & 4 != 0 { 'w' } else { '-' };
            let execute = if mapping.rights & 8 != 0 { 'x' } else { '-' };
            let shared = if matches!(mapping.kind, MappingKind::File { shared: true, .. }) {
                's'
            } else {
                'p'
            };
            let offset = match mapping.kind {
                MappingKind::File { file_offset, .. } => file_offset,
                _ => mapping.vmo_offset,
            };
            let name = match mapping.kind {
                MappingKind::Image => "[image]",
                MappingKind::Stack => "[stack]",
                MappingKind::Anonymous => "",
                MappingKind::File { .. } => "[file]",
                MappingKind::SignalTrampoline => "[vdso]",
            };
            use core::fmt::Write;
            let _ = writeln!(
                output,
                "{:012x}-{:012x} {read}{write}{execute}{shared} {:08x} 00:00 0 {name}",
                mapping.address,
                mapping.address.saturating_add(mapping.size),
                offset,
            );
        }
        output
    }

    pub fn into_mappings(mut self) -> Vec<Mapping> {
        std::mem::take(&mut self.mappings)
    }

    pub fn prepare_exec(&mut self) {
        let mappings = std::mem::take(&mut self.mappings);
        self.mappings = mappings
            .into_iter()
            .filter(|mapping| mapping.kind == MappingKind::SignalTrampoline)
            .collect();
        self.brk = BRK_BASE;
    }

    pub fn add_mapping(&mut self, mapping: Mapping) {
        self.mappings.push(mapping);
    }

    fn containing(&self, address: u64, length: u64, rights: u32) -> Option<&Mapping> {
        let end = address.checked_add(length)?;
        self.mappings.iter().find(|mapping| {
            mapping.rights & rights == rights
                && address >= mapping.address
                && end <= mapping.address.saturating_add(mapping.size)
        })
    }

    pub fn read(&self, address: u64, length: usize) -> Result<&[u8], i64> {
        self.containing(address, length as u64, 2).ok_or(EFAULT)?;
        Ok(unsafe { std::slice::from_raw_parts(address as *const u8, length) })
    }

    pub fn validate_write(&self, address: u64, length: usize) -> Result<(), i64> {
        self.containing(address, length as u64, 4)
            .map(|_| ())
            .ok_or(EFAULT)
    }

    pub fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), i64> {
        self.containing(address, bytes.len() as u64, 4)
            .ok_or(EFAULT)?;
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len()) };
        Ok(())
    }

    pub fn cstring(&self, address: u64, maximum: usize) -> Result<String, i64> {
        let mapping = self.containing(address, 1, 2).ok_or(EFAULT)?;
        let available = (mapping.address + mapping.size - address) as usize;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                address as *const u8,
                available.min(maximum.saturating_add(1)),
            )
        };
        let end = bytes.iter().position(|byte| *byte == 0).ok_or(EINVAL)?;
        core::str::from_utf8(&bytes[..end])
            .map(str::to_string)
            .map_err(|_| EINVAL)
    }

    pub fn map_anonymous(
        &mut self,
        address: u64,
        length: u64,
        prot: u64,
        kind: MappingKind,
        fixed: bool,
    ) -> Result<u64, i64> {
        let size = round(length)?;
        if size == 0 || (fixed && (address == 0 || address & (PAGE_SIZE - 1) != 0)) {
            return Err(EINVAL);
        }
        let rights = linux_rights(prot)?;
        let vmo = Arc::new(Vmo::create(size).map_err(|_| ENOMEM)?);
        let mapped = Vmar::root_self()
            .map(
                if fixed { address } else { 0 },
                &vmo,
                0,
                size,
                flags(rights, fixed),
            )
            .map_err(|_| ENOMEM)?;
        let address = mapped.address();
        core::mem::forget(mapped);
        if rights & 4 != 0 {
            if Memory::commit_range(address, size).is_err() {
                let _ = Vmar::root_self().unmap(address, size);
                return Err(ENOMEM);
            }
        }
        self.mappings.push(Mapping {
            vmo,
            futex_id: new_futex_id(),
            address,
            size,
            vmo_offset: 0,
            rights,
            kind,
            mapped: true,
        });
        Ok(address)
    }

    pub fn map_file_copy(
        &mut self,
        address: u64,
        length: u64,
        prot: u64,
        fd: i32,
        file_offset: u64,
        source_handle: u64,
        source_size: u64,
        shared: bool,
        fixed: bool,
    ) -> Result<u64, i64> {
        let size = round(length)?;
        let target = self.map_anonymous(
            address,
            size,
            prot | 2,
            MappingKind::File {
                fd,
                file_offset,
                shared,
            },
            fixed,
        )?;
        let result = (|| {
            let copy = length.min(source_size.saturating_sub(file_offset));
            if copy != 0 {
                let delta = file_offset & (PAGE_SIZE - 1);
                let mapped_size = round(copy.checked_add(delta).ok_or(ENOMEM)?)?;
                let source = Memory::map_at(source_handle, file_offset - delta, mapped_size, 0, 2)
                    .map_err(|_| EACCES)?;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (source + delta) as *const u8,
                        target as *mut u8,
                        copy as usize,
                    );
                }
                let _ = Memory::unmap(source, mapped_size);
            }
            if prot & 2 == 0 {
                self.protect(target, size, prot)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = self.unmap(target, size);
            return Err(error);
        }
        Ok(target)
    }

    pub fn unmap(&mut self, address: u64, length: u64) -> Result<(), i64> {
        if address & (PAGE_SIZE - 1) != 0 || length == 0 {
            return Err(EINVAL);
        }
        self.rewrite_range(address, round(length)?, None, false)
    }

    pub fn protect(&mut self, address: u64, length: u64, prot: u64) -> Result<(), i64> {
        if address & (PAGE_SIZE - 1) != 0 || length == 0 {
            return Err(EINVAL);
        }
        self.rewrite_range(address, round(length)?, Some(linux_rights(prot)?), true)
    }

    pub fn remap(
        &mut self,
        old_address: u64,
        old_length: u64,
        new_length: u64,
        remap_flags: u64,
        new_address: u64,
    ) -> Result<u64, i64> {
        const MAYMOVE: u64 = 1;
        const FIXED: u64 = 2;
        const DONTUNMAP: u64 = 4;

        if remap_flags & !(MAYMOVE | FIXED | DONTUNMAP) != 0
            || remap_flags & FIXED != 0 && remap_flags & MAYMOVE == 0
            || remap_flags & DONTUNMAP != 0
        {
            return Err(if remap_flags & DONTUNMAP != 0 {
                ENOTSUP
            } else {
                EINVAL
            });
        }
        if old_address & (PAGE_SIZE - 1) != 0
            || old_length == 0
            || new_length == 0
            || remap_flags & FIXED != 0 && (new_address == 0 || new_address & (PAGE_SIZE - 1) != 0)
        {
            return Err(EINVAL);
        }
        let old_size = round(old_length)?;
        let new_size = round(new_length)?;
        let old_end = old_address.checked_add(old_size).ok_or(EINVAL)?;
        let mapping = self
            .mappings
            .iter()
            .find(|mapping| {
                old_address >= mapping.address
                    && old_end <= mapping.address.saturating_add(mapping.size)
            })
            .ok_or(EFAULT)?;
        let rights = mapping.rights;
        let source_vmo = mapping.vmo.clone();
        let source_offset = mapping.vmo_offset + old_address - mapping.address;
        let kind = match mapping.kind {
            MappingKind::File {
                fd,
                file_offset,
                shared,
            } => MappingKind::File {
                fd,
                file_offset: file_offset + old_address - mapping.address,
                shared,
            },
            kind => kind,
        };

        if remap_flags & FIXED == 0 {
            if new_size <= old_size {
                if new_size < old_size {
                    self.unmap(old_address + new_size, old_size - new_size)?;
                }
                return Ok(old_address);
            }
            if remap_flags & MAYMOVE == 0 {
                return Err(ENOMEM);
            }
        } else {
            let new_end = new_address.checked_add(new_size).ok_or(EINVAL)?;
            if new_address < old_end && old_address < new_end {
                return Err(EINVAL);
            }
            self.unmap(new_address, new_size)?;
        }

        let target =
            self.map_anonymous(new_address, new_size, 3, kind, remap_flags & FIXED != 0)?;
        let copy = old_size.min(new_size);
        let source = match Memory::map_at(
            source_vmo.as_handle_ref().raw_handle(),
            source_offset,
            copy,
            0,
            2,
        ) {
            Ok(source) => source,
            Err(_) => {
                let _ = self.unmap(target, new_size);
                return Err(EFAULT);
            }
        };
        unsafe {
            core::ptr::copy_nonoverlapping(source as *const u8, target as *mut u8, copy as usize);
        }
        let _ = Memory::unmap(source, copy);
        if let Err(error) = self.protect(target, new_size, linux_protection(rights)) {
            let _ = self.unmap(target, new_size);
            return Err(error);
        }
        if let Err(error) = self.unmap(old_address, old_size) {
            let _ = self.unmap(target, new_size);
            return Err(error);
        }
        Ok(target)
    }

    fn rewrite_range(
        &mut self,
        address: u64,
        size: u64,
        rights: Option<u32>,
        require_mapping: bool,
    ) -> Result<(), i64> {
        let end = address.checked_add(size).ok_or(EINVAL)?;
        let mut touched = false;
        let mut rebuilt = Vec::new();
        for mapping in std::mem::take(&mut self.mappings) {
            let start = mapping.address;
            let mapping_end = start + mapping.size;
            if end <= start || address >= mapping_end {
                rebuilt.push(mapping);
                continue;
            }
            touched = true;
            let _ = Vmar::root_self().unmap(mapping.address, mapping.size);
            let vmo = mapping.vmo.clone();
            let futex_id = mapping.futex_id;
            let kind = mapping.kind;
            let old_rights = mapping.rights;
            let old_offset = mapping.vmo_offset;
            std::mem::forget(mapping);
            let before = address.saturating_sub(start).min(mapping_end - start);
            let cut_start = address.max(start);
            let cut_end = end.min(mapping_end);
            let after = mapping_end - cut_end;
            if before != 0 {
                rebuilt.push(remap(
                    vmo.clone(),
                    futex_id,
                    start,
                    before,
                    old_offset,
                    old_rights,
                    kind,
                )?);
            }
            if let Some(rights) = rights {
                rebuilt.push(remap(
                    vmo.clone(),
                    futex_id,
                    cut_start,
                    cut_end - cut_start,
                    old_offset + cut_start - start,
                    rights,
                    advance_kind(kind, cut_start - start),
                )?);
            }
            if after != 0 {
                rebuilt.push(remap(
                    vmo,
                    futex_id,
                    cut_end,
                    after,
                    old_offset + cut_end - start,
                    old_rights,
                    advance_kind(kind, cut_end - start),
                )?);
            }
        }
        self.mappings = rebuilt;
        if touched || !require_mapping {
            Ok(())
        } else {
            Err(EINVAL)
        }
    }

    pub fn brk(&mut self, requested: u64) -> u64 {
        if requested == 0 {
            return self.brk;
        }
        if !(BRK_BASE..=BRK_LIMIT).contains(&requested) {
            return self.brk;
        }
        let old_page = round(self.brk).unwrap_or(self.brk);
        let new_page = round(requested).unwrap_or(requested);
        let result = if new_page > old_page {
            self.map_anonymous(
                old_page,
                new_page - old_page,
                3,
                MappingKind::Anonymous,
                true,
            )
            .map(|_| ())
        } else if new_page < old_page {
            self.unmap(new_page, old_page - new_page)
        } else {
            Ok(())
        };
        if result.is_ok() {
            self.brk = requested;
        }
        self.brk
    }

    pub fn shared_dirty_ranges(&self, address: u64, length: u64) -> Vec<(i32, u64, &[u8])> {
        let end = address.saturating_add(length);
        self.mappings
            .iter()
            .filter_map(|mapping| {
                let MappingKind::File {
                    fd,
                    file_offset,
                    shared: true,
                } = mapping.kind
                else {
                    return None;
                };
                let start = mapping.address.max(address);
                let finish = (mapping.address + mapping.size).min(end);
                (start < finish).then(|| {
                    let offset = file_offset + start - mapping.address;
                    let bytes = unsafe {
                        std::slice::from_raw_parts(start as *const u8, (finish - start) as usize)
                    };
                    (fd, offset, bytes)
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_protection_rejects_write_execute() {
        assert_eq!(linux_rights(1), Ok(2));
        assert_eq!(linux_rights(2), Ok(4));
        assert_eq!(linux_rights(4), Ok(8));
        assert_eq!(linux_rights(6), Err(EACCES));
        assert_eq!(linux_rights(8), Err(EINVAL));
        assert_eq!(linux_protection(2), 1);
        assert_eq!(linux_protection(4), 2);
        assert_eq!(linux_protection(8), 4);
    }

    #[test]
    fn split_file_mappings_advance_the_file_offset() {
        assert_eq!(
            advance_kind(
                MappingKind::File {
                    fd: 7,
                    file_offset: 0x2000,
                    shared: true,
                },
                0x3000,
            ),
            MappingKind::File {
                fd: 7,
                file_offset: 0x5000,
                shared: true,
            }
        );
        assert_eq!(
            advance_kind(MappingKind::Anonymous, 0x3000),
            MappingKind::Anonymous
        );
    }

    #[test]
    fn page_rounding_is_checked() {
        assert_eq!(round(1), Ok(4096));
        assert_eq!(round(4096), Ok(4096));
        assert_eq!(round(u64::MAX), Err(ENOMEM));
    }
}
