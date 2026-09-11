//! Bounded access to a stopped domain's RAM and first-level translations.
//! Only the assigned RAM slice is readable; page-table entries cannot turn a
//! guest instruction fetch into a monitor or device read.

#[derive(Clone, Copy)]
pub enum Paging {
    Disabled,
    Long4 { root: u64 },
}

pub struct Memory<'a> {
    ram: &'a [u8],
}
impl<'a> Memory<'a> {
    pub fn new(ram: &'a [u8]) -> Self {
        Self { ram }
    }
    fn word(&self, address: u64) -> Option<u64> {
        let start = usize::try_from(address).ok()?;
        Some(u64::from_le_bytes(
            self.ram
                .get(start..start.checked_add(8)?)?
                .try_into()
                .ok()?,
        ))
    }
    pub fn translate(&self, paging: Paging, address: u64) -> Option<u64> {
        self.resolve(paging, address)
            .filter(|physical| *physical < self.ram.len() as u64)
    }
    /// Resolve a guest physical address for device emulation. Page-table reads
    /// stay bounded to RAM; the result still requires device authorization and
    /// must never be cast directly to a monitor pointer.
    pub fn resolve(&self, paging: Paging, address: u64) -> Option<u64> {
        let Paging::Long4 { root } = paging else {
            return (address < 1 << 52).then_some(address);
        };
        const MASK: u64 = 0x000f_ffff_ffff_f000;
        if ((address << 16) as i64 >> 16) as u64 != address || root & !(MASK | 0xfff) != 0 {
            return None;
        }
        let mut table = root & MASK;
        for (level, shift) in [39, 30, 21, 12].into_iter().enumerate() {
            let entry = self.word(table.checked_add(((address >> shift) & 511) * 8)?)?;
            // This walker supports the 52-bit physical-address format and NX.
            // Reserved high bits or PS at PML4 cannot designate host addresses.
            if entry & 1 == 0
                || entry & 0x7ff0_0000_0000_0000 != 0
                || level == 0 && entry & 128 != 0
            {
                return None;
            }
            if level == 3 || level >= 1 && entry & 128 != 0 {
                let offset = (1u64 << shift) - 1;
                // Bit 12 is PAT for a huge page; all other low address bits
                // must be zero. Silently rounding them hides malformed tables.
                if level < 3 && entry & MASK & offset & !0x1000 != 0 {
                    return None;
                }
                let physical = (entry & MASK & !offset) | (address & offset);
                return Some(physical);
            }
            table = entry & MASK;
        }
        None
    }
    pub fn read(&self, paging: Paging, address: u64) -> Option<u8> {
        let physical = self.translate(paging, address)?;
        self.ram.get(usize::try_from(physical).ok()?).copied()
    }
    pub fn instruction(&self, paging: Paging, address: u64) -> ([u8; 15], usize) {
        let mut bytes = [0; 15];
        let mut length = 0;
        for (index, byte) in bytes.iter_mut().enumerate() {
            let Some(value) = address
                .checked_add(index as u64)
                .and_then(|p| self.read(paging, p))
            else {
                break;
            };
            *byte = value;
            length += 1;
        }
        (bytes, length)
    }
}

#[cfg(test)]
#[path = "guest_memory_tests.rs"]
mod tests;
