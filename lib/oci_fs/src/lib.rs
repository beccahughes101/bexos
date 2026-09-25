//! OCI layer sink over the common filesystem protocol.

use bexos_fs_client::{Directory, Node};
use bexos_oci::{LayerSink, Metadata};
use bexos_userspace::Channel;
use fs_fidl::{FileAttributes, FsStatus, NodeKind};

pub struct FilesystemSink {
    root: Directory,
}

impl FilesystemSink {
    pub fn new(root: Channel) -> Result<Self, FsStatus> {
        let root = Directory::from_channel(root);
        root.node().get_attr()?;
        Ok(Self { root })
    }

    fn ensure_parents(&self, path: &str) -> Result<(), FsStatus> {
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
            match self.root.open_directory(&current, false) {
                Ok(_) => {}
                Err(FsStatus::NotFound) | Err(FsStatus::NotDirectory) => {
                    self.remove_if_present(&current)?;
                    self.root.open_directory(&current, true)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn parent_and_name<'a>(&self, path: &'a str) -> Result<(Option<Directory>, &'a str), FsStatus> {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        if name.is_empty() {
            return Err(FsStatus::InvalidArgs);
        }
        let directory = if parent.is_empty() {
            None
        } else {
            Some(self.root.open_directory(parent, false)?)
        };
        Ok((directory, name))
    }

    fn entries(&self, path: &str) -> Result<Vec<bexos_fs_client::OwnedDirEntry>, FsStatus> {
        if path.is_empty() {
            self.root.read_entries()
        } else {
            self.root.open_directory(path, false)?.read_entries()
        }
    }

    fn remove_tree(&self, path: &str) -> Result<(), FsStatus> {
        let (parent, name) = self.parent_and_name(path)?;
        let entries = parent.as_ref().unwrap_or(&self.root).read_entries()?;
        let entry = entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or(FsStatus::NotFound)?;
        if entry.kind == NodeKind::Directory {
            for child in self.entries(path)? {
                self.remove_tree(&format!("{path}/{}", child.name))?;
            }
        }
        parent.as_ref().unwrap_or(&self.root).unlink(name)
    }

    fn remove_if_present(&self, path: &str) -> Result<(), FsStatus> {
        match self.remove_tree(path) {
            Ok(()) | Err(FsStatus::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn metadata(&self, node: &Node, metadata: Metadata) -> Result<(), FsStatus> {
        let current = node.get_attr()?;
        node.set_attr(FileAttributes {
            size_bytes: current.size_bytes,
            storage_allocated_bytes: current.storage_allocated_bytes,
            creation_time_nanos: current.creation_time_nanos,
            modification_time_nanos: metadata.mtime.saturating_mul(1_000_000_000),
            mode: metadata.mode,
            uid: metadata.uid,
            gid: metadata.gid,
        })
    }
}

impl LayerSink for FilesystemSink {
    type Error = FsStatus;

    fn ensure_directory(&mut self, path: &str, metadata: Metadata) -> Result<(), Self::Error> {
        self.ensure_parents(path)?;
        let directory = match self.root.open_directory(path, false) {
            Ok(directory) => directory,
            Err(FsStatus::NotFound) | Err(FsStatus::NotDirectory) => {
                self.remove_if_present(path)?;
                self.root.open_directory(path, true)?
            }
            Err(error) => return Err(error),
        };
        self.metadata(directory.node(), metadata)
    }

    fn replace_file(
        &mut self,
        path: &str,
        data: &[u8],
        metadata: Metadata,
    ) -> Result<(), Self::Error> {
        self.ensure_parents(path)?;
        self.remove_if_present(path)?;
        let file = self.root.open_file(path, true, true)?;
        file.write_all(data)?;
        self.metadata(file.node(), metadata)
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
        self.root.symlink(target, path)?;
        let node = self.root.open_no_follow(path)?;
        self.metadata(
            &node,
            Metadata {
                mode: 0o777,
                uid,
                gid,
                mtime: 0,
            },
        )
    }

    fn replace_hardlink(&mut self, path: &str, target: &str) -> Result<(), Self::Error> {
        self.ensure_parents(path)?;
        self.remove_if_present(path)?;
        self.root.link(target, path)
    }

    fn remove(&mut self, path: &str) -> Result<(), Self::Error> {
        self.remove_if_present(path)
    }

    fn remove_children(&mut self, path: &str) -> Result<(), Self::Error> {
        for child in self.entries(path)? {
            let child = if path.is_empty() {
                child.name
            } else {
                format!("{path}/{}", child.name)
            };
            self.remove_tree(&child)?;
        }
        Ok(())
    }

    fn set_xattrs(&mut self, path: &str, values: &[(String, Vec<u8>)]) -> Result<(), Self::Error> {
        let node = self.root.open_no_follow(path)?;
        for (name, value) in values {
            node.set_xattr(name, value, 0)?;
        }
        Ok(())
    }
}
