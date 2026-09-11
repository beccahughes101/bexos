use alloc::vec::Vec;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciId {
    pub vendor_id: u16,
    pub device_id: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarResource {
    pub index: u8,
    pub base: u64,
    pub size: u64,
    pub prefetchable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PciDevice {
    pub id: PciId,
    pub bars: Vec<BarResource>,
    pub irq_vectors: Vec<u32>,
}

impl PciDevice {
    pub fn bar(&self, index: u8) -> Option<BarResource> {
        self.bars.iter().copied().find(|bar| bar.index == index)
    }
}
