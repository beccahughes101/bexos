use bexos_app_archive::{BuildEntry, Compression, OpenArchive, TrustedKey, build_archive};

mod allocations;

const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const PUBLIC: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

fn trusted() -> [TrustedKey<'static>; 1] {
    [TrustedKey {
        key_id: KEY_ID,
        public_key: &PUBLIC,
    }]
}

#[test]
fn round_trips_uncompressed_archive() {
    let built = build_archive(
        &[
            BuildEntry {
                path: "package.bexmanifest",
                bytes: b"manifest",
                mode: 0o444,
            },
            BuildEntry {
                path: "bin/app",
                bytes: b"hello from app",
                mode: 0o555,
            },
        ],
        Compression::None,
        KEY_ID,
        SEED,
    )
    .unwrap();
    let archive = OpenArchive::parse_and_verify(&built.bytes, &trusted()).unwrap();
    assert_eq!(archive.entries().len(), 2);
    let entry = archive.find("bin/app").unwrap();
    assert_eq!(archive.read_file(entry).unwrap(), b"hello from app");
}

#[test]
fn round_trips_zstd_archive() {
    let payload = vec![42u8; 9000];
    let built = build_archive(
        &[BuildEntry {
            path: "asset/repeated.bin",
            bytes: &payload,
            mode: 0o444,
        }],
        Compression::Zstd,
        KEY_ID,
        SEED,
    )
    .unwrap();
    let archive = OpenArchive::parse_and_verify(&built.bytes, &trusted()).unwrap();
    let entry = archive.find("asset/repeated.bin").unwrap();
    assert_eq!(archive.read_file(entry).unwrap(), payload);
}

#[test]
fn large_service_decompression_uses_exact_signed_output_bound() {
    let payload = vec![42u8; 33 * 1024 * 1024 + 17];
    let built = build_archive(
        &[BuildEntry {
            path: "bin/scened",
            bytes: &payload,
            mode: 0o555,
        }],
        Compression::Zstd,
        KEY_ID,
        SEED,
    )
    .unwrap();
    let archive = OpenArchive::parse_and_verify(&built.bytes, &trusted()).unwrap();
    let entry = archive.find("bin/scened").unwrap();
    let (decoded, maximum) = allocations::largest_request(|| archive.read_file(entry).unwrap());
    assert!(
        maximum <= payload.len(),
        "decoder requested {maximum} bytes for a {} byte executable",
        payload.len()
    );
    assert_eq!(decoded, payload);
    assert_eq!(
        decoded.capacity(),
        payload.len(),
        "geometric growth exceeded the signed bound"
    );
    drop(decoded);
    for length in [0, payload.len() - 1, payload.len() + 1] {
        let mut invalid = entry.clone();
        invalid.uncompressed_size = length as u64;
        assert!(archive.read_file(&invalid).is_err());
    }
}

#[test]
fn rejects_unknown_key_and_tampered_chunks() {
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
    assert_eq!(
        OpenArchive::parse_and_verify(&built.bytes, &[]).unwrap_err(),
        bexos_app_archive::ArchiveError::UnknownKey
    );

    let mut tampered = built.bytes;
    tampered[128] ^= 0x55;
    assert_eq!(
        OpenArchive::parse_and_verify(&tampered, &trusted()).unwrap_err(),
        bexos_app_archive::ArchiveError::DigestMismatch
    );
}

#[test]
fn rejects_legacy_v1_magic_unconditionally() {
    let mut legacy = vec![0u8; 64];
    legacy[..8].copy_from_slice(b"BEXARCV1");
    assert_eq!(
        OpenArchive::parse(&legacy).unwrap_err(),
        bexos_app_archive::ArchiveError::UnsupportedVersion
    );
}

#[test]
fn rejects_invalid_paths_and_duplicates() {
    assert_eq!(
        build_archive(
            &[BuildEntry {
                path: "../bad",
                bytes: b"x",
                mode: 0o444
            }],
            Compression::None,
            KEY_ID,
            SEED,
        )
        .unwrap_err(),
        bexos_app_archive::ArchiveError::InvalidPath
    );
    assert_eq!(
        build_archive(
            &[
                BuildEntry {
                    path: "same",
                    bytes: b"x",
                    mode: 0o444
                },
                BuildEntry {
                    path: "same",
                    bytes: b"y",
                    mode: 0o444
                },
            ],
            Compression::None,
            KEY_ID,
            SEED,
        )
        .unwrap_err(),
        bexos_app_archive::ArchiveError::DuplicatePath
    );
}
