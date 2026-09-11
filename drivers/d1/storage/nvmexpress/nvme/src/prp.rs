use bexos_d1_linux_shim::page::PAGE_SIZE;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrpError {
    Empty,
    CrossesTooManyPages,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrpList {
    pub prp1: u64,
    pub prp2: u64,
    pub pages: usize,
}

pub fn build_prp(phys: u64, len: usize) -> Result<PrpList, PrpError> {
    if len == 0 {
        return Err(PrpError::Empty);
    }

    let first_page_remaining = PAGE_SIZE - (phys as usize % PAGE_SIZE);
    if len <= first_page_remaining {
        return Ok(PrpList {
            prp1: phys,
            prp2: 0,
            pages: 1,
        });
    }

    let second =
        (phys + first_page_remaining as u64 + PAGE_SIZE as u64 - 1) & !((PAGE_SIZE as u64) - 1);
    let remaining = len - first_page_remaining;
    if remaining > PAGE_SIZE {
        return Err(PrpError::CrossesTooManyPages);
    }

    Ok(PrpList {
        prp1: phys,
        prp2: second,
        pages: 2,
    })
}
