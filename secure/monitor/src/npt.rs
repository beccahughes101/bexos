//! An explicit 2 MiB domain mapping for the SVM bring-up probe. No identity
//! mapping of monitor or other-domain memory is installed.

#[repr(C, align(4096))]
pub struct Page(pub [u64; 512]);
impl Page {
    pub const fn zeroed() -> Self {
        Self([0; 512])
    }
}

#[repr(C, align(4096))]
pub struct DomainMap {
    pub pml4: Page,
    pub pdpt: Page,
    pub pd: Page,
}

impl Default for DomainMap {
    fn default() -> Self {
        Self::new()
    }
}
impl DomainMap {
    pub const fn new() -> Self {
        Self {
            pml4: Page::zeroed(),
            pdpt: Page::zeroed(),
            pd: Page::zeroed(),
        }
    }
    /// Caller keeps this structure at a stable, identity-mapped host address.
    pub fn map_probe_memory(&mut self, host: u64) -> Result<(), InvalidMapping> {
        if host == 0 || host & ((1 << 21) - 1) != 0 || host >= 1 << 52 {
            return Err(InvalidMapping);
        }
        self.pml4.0.fill(0);
        self.pdpt.0.fill(0);
        self.pd.0.fill(0);
        self.pml4.0[0] = core::ptr::addr_of!(self.pdpt) as u64 | 7;
        self.pdpt.0[0] = core::ptr::addr_of!(self.pd) as u64 | 7;
        self.pd.0[0] = host | 0x87;
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMapping;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_domain_memory_is_mapped() {
        let mut map = DomainMap::new();
        assert_eq!(map.map_probe_memory(0x200001), Err(InvalidMapping));
        assert_eq!(map.map_probe_memory(1 << 52), Err(InvalidMapping));
        map.map_probe_memory(0x200000).unwrap();
        assert_eq!(map.pd.0[0], 0x200087);
        assert!(map.pd.0[1..].iter().all(|p| *p == 0));
        assert!(map.pdpt.0[1..].iter().all(|p| *p == 0));
        assert!(map.pml4.0[1..].iter().all(|p| *p == 0));
    }
}
