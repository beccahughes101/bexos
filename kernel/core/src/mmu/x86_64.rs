pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const PAGE_SIZE: usize = 4096;
pub const TABLE_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
pub const BLOCK_1G_SIZE: u64 = 1 << 30;
pub const BLOCK_2M_SIZE: u64 = 1 << 21;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAttr {
    Device,
    Normal,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    KernelReadWrite,
    KernelReadOnly,
    KernelUserReadOnly,
    KernelUserReadWrite,
}
pub const fn table_descriptor(physical: u64) -> u64 {
    (physical & TABLE_ADDR_MASK) | 7
}
pub const fn page_descriptor(physical: u64, attr: MemoryAttr, access: Access) -> u64 {
    (physical & TABLE_ADDR_MASK)
        | 1
        | (1 << 63)
        | match access {
            Access::KernelReadWrite => 2,
            Access::KernelReadOnly => 0,
            Access::KernelUserReadOnly => 4,
            Access::KernelUserReadWrite => 6,
        }
        | match attr {
            MemoryAttr::Normal => 0,
            MemoryAttr::Device => 0x18,
        }
}
pub const fn kernel_page_descriptor(
    physical: u64,
    attr: MemoryAttr,
    access: Access,
    executable: bool,
) -> u64 {
    let d = page_descriptor(physical, attr, access) & !4;
    if executable { d & !(1 << 63) } else { d }
}
pub const fn user_code_page_descriptor(physical: u64) -> u64 {
    page_descriptor(physical, MemoryAttr::Normal, Access::KernelUserReadOnly) & !(1 << 63)
}
pub const fn user_data_page_descriptor(physical: u64) -> u64 {
    page_descriptor(physical, MemoryAttr::Normal, Access::KernelUserReadWrite)
}
pub const fn block_descriptor_1g(physical: u64, attr: MemoryAttr, access: Access) -> u64 {
    page_descriptor(physical & !(BLOCK_1G_SIZE - 1), attr, access) | 128
}
pub const fn block_descriptor_2m(physical: u64, attr: MemoryAttr, access: Access) -> u64 {
    page_descriptor(physical & !(BLOCK_2M_SIZE - 1), attr, access) | 128
}
