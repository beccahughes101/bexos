use bexos_memfs::{MemFs, MemFsError, NodeKind};

#[test]
fn creates_reads_truncates_lists_and_unlinks() {
    let mut fs = MemFs::new();
    let dir = fs.open(1, "logs", 1 | 2 | 8 | 0x20).unwrap();
    let mut file = fs.open(dir.inode(), "panic.log", 1 | 2 | 8).unwrap();
    fs.write(&mut file, b"panic").unwrap();
    fs.seek(&mut file, 0, 0).unwrap();
    assert_eq!(fs.read(&mut file, 32).unwrap(), b"panic");

    let entries = fs.read_entries(dir.inode()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "panic.log");
    assert_eq!(entries[0].kind, NodeKind::File);

    fs.set_len(&file, 2).unwrap();
    fs.seek(&mut file, 0, 0).unwrap();
    assert_eq!(fs.read(&mut file, 32).unwrap(), b"pa");
    assert_eq!(fs.unlink(1, "logs"), Err(MemFsError::NotEmpty));
    fs.unlink(dir.inode(), "panic.log").unwrap();
    fs.unlink(1, "logs").unwrap();
}

#[test]
fn rejects_unsafe_paths_and_wrong_rights() {
    let mut fs = MemFs::new();
    assert_eq!(fs.open(1, "/absolute", 1), Err(MemFsError::InvalidArgs));
    assert_eq!(fs.open(1, "../escape", 1), Err(MemFsError::InvalidArgs));
    assert_eq!(fs.open(1, "bad/", 1), Err(MemFsError::InvalidArgs));
    let mut file = fs.open(1, "readonly", 1 | 8).unwrap();
    assert_eq!(fs.write(&mut file, b"nope"), Err(MemFsError::AccessDenied));
}

#[test]
fn checkpoint_preserves_tree_and_cursor() {
    let mut fs = MemFs::new();
    let mut file = fs.open(1, "tmp.txt", 1 | 2 | 8).unwrap();
    fs.write(&mut file, b"hello").unwrap();
    fs.seek(&mut file, 2, 0).unwrap();
    let saved_file = file.checkpoint();
    let fs = MemFs::adopt(&fs.checkpoint()).unwrap();
    let mut file = bexos_memfs::OpenedNode::adopt(&saved_file).unwrap();
    assert_eq!(fs.read(&mut file, 3).unwrap(), b"llo");
}
