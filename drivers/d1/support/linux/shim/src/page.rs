pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

pub const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysAddr(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Page {
    pub phys: PhysAddr,
}
