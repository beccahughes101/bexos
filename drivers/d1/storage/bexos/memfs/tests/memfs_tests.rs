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

#[test]
fn rename_moves_nodes_and_replaces_compatible_targets() {
    let mut fs = MemFs::new();
    fs.open(1, "a", 1 | 2 | 8).unwrap();
    fs.rename(1, "a", "b").unwrap();
    assert_eq!(fs.open(1, "a", 1), Err(MemFsError::NotFound));
    assert!(fs.open(1, "b", 1).is_ok());
    fs.open(1, "c", 1 | 2 | 8).unwrap();
    fs.rename(1, "b", "c").unwrap();
    assert!(fs.open(1, "c", 1).is_ok());

    fs.open(1, "dir", 1 | 2 | 8 | 0x20).unwrap();
    assert_eq!(
        fs.rename(1, "dir", "dir/child"),
        Err(MemFsError::InvalidArgs)
    );
}

#[test]
fn hard_links_share_an_inode_survive_unlink_and_checkpoint() {
    let mut fs = MemFs::new();
    let mut original = fs.open(1, "original", 0x1 | 0x2 | 0x8).unwrap();
    fs.write(&mut original, b"shared bytes").unwrap();
    fs.link(1, "original", "alias").unwrap();

    let alias = fs.open(1, "alias", 0x1).unwrap();
    assert_eq!(original.inode(), alias.inode());
    assert_eq!(
        fs.link(1, "original", "alias"),
        Err(MemFsError::AlreadyExists)
    );
    assert_eq!(
        fs.link(1, "original", "directory/link"),
        Err(MemFsError::NotFound)
    );

    fs.unlink(1, "original").unwrap();
    let saved = fs.checkpoint();
    let mut fs = MemFs::adopt(&saved).unwrap();
    let mut alias = fs.open(1, "alias", 0x1).unwrap();
    assert_eq!(fs.read(&mut alias, 64).unwrap(), b"shared bytes");
    fs.unlink(1, "alias").unwrap();
    assert_eq!(fs.open(1, "alias", 0x1), Err(MemFsError::NotFound));
}

#[test]
fn symlinks_and_xattrs_follow_linux_semantics_across_checkpoint() {
    let mut fs = MemFs::new();
    let mut original = fs.open(1, "original", 0x1 | 0x2 | 0x8).unwrap();
    fs.write(&mut original, b"payload").unwrap();
    let directory = fs.open(1, "links", 0x1 | 0x8 | 0x20).unwrap();
    fs.symlink(directory.inode(), "../original", "relative")
        .unwrap();
    fs.link(directory.inode(), "relative", "relative_alias")
        .unwrap();
    fs.symlink(1, "/original", "absolute").unwrap();
    fs.set_xattr(original.inode(), "user.container", b"metadata", 1)
        .unwrap();
    assert_eq!(
        fs.set_xattr(original.inode(), "user.container", b"again", 1),
        Err(MemFsError::AlreadyExists)
    );
    assert_eq!(
        fs.readlink(directory.inode(), "relative").unwrap(),
        "../original"
    );
    assert_eq!(
        fs.readlink(directory.inode(), "relative_alias").unwrap(),
        "../original"
    );

    let mut relative = fs.open(directory.inode(), "relative", 0x1).unwrap();
    assert_eq!(fs.read(&mut relative, 16).unwrap(), b"payload");
    let saved = fs.checkpoint();
    let mut fs = MemFs::adopt(&saved).unwrap();
    assert_eq!(
        fs.readlink(directory.inode(), "relative_alias").unwrap(),
        "../original"
    );
    let mut absolute = fs.open(1, "absolute", 0x1).unwrap();
    assert_eq!(fs.read(&mut absolute, 16).unwrap(), b"payload");
    assert_eq!(
        fs.get_xattr(absolute.inode(), "user.container").unwrap(),
        b"metadata"
    );
    assert_eq!(
        fs.list_xattrs(absolute.inode()).unwrap(),
        ["user.container"]
    );
    fs.remove_xattr(absolute.inode(), "user.container").unwrap();

    fs.symlink(1, "loop-b", "loop-a").unwrap();
    fs.symlink(1, "loop-a", "loop-b").unwrap();
    assert_eq!(fs.open(1, "loop-a", 0x1), Err(MemFsError::InvalidArgs));
}
