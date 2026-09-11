use super::*;
#[test]
fn decodes_register_sib_rip_relative_and_immediate_accesses() {
    let mut regs = [0; 16];
    regs[0] = 0xfee00000;
    regs[1] = 8;
    regs[9] = 0x12345678;
    let access = decode(&[0x44, 0x89, 0x88, 0x80, 0, 0, 0], 0, &regs, true).unwrap();
    assert_eq!(
        access,
        Access {
            address: 0xfee00080,
            bytes: 4,
            length: 7,
            operation: Operation::Write { value: 0x12345678 }
        }
    );
    assert_eq!(
        decode(&[0x48, 0x8b, 0x54, 0xc8, 0xf0], 0, &regs, true)
            .unwrap()
            .address,
        0xfee00030
    );
    let rip = decode(
        &[
            0x48, 0xc7, 0x05, 0xf0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ],
        0x1000,
        &regs,
        true,
    )
    .unwrap();
    assert_eq!(rip.address, 0xffb);
    assert_eq!(rip.operation, Operation::Write { value: u64::MAX });
}
#[test]
fn read_results_follow_partial_register_and_zero_extension_rules() {
    let regs = [0; 16];
    for (code, expected) in [
        (&[0x8a, 0x20][..], 0xffffffffffff12ff),
        (&[0x66, 0x8b, 0x00][..], 0xffffffffffff0012),
        (&[0x8b, 0x00][..], 0x12),
        (&[0x0f, 0xb6, 0x00][..], 0x12),
    ] {
        let access = decode(code, 0, &regs, true).unwrap();
        assert_eq!(access.read_result(u64::MAX, 0x12), Some(expected));
    }
}
#[test]
fn malformed_and_unsupported_encodings_never_produce_mmio() {
    for code in [
        &[0x48, 0x8b][..],
        &[0x48, 0x8b, 0xc0],
        &[0xf0, 0x89, 0],
        &[0xc7, 0x08, 0, 0, 0, 0],
        &[0x64, 0x8b, 0],
        &[0x0f, 0x01, 0xd9],
    ] {
        assert_eq!(decode(code, 0, &[0; 16], true), None);
    }
    let code = [0x48, 0xc7, 0x80, 0x80, 0, 0, 0, 1, 0, 0, 0];
    for length in 0..code.len() {
        assert_eq!(decode(&code[..length], 0, &[0; 16], true), None);
    }
}
#[test]
fn address_size_override_truncates_effective_address() {
    let mut regs = [0; 16];
    regs[0] = 0x1fee00000;
    assert_eq!(
        decode(&[0x67, 0x8b, 0], 0, &regs, true).unwrap().address,
        0xfee00000
    );
}

#[test]
fn lapic_poll_test_reads_without_mutation_and_preserves_control_flags() {
    let mut regs = [0; 16];
    regs[1] = 0xfee00000;
    let code = [0xf7, 0x81, 0, 3, 0, 0, 0, 0x10, 0, 0];
    let access = decode(&code, 0, &regs, true).unwrap();
    assert_eq!(access.address, 0xfee00300);
    assert_eq!(access.length, 10);
    assert_eq!(access.operation, Operation::Test { value: 0x1000 });
    assert_eq!(access.test_flags(0x202 | 0x8d5, 0), Some(0x246));
    assert_eq!(access.test_flags(0x202, 0x1000), Some(0x206));
    for length in 0..code.len() {
        assert!(decode(&code[..length], 0, &regs, true).is_none());
    }
    assert!(decode(&[0xf7, 0x08, 0, 0, 0, 0], 0, &regs, true).is_none());
    regs[0] = 0x80000001;
    let register_test = decode(&[0x85, 0x01], 0, &regs, true).unwrap();
    assert_eq!(register_test.test_flags(0x202, 0xffffffff), Some(0x282));
    let sign_extended =
        decode(&[0x48, 0xf7, 0x01, 0xff, 0xff, 0xff, 0xff], 0, &regs, true).unwrap();
    assert_eq!(sign_extended.test_flags(2, 1 << 63), Some(0x86));
}

#[test]
fn hpet_arithmetic_obeys_width_overflow_carry_and_partial_register_rules() {
    let mut regs = [0; 16];
    regs[14] = 0xfed00000;
    let access = decode(&[0x49, 0x03, 0x86, 0xf0, 0, 0, 0], 0, &regs, true).unwrap();
    assert_eq!(access.address, 0xfed000f0);
    assert_eq!(access.bytes, 8);
    assert_eq!(access.alu_result(u64::MAX, 1, 0x202), Some((0, 0x257)));
    assert_eq!(
        access.alu_result(i64::MAX as u64, 1, 0x202),
        Some((1 << 63, 0xa96))
    );
    let sub = decode(&[0x2b, 0x01], 0, &regs, true).unwrap();
    assert_eq!(sub.alu_result(0, 1, 0x202), Some((0xffffffff, 0x297)));
    let compare = decode(&[0x3b, 0x01], 0, &regs, true).unwrap();
    assert_eq!(
        compare.alu_result(u64::MAX, 0xffffffff, 0x202),
        Some((u64::MAX, 0x246))
    );
    let byte = decode(&[0x02, 0x21], 0, &regs, true).unwrap(); // add ah, [rcx]
    assert_eq!(byte.alu_result(0xffff, 1, 0x202), Some((0xff, 0x257)));
    assert!(decode(&[0xf0, 0x03, 0x01], 0, &regs, true).is_none());
}

#[test]
fn stores_ignore_bits_outside_the_encoded_operand_width() {
    let mut registers = [0; 16];
    registers[0] = u64::MAX;
    for (code, expected) in [
        (&[0x88, 0x01][..], 255),
        (&[0x88, 0x21][..], 255),
        (&[0x66, 0x89, 0x01][..], 65535),
        (&[0x89, 0x01][..], u64::from(u32::MAX)),
        (&[0x48, 0x89, 0x01][..], u64::MAX),
    ] {
        assert_eq!(
            decode(code, 0, &registers, true).unwrap().operation,
            Operation::Write { value: expected }
        );
    }
}
