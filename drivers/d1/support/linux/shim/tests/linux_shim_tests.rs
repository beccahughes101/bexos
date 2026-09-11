use bexos_d1_linux_shim::dma::{DmaAllocator, MockDmaAllocator};
use bexos_d1_linux_shim::mmio::{Mmio, MockMmioBar};
use bexos_d1_linux_shim::page::PAGE_SIZE;

#[test]
fn dma_allocator_returns_page_aligned_coherent_buffers() {
    let mut allocator = MockDmaAllocator::new(0x5000_0123);
    let buffer = allocator
        .allocate_coherent(512, 64)
        .expect("dma allocation");

    assert_eq!(buffer.phys().0 as usize % PAGE_SIZE, 0);
    assert_eq!(buffer.len(), PAGE_SIZE);
    assert!(buffer.is_coherent());
}

#[test]
fn mock_mmio_tracks_32_and_64_bit_registers() {
    let mut bar = MockMmioBar::new(0x100);

    bar.write32(0x10, 0xaabb_ccdd).expect("write32");
    assert_eq!(bar.read32(0x10).expect("read32"), 0xaabb_ccdd);

    bar.write64(0x20, 0x1122_3344_5566_7788).expect("write64");
    assert_eq!(bar.read64(0x20).expect("read64"), 0x1122_3344_5566_7788);
}
