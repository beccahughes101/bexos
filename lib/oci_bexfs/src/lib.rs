//! Application of validated OCI layer plans to a staged BexFS root.

use bexos_bexfs::{BexFs, BexFsError, NodeKind};
use bexos_oci::{LayerSink, Metadata};

pub struct BexFsSink<'a> {
    filesystem: &'a mut BexFs,
    root: u64,
}

impl<'a> BexFsSink<'a> {
    pub fn new(filesystem: &'a mut BexFs, root: u64) -> Result<Self, BexFsError> {
        filesystem.read_entries(root)?;
        Ok(Self { filesystem, root })
    }

    pub fn filesystem(&self) -> &BexFs {
        self.filesystem
    }

    fn ensure_parents(&mut self, path: &str) -> Result<(), BexFsError> {
        let mut current = String::new();
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                break;
            }
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(component);
            match self.filesystem.kind_no_follow(self.root, &current) {
                Ok(NodeKind::Directory) => {}
                Ok(_) => {
                    self.filesystem.remove_tree(self.root, &current)?;
                    self.filesystem
                        .open(self.root, &current, 0x1 | 0x2 | 0x8 | 0x20)?;
                }
                Err(BexFsError::NotFound) => {
                    self.filesystem
                        .open(self.root, &current, 0x1 | 0x2 | 0x8 | 0x20)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn remove_if_present(&mut self, path: &str) -> Result<(), BexFsError> {
        match self.filesystem.remove_tree(self.root, path) {
            Ok(()) | Err(BexFsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn set_metadata(&mut self, inode: u64, metadata: Metadata) -> Result<(), BexFsError> {
        self.filesystem.set_metadata(
            inode,
            metadata.mode,
            metadata.uid,
            metadata.gid,
            metadata.mtime.saturating_mul(1_000_000_000),
        )
    }
}

impl LayerSink for BexFsSink<'_> {
    type Error = BexFsError;

    fn ensure_directory(&mut self, path: &str, metadata: Metadata) -> Result<(), Self::Error> {
        self.ensure_parents(path)?;
        if !matches!(
            self.filesystem.kind_no_follow(self.root, path),
            Ok(NodeKind::Directory)
        ) {
            self.remove_if_present(path)?;
        }
        let directory = self
            .filesystem
            .open(self.root, path, 0x1 | 0x2 | 0x8 | 0x20)?;
        self.set_metadata(directory.inode(), metadata)
    }

    fn replace_file(
        &mut self,
        path: &str,
        data: &[u8],
        metadata: Metadata,
    ) -> Result<(), Self::Error> {
        self.ensure_parents(path)?;
        self.remove_if_present(path)?;
        let mut file = self
            .filesystem
            .open(self.root, path, 0x1 | 0x2 | 0x8 | 0x10)?;
        self.filesystem.write(&mut file, data)?;
        self.set_metadata(file.inode(), metadata)
    }

    fn replace_symlink(
        &mut self,
        path: &str,
        target: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), Self::Error> {
        self.ensure_parents(path)?;
        self.remove_if_present(path)?;
        self.filesystem.symlink(self.root, target, path)?;
        let inode = self.filesystem.inode_no_follow(self.root, path)?;
        self.filesystem.set_metadata(inode, 0o777, uid, gid, 0)
    }

    fn replace_hardlink(&mut self, path: &str, target: &str) -> Result<(), Self::Error> {
        self.ensure_parents(path)?;
        self.remove_if_present(path)?;
        self.filesystem.link(self.root, target, path)
    }

    fn remove(&mut self, path: &str) -> Result<(), Self::Error> {
        self.remove_if_present(path)
    }

    fn remove_children(&mut self, path: &str) -> Result<(), Self::Error> {
        let directory = self.filesystem.open(self.root, path, 0x1 | 0x20)?.inode();
        let entries = self.filesystem.read_entries(directory)?;
        for entry in entries {
            let child = if path.is_empty() {
                entry.name
            } else {
                format!("{path}/{}", entry.name)
            };
            self.filesystem.remove_tree(self.root, &child)?;
        }
        Ok(())
    }

    fn set_xattrs(&mut self, path: &str, values: &[(String, Vec<u8>)]) -> Result<(), Self::Error> {
        let inode = self.filesystem.inode_no_follow(self.root, path)?;
        for (name, value) in values {
            self.filesystem.set_xattr(inode, name, value, 0)?;
        }
        Ok(())
    }
}
