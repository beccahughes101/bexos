pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const PAGE_SIZE: usize = 4096;
pub const TABLE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;
pub const BLOCK_1G_SIZE: u64 = 1024 * 1024 * 1024;
pub const BLOCK_2M_SIZE: u64 = 2 * 1024 * 1024;

const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE_OR_PAGE: u64 = 1 << 1;
const DESC_AF: u64 = 1 << 10;
const DESC_INNER_SHAREABLE: u64 = 0b11 << 8;
const DESC_PXN: u64 = 1 << 53;
const DESC_UXN: u64 = 1 << 54;
const DESC_AP_USER: u64 = 0b01 << 6;
const DESC_AP_READ_ONLY: u64 = 0b10 << 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAttr {
    Device,
    Normal,
}

impl MemoryAttr {
    pub const fn mair_index(self) -> u64 {
        match self {
            Self::Device => 0,
            Self::Normal => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    KernelReadWrite,
    KernelReadOnly,
    KernelUserReadOnly,
    KernelUserReadWrite,
}

impl Access {
    const fn ap_bits(self) -> u64 {
        match self {
            Self::KernelReadWrite => 0b00 << 6,
            Self::KernelReadOnly => 0b10 << 6,
            Self::KernelUserReadOnly => DESC_AP_USER | DESC_AP_READ_ONLY,
            Self::KernelUserReadWrite => 0b01 << 6,
        }
    }
}

pub const fn table_descriptor(next_table_phys: u64) -> u64 {
    (next_table_phys & TABLE_ADDR_MASK) | DESC_VALID | DESC_TABLE_OR_PAGE
}

pub const fn block_descriptor_1g(phys: u64, attr: MemoryAttr, access: Access) -> u64 {
    (phys & !(BLOCK_1G_SIZE - 1))
        | DESC_VALID
        | (attr.mair_index() << 2)
        | access.ap_bits()
        | shareability_bits(attr)
        | DESC_AF
        | execute_never_bits(attr)
}

pub const fn block_descriptor_2m(phys: u64, attr: MemoryAttr, access: Access) -> u64 {
    (phys & !(BLOCK_2M_SIZE - 1))
        | DESC_VALID
        | (attr.mair_index() << 2)
        | access.ap_bits()
        | shareability_bits(attr)
        | DESC_AF
        | execute_never_bits(attr)
}

pub const fn page_descriptor(phys: u64, attr: MemoryAttr, access: Access) -> u64 {
    (phys & TABLE_ADDR_MASK)
        | DESC_VALID
        | DESC_TABLE_OR_PAGE
        | (attr.mair_index() << 2)
        | access.ap_bits()
        | shareability_bits(attr)
        | DESC_AF
        | execute_never_bits(attr)
}

pub const fn kernel_page_descriptor(
    phys: u64,
    attr: MemoryAttr,
    access: Access,
    executable: bool,
) -> u64 {
    let desc = page_descriptor(phys, attr, access) | DESC_UXN;
    if executable { desc } else { desc | DESC_PXN }
}

const fn shareability_bits(attr: MemoryAttr) -> u64 {
    match attr {
        MemoryAttr::Device => 0,
        MemoryAttr::Normal => DESC_INNER_SHAREABLE,
    }
}

const fn execute_never_bits(attr: MemoryAttr) -> u64 {
    match attr {
        MemoryAttr::Device => DESC_PXN | DESC_UXN,
        MemoryAttr::Normal => 0,
    }
}

pub const fn user_data_page_descriptor(phys: u64) -> u64 {
    page_descriptor(phys, MemoryAttr::Normal, Access::KernelUserReadWrite) | DESC_PXN | DESC_UXN
}

pub const fn user_code_page_descriptor(phys: u64) -> u64 {
    page_descriptor(phys, MemoryAttr::Normal, Access::KernelUserReadOnly) | DESC_PXN
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserPageMapping {
    pub vaddr: u64,
    pub phys: u64,
    pub executable: bool,
    pub writable: bool,
}

impl UserPageMapping {
    pub const fn descriptor(self) -> u64 {
        if self.executable {
            user_code_page_descriptor(self.phys)
        } else {
            user_data_page_descriptor(self.phys)
        }
    }
}
