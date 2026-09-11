extern crate std;
use super::*;
use std::vec;
fn write(ram: &mut [u8], offset: usize, value: u64) {
    ram[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
#[test]
fn translates_cross_page_instructions_without_assuming_contiguous_backing() {
    let mut ram = vec![0u8; 0x8000];
    for (offset, value) in [
        (0x1000, 0x2007),
        (0x2000, 0x3007),
        (0x3000, 0x4007),
        (0x4000, 0x6007),
        (0x4008, 0x5007),
    ] {
        write(&mut ram, offset, value);
    }
    ram[0x6fff] = 0x0f;
    ram[0x5000] = 0x32;
    let memory = Memory::new(&ram);
    let (code, length) = memory.instruction(Paging::Long4 { root: 0x1000 }, 0xfff);
    assert_eq!(length, 15);
    assert_eq!(&code[..2], &[0x0f, 0x32]);
}
#[test]
fn stops_at_unmapped_boundary_without_reading_another_domain() {
    let ram = [0xf4; 4];
    let (bytes, length) = Memory::new(&ram).instruction(Paging::Disabled, 3);
    assert_eq!(length, 1);
    assert_eq!(bytes[0], 0xf4);
    assert_eq!(Memory::new(&ram).read(Paging::Disabled, u64::MAX), None);
    assert_eq!(
        Memory::new(&ram).read(Paging::Long4 { root: 0x1000 }, 0),
        None
    );
}
#[test]
fn rejects_reserved_bits_misalignment_and_outside_bank_page_tables() {
    let mut ram = vec![0u8; 0x5000];
    for (offset, value) in [(0x1000, 0x2007), (0x2000, 0x3007), (0x3000, 0x87)] {
        write(&mut ram, offset, value);
    }
    let paging = Paging::Long4 { root: 0x1000 };
    assert_eq!(Memory::new(&ram).translate(paging, 0x100), Some(0x100));
    assert_eq!(Memory::new(&ram).translate(paging, 1 << 48), None);
    for value in [0x2087, 0x200087, (1u64 << 52) | 0x87, 0] {
        write(&mut ram, 0x3000, value);
        assert_eq!(Memory::new(&ram).translate(paging, 0x100), None);
    }
    write(&mut ram, 0x1000, 0x1000007);
    assert_eq!(Memory::new(&ram).translate(paging, 0), None);
}
