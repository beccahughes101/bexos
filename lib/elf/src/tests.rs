use crate::*;

fn executable(machine: Machine) -> Vec<u8> {
    let mut bytes = alloc::vec![0; 8192];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.elf_machine().to_le_bytes());
    bytes[24..32].copy_from_slice(&0x8000_1000u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&5u32.to_le_bytes());
    for (offset, value) in [
        (72, 4096u64),
        (80, 0x8000_1000),
        (96, 4096),
        (104, 4096),
        (112, 4096),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[test]
fn executable_machine_permissions_and_entry_are_checked() {
    for machine in [Machine::Aarch64, Machine::X86_64] {
        let mut bytes = executable(machine);
        let parsed = ParsedElf::parse_for(&bytes, machine).unwrap();
        assert_eq!(parsed.entry_vaddr, 0x8000_1000);
        assert_eq!(parsed.mappings[0].rights, RIGHTS_READ | RIGHTS_EXECUTE);
        let other = if machine == Machine::Aarch64 {
            Machine::X86_64
        } else {
            Machine::Aarch64
        };
        assert_eq!(
            ParsedElf::parse_for(&bytes, other),
            Err(ElfError::UnsupportedMachine)
        );
        bytes[68] = 7;
        assert_eq!(
            ParsedElf::parse_for(&bytes, machine),
            Err(ElfError::WriteExecuteSegment)
        );
        bytes[68] = 5;
        bytes[24..32].copy_from_slice(&0x9000_0000u64.to_le_bytes());
        assert_eq!(
            ParsedElf::parse_for(&bytes, machine),
            Err(ElfError::EntryOutsideExecutableSegment)
        );
    }
}

#[test]
fn malformed_program_headers_are_rejected() {
    let mut bytes = executable(Machine::X86_64);
    bytes[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(
        LoadPlan::parse(&bytes, Machine::X86_64),
        Err(ElfLoadError::Overflow)
    );
    let mut bytes = executable(Machine::X86_64);
    bytes[112..120].copy_from_slice(&3u64.to_le_bytes());
    assert_eq!(
        LoadPlan::parse(&bytes, Machine::X86_64),
        Err(ElfLoadError::InvalidLoadSegment)
    );
}

#[test]
fn tls_variants_preserve_templates_and_bss() {
    let executable = [1u8, 2, 3];
    let library = [4u8, 5];
    for machine in [Machine::Aarch64, Machine::X86_64] {
        let plan = tls::layout(
            machine,
            [
                (
                    &executable[..],
                    Some(TlsSegment {
                        file_offset: 0,
                        file_size: 3,
                        mem_size: 32,
                        align: 16,
                    }),
                ),
                (
                    &library[..],
                    Some(TlsSegment {
                        file_offset: 0,
                        file_size: 2,
                        mem_size: 48,
                        align: 32,
                    }),
                ),
            ],
        )
        .unwrap();
        assert_eq!(plan.modules[0].id, 1);
        assert_eq!(plan.modules[1].id, 2);
        for (module, template) in plan.modules.iter().zip([&executable[..], &library[..]]) {
            assert_eq!(
                &plan.template[module.offset as usize..module.offset as usize + template.len()],
                template
            );
            assert_eq!(plan.template[module.offset as usize + template.len()], 0);
        }
        let (bytes, tp) = tls::initialize(&plan, machine, 0x10000).unwrap();
        let expected_offset = if machine == Machine::Aarch64 { 16 } else { -32 };
        assert_eq!(plan.modules[0].thread_offset, expected_offset);
        let address = tp.checked_add_signed(expected_offset).unwrap();
        assert_eq!(
            &bytes[(address - 0x10000) as usize..(address - 0x10000) as usize + 3],
            &executable
        );
        if machine == Machine::X86_64 {
            let start = (tp - 0x10000) as usize;
            assert_eq!(
                u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap()),
                tp
            );
        }
    }
}

fn relocate(
    machine: Machine,
    kind: u32,
    symbol: DynamicSymbol,
    rights: u32,
) -> Result<u64, ElfError> {
    relocate_with_addend(machine, kind, symbol, rights, 0)
}

fn relocate_with_addend(
    machine: Machine,
    kind: u32,
    symbol: DynamicSymbol,
    rights: u32,
    addend: i64,
) -> Result<u64, ElfError> {
    let mut image = alloc::vec![0; 1024];
    image[272..280].copy_from_slice(&addend.to_le_bytes());
    image[256..264].copy_from_slice(&128u64.to_le_bytes());
    image[264..272].copy_from_slice(&((1u64 << 32) | kind as u64).to_le_bytes());
    image[768..775].copy_from_slice(b"symbol\0");
    let loads = [ProgramLoad {
        file_offset: 0,
        file_size: 1024,
        mem_size: 1024,
        vaddr: 0,
        rights,
    }];
    let symbols = [
        DynamicSymbol {
            name_offset: 0,
            info: 0,
            visibility: 0,
            section_index: 0,
            value: 0,
        },
        symbol,
    ];
    let dynamic = DynamicInfo {
        strtab: 768,
        string_size: 7,
        ..DynamicInfo::default()
    };
    relocation::apply_relocation_table(
        &mut image,
        &loads,
        256,
        24,
        &symbols,
        0x10000,
        &dynamic,
        &BTreeMap::new(),
        machine,
        tls::Module {
            id: 2,
            offset: 0,
            thread_offset: -64,
            mem_size: 64,
        },
    )?;
    Ok(u64::from_le_bytes(image[128..136].try_into().unwrap()))
}

#[test]
fn architecture_relocations_and_tls_have_expected_values() {
    for (machine, relative, absolute, module, offset, thread) in [
        (Machine::Aarch64, 1027, 257, 1028, 1029, 1030),
        (Machine::X86_64, 8, 1, 16, 17, 18),
    ] {
        let symbol = DynamicSymbol {
            name_offset: 0,
            info: 0x16,
            visibility: 0,
            section_index: 1,
            value: 8,
        };
        for (kind, expected) in [
            (relative, 0x10000),
            (absolute, 0x10008),
            (module, 2),
            (offset, 8),
            (thread, (-56i64) as u64),
        ] {
            assert_eq!(
                relocate(machine, kind, symbol, RIGHTS_READ | RIGHTS_WRITE),
                Ok(expected)
            );
        }
        assert_eq!(
            relocate(machine, absolute, symbol, RIGHTS_READ | RIGHTS_EXECUTE),
            Err(ElfError::TextRelocation)
        );
        assert_eq!(
            relocate(machine, 0xffff, symbol, RIGHTS_READ | RIGHTS_WRITE),
            Err(ElfError::UnsupportedRelocation)
        );
    }
}

#[test]
fn strong_undefined_symbols_fail_and_weak_symbols_bind_zero() {
    for (machine, kind) in [(Machine::Aarch64, 257), (Machine::X86_64, 1)] {
        let mut symbol = DynamicSymbol {
            name_offset: 0,
            info: 0x10,
            visibility: 0,
            section_index: 0,
            value: 0,
        };
        assert_eq!(
            relocate(machine, kind, symbol, RIGHTS_READ | RIGHTS_WRITE),
            Err(ElfError::UnresolvedStrongSymbol)
        );
        symbol.info = 0x20;
        assert_eq!(
            relocate(machine, kind, symbol, RIGHTS_READ | RIGHTS_WRITE),
            Ok(0)
        );
    }
}

fn dynamic_fixture(machine: Machine, executable: bool) -> Vec<u8> {
    let mut bytes = alloc::vec![0; 0x3000];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&(if executable { 2u16 } else { 3u16 }).to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.elf_machine().to_le_bytes());
    bytes[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&4u16.to_le_bytes());
    for (index, kind, flags, offset, vaddr, file, memory) in [
        (0, 1u32, 5u32, 0x1000u64, 0x1000u64, 0x1000u64, 0x1000u64),
        (1, 1, 6, 0x2000, 0x2000, 0x1000, 0x3000),
        (2, 2, 6, 0x2000, 0x2000, 256, 256),
        (3, 0x6474e552, 4, 0x2000, 0x2000, 0x1000, 0x1000),
    ] {
        let ph = 64 + index * 56;
        bytes[ph..ph + 4].copy_from_slice(&kind.to_le_bytes());
        bytes[ph + 4..ph + 8].copy_from_slice(&flags.to_le_bytes());
        for (at, value) in [
            (8, offset),
            (16, vaddr),
            (32, file),
            (40, memory),
            (48, 4096),
        ] {
            bytes[ph + at..ph + at + 8].copy_from_slice(&value.to_le_bytes());
        }
    }
    let entries = [
        (4u64, 0x2400u64),
        (5, 0x2300),
        (6, 0x2200),
        (10, 15),
        (11, 24),
        (7, 0x2500),
        (8, 48),
        (9, 24),
        (25, 0x2600),
        (27, 8),
        (if executable { 0 } else { 14 }, 1),
        (0, 0),
    ];
    for (index, (tag, value)) in entries.into_iter().enumerate() {
        let at = 0x2000 + index * 16;
        bytes[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        bytes[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }
    bytes[0x2300..0x230f].copy_from_slice(b"\0libfixture.so\0");
    bytes[0x2400..0x2404].copy_from_slice(&1u32.to_le_bytes());
    bytes[0x2404..0x2408].copy_from_slice(&1u32.to_le_bytes());
    let relative = if machine == Machine::Aarch64 {
        1027u64
    } else {
        8u64
    };
    for (index, target, addend) in [(0, 0x3500u64, 0x1234u64), (1, 0x2600, 0x1000)] {
        let at = 0x2500 + index * 24;
        bytes[at..at + 8].copy_from_slice(&target.to_le_bytes());
        bytes[at + 8..at + 16].copy_from_slice(&relative.to_le_bytes());
        bytes[at + 16..at + 24].copy_from_slice(&addend.to_le_bytes());
    }
    bytes
}

#[test]
fn executable_and_library_linking_relocate_bss_constructors_and_relro() {
    for machine in [Machine::Aarch64, Machine::X86_64] {
        for executable in [false, true] {
            let bytes = dynamic_fixture(machine, executable);
            let bias = if executable { 0 } else { 0x40000000 };
            let linked = if executable {
                link_executable(&bytes, machine, tls::Module::default(), &[], &[])
                    .unwrap()
                    .unwrap()
            } else {
                DynamicLibrary::parse_and_relocate(
                    &bytes,
                    bias,
                    LibraryPolicy {
                        soname: "libfixture.so",
                        symbol_prefix: "",
                        tls: tls::Module::default(),
                        direct_dependencies: &[],
                    },
                    machine,
                    &[],
                )
                .unwrap()
            };
            assert_eq!(linked.constructors, [bias + 0x1000]);
            let relro = linked
                .segments
                .iter()
                .find(|s| s.vaddr == bias + 0x2000)
                .unwrap();
            assert_eq!(relro.rights, RIGHTS_READ);
            let bss = linked
                .segments
                .iter()
                .find(|s| s.vaddr == bias + 0x3000)
                .unwrap();
            assert_eq!(bss.rights, RIGHTS_READ | RIGHTS_WRITE);
            assert_eq!(read_u64(&bss.bytes, 0x500).unwrap(), bias + 0x1234);
            assert!(bss.bytes[..0x500].iter().all(|v| *v == 0));
        }
    }
}

#[test]
fn partial_page_bss_uses_exact_file_bytes_and_separate_zero_pages() {
    for machine in [Machine::Aarch64, Machine::X86_64] {
        let mut bytes = executable(machine);
        bytes[96..104].copy_from_slice(&17u64.to_le_bytes());
        bytes[104..112].copy_from_slice(&8192u64.to_le_bytes());
        let plan = LoadPlan::parse(&bytes, machine).unwrap();
        assert_eq!(plan.segments[0].file_size, 17);
        let bss = plan.segments[0].zero_fill.unwrap();
        assert_eq!(bss.vaddr, 0x80002000);
        assert_eq!(bss.size_bytes, 4096);
    }
}

#[test]
fn relocated_tls_templates_are_copied_into_each_threads_layout() {
    for machine in [Machine::Aarch64, Machine::X86_64] {
        let mut bytes = dynamic_fixture(machine, false);
        bytes[56..58].copy_from_slice(&5u16.to_le_bytes());
        let header = 64 + 4 * 56;
        bytes[header..header + 4].copy_from_slice(&7u32.to_le_bytes());
        for (offset, value) in [(8, 0x2800u64), (16, 0x2800), (32, 8), (40, 16), (48, 8)] {
            bytes[header + offset..header + offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes[0x2500..0x2508].copy_from_slice(&0x2800u64.to_le_bytes());
        // A normal initialized section follows .tdata at the same addresses
        // covered by .tbss. Those bytes must not initialize zero-filled TLS.
        bytes[0x2808..0x2810].copy_from_slice(&0xfeed_beef_u64.to_le_bytes());
        let tls = library_tls(&bytes, machine).unwrap();
        let mut layout = tls::layout(machine, [(&bytes[..], tls)]).unwrap();
        let bias = 0x40000000;
        let linked = DynamicLibrary::parse_and_relocate(
            &bytes,
            bias,
            LibraryPolicy {
                soname: "libfixture.so",
                symbol_prefix: "",
                tls: layout.modules[0],
                direct_dependencies: &[],
            },
            machine,
            &[],
        )
        .unwrap();
        layout.install_relocated(0, &linked).unwrap();
        let offset = layout.modules[0].offset as usize;
        assert_eq!(read_u64(&layout.template, offset).unwrap(), bias + 0x1234);
        assert_eq!(read_u64(&layout.template, offset + 8).unwrap(), 0);
        for base in [0x80000000, 0x90000000] {
            let (thread, tp) = tls::initialize(&layout, machine, base).unwrap();
            let address = tp
                .checked_add_signed(layout.modules[0].thread_offset)
                .unwrap();
            assert_eq!(
                read_u64(&thread, (address - base) as usize).unwrap(),
                bias + 0x1234
            );
        }
    }
}

#[test]
fn dynamic_symbol_relocations_follow_each_architectures_addend_rules() {
    let symbol = DynamicSymbol {
        name_offset: 0,
        info: 0x11,
        visibility: 0,
        section_index: 1,
        value: 8,
    };
    for (machine, kind, expected) in [
        (Machine::X86_64, 1, 0x1000d),
        (Machine::X86_64, 6, 0x10008),
        (Machine::X86_64, 7, 0x10008),
        (Machine::Aarch64, 257, 0x1000d),
        (Machine::Aarch64, 1025, 0x1000d),
        (Machine::Aarch64, 1026, 0x1000d),
    ] {
        assert_eq!(
            relocate_with_addend(machine, kind, symbol, RIGHTS_READ | RIGHTS_WRITE, 5),
            Ok(expected)
        );
    }
}

#[test]
fn over_aligned_tls_modules_are_aligned_relative_to_the_thread_pointer() {
    for machine in [Machine::Aarch64, Machine::X86_64] {
        let image = [7u8; 8];
        let layout = tls::layout(
            machine,
            [64, 256].map(|align| {
                (
                    &image[..],
                    Some(TlsSegment {
                        file_offset: 0,
                        file_size: 8,
                        mem_size: 24,
                        align,
                    }),
                )
            }),
        )
        .unwrap();
        let (bytes, tp) = tls::initialize(&layout, machine, 0x10000).unwrap();
        for (module, align) in layout.modules.iter().zip([64, 256]) {
            let address = tp.checked_add_signed(module.thread_offset).unwrap();
            assert_eq!(address % align, 0);
            let offset = (address - 0x10000) as usize;
            assert_eq!(&bytes[offset..offset + 8], &image);
            assert!(bytes[offset + 8..offset + 24].iter().all(|byte| *byte == 0));
        }
        assert!(tls::initialize(&layout, machine, 0x10001).is_err());
    }
}
