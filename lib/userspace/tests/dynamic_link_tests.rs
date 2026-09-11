static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
use bexos_userspace::dynamic_link::{
    EncodedSymbol, copy_tls_template, encode_linker_data, install, symbol_address, tls_layout,
};

#[test]
fn linker_data_v2_installs_symbols_and_tls_template() {
    let _lock = LOCK.lock().unwrap();
    let bytes = encode_linker_data(
        &[EncodedSymbol {
            name: "bexos_test_symbol",
            address: 0x1234,
        }],
        &[],
        &[1, 2, 3, 4],
        16,
        16,
    )
    .expect("encode linker data");

    if bexos_userspace::dynamic_link::ARCHITECTURE_ID != 1 {
        assert!(
            install(&bytes).is_err(),
            "legacy ARM metadata must be rejected on x86"
        );
        return;
    }

    install(&bytes).expect("install linker data");

    assert_eq!(symbol_address(b"bexos_test_symbol"), Some(0x1234));
    assert_eq!(tls_layout(), Some((16, 16)));
    let mut tls = [0xff; 16];
    let (file_size, mem_size, align) = copy_tls_template(&mut tls).expect("TLS template");
    assert_eq!((file_size, mem_size, align), (4, 16, 16));
    assert_eq!(&tls[..4], &[1, 2, 3, 4]);
    assert!(tls[4..].iter().all(|byte| *byte == 0));
}

#[test]
fn legacy_link_map_v1_still_installs_symbols() {
    let _lock = LOCK.lock().unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BXLINK01");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&8u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0xfeed_u64.to_le_bytes());
    bytes.extend_from_slice(b"legacy01");

    if bexos_userspace::dynamic_link::ARCHITECTURE_ID != 1 {
        assert!(
            install(&bytes).is_err(),
            "legacy ARM metadata must be rejected on x86"
        );
        return;
    }
    install(&bytes).expect("install legacy link map");

    assert_eq!(symbol_address(b"legacy01"), Some(0xfeed));
}

#[test]
fn linker_data_v3_validates_architecture_and_resolves_separate_thread_tls() {
    let _lock = LOCK.lock().unwrap();
    use bexos_userspace::dynamic_link::{TlsModule, encode_linker_data_v3, tls_address};
    let architecture = bexos_userspace::dynamic_link::ARCHITECTURE_ID;
    let modules = [
        TlsModule {
            thread_offset: if architecture == 1 { 16 } else { -24 },
            mem_size: 8,
        },
        TlsModule {
            thread_offset: if architecture == 1 { 32 } else { -8 },
            mem_size: 8,
        },
    ];
    let bytes = encode_linker_data_v3(&[], &[], &[0; 24], 24, 8, architecture, &modules).unwrap();
    install(&bytes).unwrap();
    assert_eq!(
        tls_address(0x1000, 1, 4),
        Some(if architecture == 1 { 0x1014 } else { 0xfec })
    );
    assert_eq!(
        tls_address(0x2000, 1, 4),
        Some(if architecture == 1 { 0x2014 } else { 0x1fec })
    );
    assert_eq!(
        tls_address(0x1000, 2, 4),
        Some(if architecture == 1 { 0x1024 } else { 0xffc })
    );
    assert_eq!(tls_address(0x1000, 2, 8), None);
    assert_eq!(tls_address(0x1000, 0, 0), None);
    let mut wrong = bytes.clone();
    wrong[36..44].copy_from_slice(&(3 - architecture).to_le_bytes());
    assert!(install(&wrong).is_err());
    assert_eq!(
        tls_address(0x1000, 2, 4),
        Some(if architecture == 1 { 0x1024 } else { 0xffc })
    );
}
