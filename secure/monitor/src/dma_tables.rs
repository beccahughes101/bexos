//! Legacy VT-d root/context and three-level DMA translation tables. The owner
//! selects requester IDs and a single assigned RAM bank. Guest configuration
//! writes never receive a pointer to these tables.
use crate::npt_tables::RamBank;
#[repr(C, align(4096))]
struct Table([u64; 512]);
impl Table {
    const fn empty() -> Self {
        Self([0; 512])
    }
}
#[repr(C, align(4096))]
pub struct DmaTables {
    root: Table,
    context: Table,
    pdpt: Table,
    directory: Table,
    // Every interrupt-remapping entry is non-present. The polling device
    // backend cannot send a guest-selected MSI to a physical processor.
    interrupts: Table,
}
impl DmaTables {
    pub const fn empty() -> Self {
        Self {
            root: Table::empty(),
            context: Table::empty(),
            pdpt: Table::empty(),
            directory: Table::empty(),
            interrupts: Table::empty(),
        }
    }
    pub fn initialize(
        &mut self,
        physical: u64,
        bank: RamBank,
        requesters: &[u8],
    ) -> Result<(), ()> {
        let end = bank.start.checked_add(bank.length).ok_or(())?;
        let table_end = physical
            .checked_add(core::mem::size_of::<Self>() as u64)
            .ok_or(())?;
        if physical == 0
            || physical & 4095 != 0
            || table_end > 1 << 48
            || bank.start == 0
            || bank.length == 0
            || bank.length > 1 << 30
            || (bank.start | bank.length) & 0x1fffff != 0
            || end > 1 << 48
            || (physical < end && bank.start < table_end)
            || requesters.is_empty()
        {
            return Err(());
        }
        for (i, requester) in requesters.iter().enumerate() {
            if requesters[..i].contains(requester) {
                return Err(());
            }
        }
        *self = Self::empty();
        self.root.0[0] = (physical + 4096) | 1;
        for requester in requesters {
            let index = usize::from(*requester) * 2;
            self.context.0[index] = (physical + 8192) | 1;
            // Translation type 0, domain 1, adjusted width 39 bits (3 levels).
            self.context.0[index + 1] = (1 << 8) | 1;
        }
        self.pdpt.0[0] = (physical + 12288) | 3;
        for page in 0..bank.length / 0x200000 {
            self.directory.0[page as usize] = (bank.start + page * 0x200000) | 0x83;
        }
        Ok(())
    }
    pub fn interrupt_table(&self, physical: u64) -> u64 {
        physical + 16384
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_assigned_requesters_and_ram_have_present_entries() {
        let mut tables = DmaTables::empty();
        tables
            .initialize(
                0x4000000,
                RamBank {
                    start: 0x40000000,
                    length: 0x30000000,
                },
                &[24, 32, 40],
            )
            .unwrap();
        assert_eq!(tables.root.0[0], 0x4001001);
        assert!(tables.root.0[1..].iter().all(|v| *v == 0));
        for requester in 0..256 {
            assert_eq!(
                tables.context.0[requester * 2] & 1,
                u64::from([24, 32, 40].contains(&requester))
            );
        }
        assert_eq!(tables.directory.0[0], 0x40000083);
        assert_eq!(tables.directory.0[383], 0x6fe00083);
        assert!(tables.directory.0[384..].iter().all(|v| *v == 0));
        assert!(tables.interrupts.0.iter().all(|v| *v == 0));
        assert!(
            tables
                .initialize(
                    0x40000000,
                    RamBank {
                        start: 0x40000000,
                        length: 0x200000
                    },
                    &[8]
                )
                .is_err()
        );
        assert!(
            tables
                .initialize(
                    0x4000000,
                    RamBank {
                        start: 0x40000000,
                        length: 0x200000
                    },
                    &[8, 8]
                )
                .is_err()
        );
    }
}
