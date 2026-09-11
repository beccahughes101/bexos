use bexos_elf::{arch::Machine, load::LoadPlan};
#[test]
fn linked_kernels_have_valid_machine_segments_and_executable_entrypoints() {
    for name in ["KERNEL_ELF", "REPLACEMENT_ELF"] {
        let bytes = std::fs::read(std::env::var(name).unwrap()).unwrap();
        let plan = LoadPlan::parse(&bytes, Machine::current_guest()).expect(name);
        assert_ne!(plan.entry_vaddr, 0);
        if name == "REPLACEMENT_ELF" {
            let header = &plan.segments[0];
            assert_eq!(header.vaddr, bexos_boot::UPDATE_BASE);
            let at = header.file_offset as usize;
            assert_eq!(
                u64::from_le_bytes(bytes[at + 32..at + 40].try_into().unwrap()),
                bexos_boot::ARCHITECTURE_ID
            );
        }
    }
}
