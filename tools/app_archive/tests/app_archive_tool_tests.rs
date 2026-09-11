use bexos_app_archive::{BuildEntry, Compression, OpenArchive, TrustedKey, build_archive};

const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const PUBLIC: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

#[test]
fn generated_archive_detects_payload_tampering() {
    let built = build_archive(
        &[BuildEntry {
            path: "package.bexmanifest",
            bytes: b"manifest",
            mode: 0o444,
        }],
        Compression::None,
        KEY_ID,
        SEED,
    )
    .unwrap();
    let trusted = [TrustedKey {
        key_id: KEY_ID,
        public_key: &PUBLIC,
    }];
    OpenArchive::parse_and_verify(&built.bytes, &trusted).unwrap();

    let mut tampered = built.bytes;
    let signed_payload_byte = 32 + 72 + "package.bexmanifest".len();
    tampered[signed_payload_byte] ^= 1;
    assert!(OpenArchive::parse_and_verify(&tampered, &trusted).is_err());
}
