//! Exercise actual QEMU EDU bus-master transactions through VT-d. A unit test
//! of page-table bits cannot establish that device DMA is confined.
use bexos_secure_monitor::{
    clock, dma_tables::DmaTables, iommu, npt::Page, npt_tables::RamBank, pci_config,
};
static mut TABLES: DmaTables = DmaTables::empty();
static mut SENTINEL: Page = Page::zeroed();
const DEVICE: u8 = 5 << 3;
const BAR: u64 = 0xd0000000;
unsafe fn dma(source: u64, destination: u64, command: u64) {
    unsafe {
        for (offset, value) in [
            (0x80, source),
            (0x88, destination),
            (0x90, 64),
            (0x98, command),
        ] {
            core::ptr::write_volatile((BAR + offset) as *mut u64, value);
        }
        let start = clock::now_ns();
        while core::ptr::read_volatile((BAR + 0x98) as *const u64) & 1 != 0 {
            assert!(clock::now_ns().saturating_sub(start) < 2_000_000_000);
        }
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}
pub unsafe fn verify() {
    unsafe {
        assert!(clock::initialize());
        assert!(pci_config::quiesce_root_bus());
        assert_eq!(pci_config::read(DEVICE, 0), 0x11e81234);
        let tables = &mut *core::ptr::addr_of_mut!(TABLES);
        let bank = core::ptr::addr_of!(crate::GUEST) as u64;
        tables
            .initialize(
                tables as *const _ as u64,
                RamBank {
                    start: bank,
                    length: 0x200000,
                },
                &[DEVICE],
            )
            .unwrap();
        assert!(iommu::initialize(tables));
        crate::log("svm-probe: DMA and interrupt remapping enabled\n");
        pci_config::write(DEVICE, 0x10, BAR as u32);
        pci_config::write(DEVICE, 4, 0x406); // Polling, memory decode and bus master.
        let ram = core::slice::from_raw_parts_mut(bank as *mut u8, 0x200000);
        for index in 0..64 {
            ram[0x10000 + index] = (index as u8) ^ 0xa5;
        }
        ram[0x11000..0x11040].fill(0);
        dma(0x10000, 0x40000, 1);
        assert!(iommu::fault().is_none());
        dma(0x40000, 0x11000, 3);
        assert!(iommu::fault().is_none());
        assert_eq!(&ram[0x10000..0x10040], &ram[0x11000..0x11040]);
        crate::log("svm-probe: assigned device DMA translated into its RAM bank\n");
        for address in [core::ptr::addr_of!(SENTINEL) as u64, 0x20000000] {
            core::ptr::write_bytes(address as *mut u8, 0x5a, 64);
            dma(0x40000, address, 3);
            let Some((fault, requester, reason)) = iommu::fault() else {
                panic!("missing DMA rejection record");
            };
            assert_eq!(fault, address & !4095);
            assert_eq!(requester, u16::from(DEVICE));
            assert_ne!(reason, 0);
            for offset in 0..64 {
                assert_eq!(
                    core::ptr::read_volatile((address + offset) as *const u8),
                    0x5a
                );
            }
            iommu::clear_fault();
        }
        pci_config::write(DEVICE, 4, 0x400);
        crate::log("svm-probe: monitor and secure-domain DMA writes explicitly rejected\n");
    }
}
