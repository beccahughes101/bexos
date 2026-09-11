use bexos_usb_host::{
    ClassPolicy, DescriptorError,
    bot::{CommandStatusWrapper, scsi_read10},
    descriptor::{parse_configuration, parse_device},
    hid::{BootKeyboardReport, KeyTransition, keyboard_transitions},
    policy::{InterfaceClass, PolicyDecision},
    transfer::{BufferRegistration, TransferLimits, validate_buffer, validate_transfer},
};
use usb_host_fidl::{ControlSetup, TransferDirection, TransferRequest};

#[test]
fn descriptor_parser_rejects_truncation_and_duplicates() {
    let device = [
        18, 1, 0, 3, 0, 0, 0, 9, 0x34, 0x12, 0x78, 0x56, 1, 0, 0, 0, 0, 1,
    ];
    assert_eq!(parse_device(&device).unwrap().vendor_id, 0x1234);
    assert!(matches!(
        parse_device(&device[..10]),
        Err(DescriptorError::Truncated)
    ));
    let duplicate_ep = [
        9, 2, 32, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 2, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10, 7, 5,
        0x81, 3, 8, 0, 10,
    ];
    assert!(matches!(
        parse_configuration(&duplicate_ep),
        Err(DescriptorError::Duplicate)
    ));
}

#[test]
fn composite_policy_requires_every_interface_to_be_supported() {
    let bytes = [
        9, 2, 41, 0, 2, 1, 0, 0x80, 50, 9, 4, 0, 0, 1, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10, 9, 4,
        1, 0, 2, 8, 6, 0x50, 0, 7, 5, 0x82, 2, 64, 0, 0,
    ];
    let cfg = parse_configuration(&bytes).unwrap();
    let policy = ClassPolicy::default();
    assert!(policy.admits_composite(&cfg));
    assert_eq!(
        policy.decide_interface(&cfg.interfaces[0]),
        PolicyDecision::Admit(InterfaceClass::BootKeyboard)
    );
    let mut unsupported = bytes;
    unsupported[30] = 0xff;
    assert!(!policy.admits_composite(&parse_configuration(&unsupported).unwrap()));
}

#[test]
fn transfers_validate_ranges_directions_and_ids() {
    let limits = TransferLimits {
        max_transfer_bytes: 4096,
        ..Default::default()
    };
    validate_buffer(128, 512, 1024, TransferDirection::In, limits).unwrap();
    let request = TransferRequest {
        transfer_id: 7,
        endpoint: 0x81,
        direction: TransferDirection::In,
        setup: ControlSetup {
            request_type: 0,
            request: 0,
            value: 0,
            index: 0,
            length: 0,
        },
        buffer_id: 2,
        buffer_offset: 128,
        length: 512,
        timeout_us: 1000,
    };
    validate_transfer(
        &request,
        Some(BufferRegistration {
            id: 2,
            size_bytes: 1024,
            direction: TransferDirection::In,
        }),
        limits,
    )
    .unwrap();
    let mut bad = request;
    bad.length = 5000;
    assert!(validate_transfer(&bad, None, limits).is_err());
}

#[test]
fn boot_keyboard_reports_emit_press_and_release_transitions() {
    let previous = BootKeyboardReport {
        modifiers: 1,
        keys: [4, 5, 0, 0, 0, 0],
    };
    let next = BootKeyboardReport {
        modifiers: 0,
        keys: [5, 6, 0, 0, 0, 0],
    };
    let mut out = [KeyTransition::default(); 8];
    let count = keyboard_transitions(previous, next, &mut out);
    assert_eq!(count, 3);
    assert_eq!(out[0].code, 0xe0);
    assert!(!out[0].pressed);
    assert_eq!(out[1].code, 4);
    assert_eq!(out[2].code, 6);
}

#[test]
fn bot_helpers_encode_scsi_and_validate_status_tags() {
    let mut cb = [0; 16];
    assert_eq!(scsi_read10(&mut cb, 0x11223344, 8), 10);
    assert_eq!(cb[0], 0x28);
    assert_eq!(&cb[2..6], &[0x11, 0x22, 0x33, 0x44]);
    let mut csw = [0; 13];
    csw[0..4].copy_from_slice(&0x5342_5355u32.to_le_bytes());
    csw[4..8].copy_from_slice(&55u32.to_le_bytes());
    let decoded = CommandStatusWrapper::decode(&csw, 55).unwrap();
    assert_eq!(decoded.status, 0);
    assert!(CommandStatusWrapper::decode(&csw, 54).is_err());
}
