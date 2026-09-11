//! Bounded, allocation-free nested page tables for isolated guest RAM.
//! Only the configured domain's backing bank can be mapped. Device mappings
//! require a separate monitor-owned device backend; this API cannot grant them.
use crate::npt::Page;

const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const USER: u64 = 4;
const LARGE: u64 = 1 << 7;
const NX: u64 = 1 << 63;
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Permissions {
    pub write: bool,
    pub execute: bool,
}
impl Permissions {
    pub const READ: Self = Self {
        write: false,
        execute: false,
    };
    pub const DATA: Self = Self {
        write: true,
        execute: false,
    };
    pub const CODE: Self = Self {
        write: false,
        execute: true,
    };
    /// Guest RAM may contain the guest's own code and data page tables. These
    /// permissions never extend beyond the domain's assigned backing bank.
    pub const RAM: Self = Self {
        write: true,
        execute: true,
    };
    fn bits(self) -> u64 {
        PRESENT | USER | if self.write { WRITE } else { 0 } | if self.execute { 0 } else { NX }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageSize {
    Small,
    Large,
}
impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Small => 4096,
            Self::Large => 2 * 1024 * 1024,
        }
    }
    fn level(self) -> usize {
        match self {
            Self::Small => 3,
            Self::Large => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapError {
    InvalidRange,
    OutsideDomain,
    Conflict,
    NoTables,
    Uninitialized,
}

/// A monitor-owned contiguous RAM assignment. No guest value selects this bank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RamBank {
    pub start: u64,
    pub length: u64,
}
impl RamBank {
    fn contains(self, start: u64, length: u64) -> bool {
        start >= self.start
            && start
                .checked_add(length)
                .is_some_and(|end| end <= self.start + self.length)
    }
}

/// The page array must remain at `root()` in host physical memory while any CPU
/// uses this address space. Mapping mutation requires all its vCPUs quiesced;
/// callers must flush the domain's TLB before resuming after any mutation.
#[repr(C, align(4096))]
pub struct PageTables<const N: usize> {
    pages: [Page; N],
    used: usize,
    physical: u64,
    bank: RamBank,
}
impl<const N: usize> PageTables<N> {
    pub const fn empty() -> Self {
        Self {
            pages: [const { Page::zeroed() }; N],
            used: 0,
            physical: 0,
            bank: RamBank {
                start: 0,
                length: 0,
            },
        }
    }

    /// Construct inactive tables. `physical` describes the monitor-owned page
    /// array, not a guest pointer. The RAM bank must exclude monitor table memory.
    /// This routine never dereferences the supplied physical address.
    pub fn initialize(&mut self, physical: u64, bank: RamBank) -> Result<(), MapError> {
        let length = (N as u64).checked_mul(4096).ok_or(MapError::InvalidRange)?;
        if N == 0
            || physical == 0
            || !valid_range(physical, length, 1 << 52)
            || !valid_range(bank.start, bank.length, 1 << 52)
        {
            return Err(MapError::InvalidRange);
        }
        if physical < bank.start + bank.length && bank.start < physical + length {
            return Err(MapError::OutsideDomain);
        }
        for page in &mut self.pages {
            page.0.fill(0);
        }
        self.used = 1;
        self.physical = physical;
        self.bank = bank;
        Ok(())
    }
    pub fn root(&self) -> Result<u64, MapError> {
        if self.used == 0 {
            Err(MapError::Uninitialized)
        } else {
            Ok(self.physical)
        }
    }
    pub fn allocated_tables(&self) -> usize {
        self.used
    }

    /// Install one mapping atomically. Invalid arguments, collisions and pool
    /// exhaustion leave all existing mappings and table allocations unchanged.
    pub fn map(
        &mut self,
        guest: u64,
        host: u64,
        size: PageSize,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        self.root()?;
        let bytes = size.bytes();
        if !valid_range(guest, bytes, 1 << 48) || (guest | host) & (bytes - 1) != 0 {
            return Err(MapError::InvalidRange);
        }
        if !self.bank.contains(host, bytes) {
            return Err(MapError::OutsideDomain);
        }
        let indices = indices(guest);
        let leaf_level = size.level();
        let mut table = 0;
        let mut missing = None;
        for (level, index) in indices.iter().copied().enumerate().take(leaf_level) {
            let entry = self.pages[table].0[index];
            if entry & PRESENT == 0 {
                missing = Some((level, table));
                break;
            }
            if entry & LARGE != 0 {
                return Err(MapError::Conflict);
            }
            table = self.table_index(entry)?;
        }
        if let Some((level, parent)) = missing {
            let required = leaf_level - level;
            if N - self.used < required {
                return Err(MapError::NoTables);
            }
            table = parent;
            for index in indices.iter().copied().take(leaf_level).skip(level) {
                let next = self.used;
                self.used += 1;
                self.pages[table].0[index] =
                    (self.physical + next as u64 * 4096) | PRESENT | WRITE | USER;
                table = next;
            }
        } else if self.pages[table].0[indices[leaf_level]] & PRESENT != 0 {
            return Err(MapError::Conflict);
        }
        self.pages[table].0[indices[leaf_level]] =
            host | permissions.bits() | if size == PageSize::Large { LARGE } else { 0 };
        Ok(())
    }

    pub fn translate(&self, guest: u64) -> Option<(u64, Permissions, PageSize)> {
        if self.used == 0 || guest >= 1 << 48 {
            return None;
        }
        let mut table = 0;
        for (level, index) in indices(guest).into_iter().enumerate() {
            let entry = self.pages[table].0[index];
            if entry & PRESENT == 0 {
                return None;
            }
            let size = if level == 2 && entry & LARGE != 0 {
                Some(PageSize::Large)
            } else if level == 3 {
                Some(PageSize::Small)
            } else {
                None
            };
            if let Some(size) = size {
                return Some((
                    (entry & ADDRESS & !(size.bytes() - 1)) | (guest & (size.bytes() - 1)),
                    Permissions {
                        write: entry & WRITE != 0,
                        execute: entry & NX == 0,
                    },
                    size,
                ));
            }
            table = self.table_index(entry).ok()?;
        }
        None
    }

    /// Revoke exactly one complete mapping. Partial large-page revocation is
    /// rejected; callers must choose small pages for independently shared RAM.
    pub fn unmap(&mut self, guest: u64, size: PageSize) -> Result<(), MapError> {
        self.root()?;
        if guest >= 1 << 48 || guest & (size.bytes() - 1) != 0 {
            return Err(MapError::InvalidRange);
        }
        let indices = indices(guest);
        let mut table = 0;
        for index in indices.iter().copied().take(size.level()) {
            let entry = self.pages[table].0[index];
            if entry & PRESENT == 0 || entry & LARGE != 0 {
                return Err(MapError::Conflict);
            }
            table = self.table_index(entry)?;
        }
        let leaf = &mut self.pages[table].0[indices[size.level()]];
        if *leaf & PRESENT == 0 || (size == PageSize::Large && *leaf & LARGE == 0) {
            return Err(MapError::Conflict);
        }
        *leaf = 0;
        Ok(())
    }
    fn table_index(&self, entry: u64) -> Result<usize, MapError> {
        let offset = (entry & ADDRESS)
            .checked_sub(self.physical)
            .ok_or(MapError::InvalidRange)?;
        let index = (offset / 4096) as usize;
        if index >= self.used {
            return Err(MapError::InvalidRange);
        }
        Ok(index)
    }
}
fn indices(address: u64) -> [usize; 4] {
    [39, 30, 21, 12].map(|shift| ((address >> shift) & 511) as usize)
}
fn valid_range(start: u64, length: u64, limit: u64) -> bool {
    length != 0
        && (start | length) & 4095 == 0
        && start.checked_add(length).is_some_and(|end| end <= limit)
}

#[cfg(test)]
#[path = "npt_tables_tests.rs"]
mod tests;
