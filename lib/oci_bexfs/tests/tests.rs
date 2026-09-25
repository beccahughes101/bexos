use bexos_bexfs::block::{BEXFS_BLOCK_SIZE, MemoryBlockDevice};
use bexos_bexfs::key::LockedVolumeKey;
use bexos_bexfs::{BexFs, FormatOptions};
use bexos_oci::{Image, LayerOperation, ProcessConfig, apply_image};
use bexos_oci_bexfs::BexFsSink;

fn filesystem(device: &mut MemoryBlockDevice) -> BexFs {
    BexFs::format(
        device,
        LockedVolumeKey::new(&[0x41; 32]).unwrap(),
        FormatOptions {
            label: "OCI",
            volume_uuid: [0x42; 16],
            device_uuid: [0x43; 16],
        },
    )
    .unwrap()
}

#[test]
fn applies_links_whiteouts_opaque_directories_and_metadata() {
    let mut device = MemoryBlockDevice::new(BEXFS_BLOCK_SIZE, 8192);
    let mut fs = filesystem(&mut device);
    let root = fs.root_inode();
    let image = Image {
        process: ProcessConfig {
            arguments: vec!["/bin/tool".into()],
            environment: vec![],
            working_directory: "/".into(),
            user: "".into(),
        },
        manifest_digest: [0; 32],
        layers: vec![
            vec![
                LayerOperation::Directory {
                    path: "bin".into(),
                    mode: 0o755,
                    uid: 10,
                    gid: 20,
                    mtime: 7,
                },
                LayerOperation::File {
                    path: "bin/tool".into(),
                    data: b"first".to_vec(),
                    mode: 0o751,
                    uid: 10,
                    gid: 20,
                    mtime: 8,
                },
                LayerOperation::Hardlink {
                    path: "bin/tool-hard".into(),
                    target: "bin/tool".into(),
                },
                LayerOperation::Symlink {
                    path: "bin/tool-link".into(),
                    target: "tool".into(),
                    uid: 10,
                    gid: 20,
                },
            ],
            vec![
                LayerOperation::Remove {
                    path: "bin/tool-hard".into(),
                },
                LayerOperation::OpaqueDirectory { path: "bin".into() },
                LayerOperation::File {
                    path: "bin/replacement".into(),
                    data: b"second".to_vec(),
                    mode: 0o700,
                    uid: 30,
                    gid: 40,
                    mtime: 9,
                },
                LayerOperation::Xattrs {
                    path: "bin/replacement".into(),
                    values: vec![
                        ("security.capability".into(), vec![1, 2, 3, 4]),
                        ("user.oci".into(), b"preserved".to_vec()),
                    ],
                },
            ],
        ],
    };

    apply_image(&image, &mut BexFsSink::new(&mut fs, root).unwrap()).unwrap();
    let directory = fs.open(root, "bin", 0x1 | 0x20).unwrap().inode();
    let entries = fs.read_entries(directory).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "replacement");
    let mut file = fs.open(root, "bin/replacement", 0x1).unwrap();
    assert_eq!(fs.read(&mut file, 16).unwrap(), b"second");
    let attributes = fs.attributes(file.inode()).unwrap();
    assert_eq!(
        (attributes.mode, attributes.uid, attributes.gid),
        (0o700, 30, 40)
    );
    assert_eq!(attributes.modification_time_nanos, 9_000_000_000);
    assert_eq!(
        fs.get_xattr(file.inode(), "security.capability").unwrap(),
        [1, 2, 3, 4]
    );
    assert_eq!(
        fs.get_xattr(file.inode(), "user.oci").unwrap(),
        b"preserved"
    );
}
