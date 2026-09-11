//! Minimal ACPI tables describing only the domain's virtual interrupt/timer
//! devices. No host firmware table or protected physical address is copied.
const RSDP: usize = 0xe0000;
const RSDT: usize = 0xe0100;
const MADT: usize = 0xe0200;
const HPET: usize = 0xe0400;
const MCFG: usize = 0xe0500;
fn checksum(bytes: &mut [u8], index: usize) {
    bytes[index] = 0;
    bytes[index] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
}
fn header(bytes: &mut [u8], signature: &[u8; 4]) {
    bytes.fill(0);
    bytes[..4].copy_from_slice(signature);
    let length = bytes.len() as u32;
    bytes[4..8].copy_from_slice(&length.to_le_bytes());
    bytes[8] = 1;
    bytes[10..16].copy_from_slice(b"BEXOS ");
    bytes[16..24].copy_from_slice(b"SVMQ35  ");
    bytes[28..32].copy_from_slice(b"BEXO");
    bytes[24] = 1;
    bytes[32] = 1;
}
pub fn install(ram: &mut [u8], cpus: u8) -> bool {
    if !(1..=32).contains(&cpus) || ram.len() < MCFG + 60 {
        return false;
    }
    let rsdt = &mut ram[RSDT..RSDT + 48];
    header(rsdt, b"RSDT");
    for (index, address) in [MADT, HPET, MCFG].into_iter().enumerate() {
        rsdt[36 + index * 4..40 + index * 4].copy_from_slice(&(address as u32).to_le_bytes());
    }
    checksum(rsdt, 9);
    let madt = &mut ram[MADT..MADT + 44 + usize::from(cpus) * 8 + 12];
    header(madt, b"APIC");
    madt[36..40].copy_from_slice(&0xfee00000u32.to_le_bytes());
    for id in 0..cpus {
        let offset = 44 + usize::from(id) * 8;
        madt[offset..offset + 8].copy_from_slice(&[0, 8, id, id, 1, 0, 0, 0]);
    }
    let offset = 44 + usize::from(cpus) * 8;
    madt[offset..offset + 12].copy_from_slice(&[1, 12, cpus, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0]);
    checksum(madt, 9);
    let hpet = &mut ram[HPET..HPET + 56];
    header(hpet, b"HPET");
    hpet[36..40].copy_from_slice(&0x80862001u32.to_le_bytes());
    hpet[41] = 64;
    hpet[44..52].copy_from_slice(&0xfed00000u64.to_le_bytes());
    hpet[53..55].copy_from_slice(&1u16.to_le_bytes());
    checksum(hpet, 9);
    let mcfg = &mut ram[MCFG..MCFG + 60];
    header(mcfg, b"MCFG");
    mcfg[44..52].copy_from_slice(&0xe0000000u64.to_le_bytes());
    // A single virtual bus. Absent devices return all ones through emulation.
    checksum(mcfg, 9);
    let rsdp = &mut ram[RSDP..RSDP + 20];
    rsdp.fill(0);
    rsdp[..8].copy_from_slice(b"RSD PTR ");
    rsdp[9..15].copy_from_slice(b"BEXOS ");
    rsdp[16..20].copy_from_slice(&(RSDT as u32).to_le_bytes());
    checksum(rsdp, 8);
    true
}
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    #[test]
    fn table_checksums_lengths_and_cpu_inventory_are_consistent() {
        let mut ram = vec![0xa5; 0x100000];
        assert!(install(&mut ram, 4));
        assert_eq!(
            ram[RSDP..RSDP + 20]
                .iter()
                .fold(0u8, |sum, b| sum.wrapping_add(*b)),
            0
        );
        for start in [RSDT, MADT, HPET, MCFG] {
            let length = u32::from_le_bytes(ram[start + 4..start + 8].try_into().unwrap()) as usize;
            assert_eq!(
                ram[start..start + length]
                    .iter()
                    .fold(0u8, |sum, b| sum.wrapping_add(*b)),
                0
            );
        }
        for id in 0..4 {
            assert_eq!(
                &ram[MADT + 44 + id * 8..MADT + 52 + id * 8],
                &[0, 8, id as u8, id as u8, 1, 0, 0, 0]
            );
        }
        assert!(!install(&mut ram, 0));
        assert!(!install(&mut ram, 33));
        assert!(!install(&mut [0; 20], 4));
        assert_eq!(ram[0], 0xa5);
    }
}
