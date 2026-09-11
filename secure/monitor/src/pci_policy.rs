//! Address policy for assigned PCI BARs. Probing is virtual and cannot move a
//! physical device over protected RAM, firmware, or monitor-owned controllers.
#[derive(Clone, Copy, Default)]
pub struct Bar {
    pub base: u64,
    pub length: u64,
    pub flags: u8,
    pub probe: u8,
}
impl Bar {
    pub fn new(length: u64, flags: u8) -> Option<Self> {
        if length == 0
            || !length.is_power_of_two()
            || length > 0x1000000
            || flags & 1 != 0
            || !matches!(flags & 6, 0 | 4)
        {
            return None;
        }
        Some(Self {
            base: 0,
            length,
            flags,
            probe: 0,
        })
    }
    pub fn wide(self) -> bool {
        self.flags & 6 == 4
    }
    pub fn read(self, high: bool) -> u32 {
        let value = if self.probe & (1 << u8::from(high)) != 0 {
            !(self.length - 1)
        } else {
            self.base
        };
        if high {
            (value >> 32) as u32
        } else {
            value as u32 & !15 | u32::from(self.flags)
        }
    }
    pub fn changed(self, high: bool, value: u32) -> Option<Self> {
        if high && !self.wide() {
            return None;
        }
        let mut bar = self;
        if value == u32::MAX {
            bar.probe |= 1 << u8::from(high);
            return Some(bar);
        }
        bar.probe &= !(1 << u8::from(high));
        bar.base = if high {
            (bar.base & 0xffffffff) | (u64::from(value) << 32)
        } else {
            (bar.base & !0xffffffff) | u64::from(value & !15)
        };
        let end = bar.base.checked_add(bar.length)?;
        if bar.base != 0
            && (bar.base < 0xc0000000
                || end > 0xfebf0000
                || bar.base & (bar.length - 1) != 0
                || bar.base < 0xe0100000 && end > 0xe0000000)
        {
            return None;
        }
        Some(bar)
    }
    pub fn contains(self, address: u64, bytes: u8) -> bool {
        self.base != 0
            && matches!(bytes, 1 | 2 | 4 | 8)
            && address & (u64::from(bytes) - 1) == 0
            && address >= self.base
            && address
                .checked_add(u64::from(bytes))
                .is_some_and(|end| end <= self.base + self.length)
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.base != 0
            && other.base != 0
            && self.base < other.base + other.length
            && other.base < self.base + self.length
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sizing_never_changes_assignment_and_protected_addresses_are_denied() {
        let original = Bar::new(0x4000, 4)
            .unwrap()
            .changed(false, 0xc0000004)
            .unwrap();
        let probe = original
            .changed(false, u32::MAX)
            .unwrap()
            .changed(true, u32::MAX)
            .unwrap();
        assert_eq!(probe.base, original.base);
        assert_eq!(probe.read(false), 0xffffc004);
        assert_eq!(probe.read(true), u32::MAX);
        for base in [
            0x4000000, 0x20000000, 0x40000000, 0xe0000000, 0xfed90000, 0xffc00000, 0xc0001000,
        ] {
            assert!(original.changed(false, base).is_none());
        }
        assert!(original.changed(true, 1).is_none());
        assert!(original.contains(0xc0003ff8, 8));
        assert!(!original.contains(0xc0003ffc, 8));
        assert!(
            original.overlaps(
                Bar::new(4096, 0)
                    .unwrap()
                    .changed(false, 0xc0001000)
                    .unwrap()
            )
        );
        assert!(
            !original.overlaps(
                Bar::new(4096, 0)
                    .unwrap()
                    .changed(false, 0xc0004000)
                    .unwrap()
            )
        );
    }
}
