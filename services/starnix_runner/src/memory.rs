use bexos_userspace::Memory;
use bexos_zircon::{Vmar, VmarFlags, Vmo};
use starnix_kernel::{EACCES, EFAULT, EINVAL, ENOMEM};
use std::{sync::Arc, vec::Vec};

pub const PAGE_SIZE: u64 = 4096;
const BRK_BASE: u64 = 0x30_0000_0000;
const BRK_LIMIT: u64 = BRK_BASE + 256 * 1024 * 1024;

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

pub struct Mapping {
    pub vmo: Arc<Vmo>,
    pub address: u64,
    pub size: u64,
    pub vmo_offset: u64,
    pub rights: u32,
    pub kind: MappingKind,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = Vmar::root_self().unmap(self.address, self.size);
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

fn remap(
    vmo: Arc<Vmo>,
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
        address,
        size,
        vmo_offset: offset,
        rights,
        kind,
    })
}

impl AddressSpace {
    pub fn new(mappings: Vec<Mapping>) -> Self {
        Self {
            mappings,
            brk: BRK_BASE,
        }
    }

    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }

    pub fn into_mappings(mut self) -> Vec<Mapping> {
        std::mem::take(&mut self.mappings)
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
            address,
            size,
            vmo_offset: 0,
            rights,
            kind,
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
        self.rewrite_range(address, round(length)?, None)
    }

    pub fn protect(&mut self, address: u64, length: u64, prot: u64) -> Result<(), i64> {
        if address & (PAGE_SIZE - 1) != 0 || length == 0 {
            return Err(EINVAL);
        }
        self.rewrite_range(address, round(length)?, Some(linux_rights(prot)?))
    }

    fn rewrite_range(&mut self, address: u64, size: u64, rights: Option<u32>) -> Result<(), i64> {
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
                    cut_start,
                    cut_end - cut_start,
                    old_offset + cut_start - start,
                    rights,
                    kind,
                )?);
            }
            if after != 0 {
                rebuilt.push(remap(
                    vmo,
                    cut_end,
                    after,
                    old_offset + cut_end - start,
                    old_rights,
                    kind,
                )?);
            }
        }
        self.mappings = rebuilt;
        if touched { Ok(()) } else { Err(EINVAL) }
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
    }

    #[test]
    fn page_rounding_is_checked() {
        assert_eq!(round(1), Ok(4096));
        assert_eq!(round(4096), Ok(4096));
        assert_eq!(round(u64::MAX), Err(ENOMEM));
    }
}
