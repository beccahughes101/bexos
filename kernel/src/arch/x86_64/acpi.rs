//! Q35 firmware discovery, completed while the bootstrap maps the low 4 GiB.
#[derive(Clone, Copy)]
pub struct Tables {
    pub lapic: u64,
    pub ioapic: u64,
    pub hpet: u64,
    pub ecam: u64,
    pub ids: [u8; 64],
    pub count: usize,
}
unsafe fn bytes(address: u64, len: usize) -> &'static [u8] {
    assert!(
        address
            .checked_add(len as u64)
            .is_some_and(|end| end <= 0x1_0000_0000)
    );
    unsafe { core::slice::from_raw_parts(address as *const u8, len) }
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn checksum(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)) == 0
}
unsafe fn table(address: u64) -> &'static [u8] {
    let h = unsafe { bytes(address, 36) };
    let len = word(h, 4) as usize;
    assert!((36..=1024 * 1024).contains(&len), "ACPI table length");
    let t = unsafe { bytes(address, len) };
    assert!(checksum(t), "ACPI table checksum");
    t
}
pub fn discover() -> Tables {
    let mut out = Tables {
        lapic: 0,
        ioapic: 0,
        hpet: 0,
        ecam: 0,
        ids: [0; 64],
        count: 0,
    };
    let rsdp = (0xe0000..0x100000)
        .step_by(16)
        .find(|address| {
            let h = unsafe { bytes(*address, 20) };
            &h[..8] == b"RSD PTR " && checksum(h)
        })
        .expect("Q35 ACPI RSDP");
    let r = unsafe { bytes(rsdp, 20) };
    let rsdt = unsafe { table(word(r, 16) as u64) };
    assert_eq!(&rsdt[..4], b"RSDT");
    for address in rsdt[36..].chunks_exact(4) {
        let t = unsafe { table(u32::from_le_bytes(address.try_into().unwrap()) as u64) };
        match &t[..4] {
            b"APIC" => {
                assert!(t.len() >= 44);
                out.lapic = word(t, 36) as u64;
                let mut offset = 44;
                while offset < t.len() {
                    assert!(offset + 2 <= t.len());
                    let len = t[offset + 1] as usize;
                    assert!(len >= 2 && offset + len <= t.len());
                    let entry = &t[offset..offset + len];
                    match entry[0] {
                        0 if len >= 8 && word(entry, 4) & 1 != 0 => {
                            assert!(out.count < 64);
                            out.ids[out.count] = entry[3];
                            out.count += 1;
                        }
                        1 if len >= 12 => {
                            if out.ioapic == 0 {
                                assert_eq!(word(entry, 8), 0, "Q35 IOAPIC GSI base");
                                out.ioapic = word(entry, 4) as u64;
                            }
                        }
                        _ => {}
                    }
                    offset += len;
                }
            }
            b"MCFG" => {
                assert!(t.len() >= 60 && t[52..55] == [0, 0, 0]);
                out.ecam = u64::from_le_bytes(t[44..52].try_into().unwrap());
            }
            b"HPET" => {
                assert!(t.len() >= 56 && t[40] == 0);
                out.hpet = u64::from_le_bytes(t[44..52].try_into().unwrap());
            }
            _ => {}
        }
    }
    assert!(
        out.lapic != 0 && out.ioapic != 0 && out.hpet != 0 && out.count != 0,
        "Q35 ACPI devices"
    );
    out
}
