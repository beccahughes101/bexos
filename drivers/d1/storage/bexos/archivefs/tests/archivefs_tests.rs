use bexos_app_archive::{BuildEntry, Compression, build_archive};
use bexos_archivefs::{ArchiveFs, ArchiveFsError};

const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];

#[test]
fn mounts_and_reads_signed_archive() {
    let built = build_archive(
        &[
            BuildEntry {
                path: "package.bexmanifest",
                bytes: b"manifest",
                mode: 0o444,
            },
            BuildEntry {
                path: "bin/app",
                bytes: b"app bytes",
                mode: 0o555,
            },
        ],
        Compression::None,
        KEY_ID,
        SEED,
    )
    .unwrap();
    let mut fs = ArchiveFs::mount(&built.bytes, Some(built.content_root)).unwrap();
    let root = fs.root_inode();
    let bin = fs.open(root, "bin", 1 | 32).unwrap();
    assert!(bin.file.is_none());
    let mut app = fs.open(root, "bin/app", 1).unwrap().file.unwrap();
    assert_eq!(fs.read(&mut app, 64).unwrap(), b"app bytes");
}

#[test]
fn rejects_writes_and_bad_expected_root() {
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
    let fs = ArchiveFs::mount(&built.bytes, None).unwrap();
    assert_eq!(
        fs.open(fs.root_inode(), "package.bexmanifest", 1 | 2)
            .unwrap_err(),
        ArchiveFsError::ReadOnly
    );
    let mut wrong = built.content_root;
    wrong[0] ^= 1;
    assert_eq!(
        ArchiveFs::mount(&built.bytes, Some(wrong)).unwrap_err(),
        ArchiveFsError::Corrupt
    );
}

#[test]
fn node_checkpoint_retains_identity_contents_and_open_cursor() {
    use bexos_migration::codec::{Decoder, Encoder};
    let bytes = vec![42; 40 * 1024];
    for compression in [Compression::None, Compression::Zstd] {
        let built = build_archive(
            &[BuildEntry {
                path: "bin/large",
                bytes: &bytes,
                mode: 0o555,
            }],
            compression,
            KEY_ID,
            SEED,
        )
        .unwrap();
        let mut old = ArchiveFs::mount(&built.bytes, None).unwrap();
        let mut file = old.open(0, "bin/large", 1).unwrap().file.unwrap();
        old.read(&mut file, 33).unwrap();
        let mut new = ArchiveFs::empty_checkpoint();
        for key in old.checkpoint_keys() {
            let record = old.checkpoint_record(key).unwrap();
            assert!(record.as_ref().unwrap().len() <= 32704);
            new.adopt_record(key, record.as_deref()).unwrap();
        }
        new.validate_checkpoint().unwrap();
        let mut w = Encoder::new();
        file.checkpoint(&mut w);
        let checkpoint = w.finish();
        let mut saved = bexos_archivefs::FileHandle::adopt(&mut Decoder::new(&checkpoint)).unwrap();
        assert_eq!(file.inode(), saved.inode());
        assert_eq!(
            old.read(&mut file, 50000).unwrap(),
            new.read(&mut saved, 50000).unwrap()
        );
        assert!(new.open(0, "bin/large", 1 | 2).is_err());
    }
}
