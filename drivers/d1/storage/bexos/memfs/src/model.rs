use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bexos_migration::codec::{Decoder, Encoder};

pub const ROOT_INODE: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemFsError {
    Io,
    NoSpace,
    NotFound,
    NotDirectory,
    IsDirectory,
    NotEmpty,
    AlreadyExists,
    AccessDenied,
    InvalidArgs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeAttributes {
    pub size_bytes: u64,
    pub storage_allocated_bytes: u64,
    pub creation_time_nanos: u64,
    pub modification_time_nanos: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub name: String,
    pub kind: NodeKind,
    pub attributes: NodeAttributes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenedNode {
    inode: u64,
    cursor: u64,
    readable: bool,
    writable: bool,
    kind: NodeKind,
}

impl OpenedNode {
    pub const fn inode(&self) -> u64 {
        self.inode
    }

    pub const fn kind(&self) -> NodeKind {
        self.kind
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.inode);
        w.word(self.cursor);
        w.word(self.readable as u64);
        w.word(self.writable as u64);
        w.word(match self.kind {
            NodeKind::File => 1,
            NodeKind::Directory => 2,
            NodeKind::Symlink => 3,
        });
        w.finish()
    }

    pub fn adopt(bytes: &[u8]) -> Result<Self, MemFsError> {
        let mut r = Decoder::new(bytes);
        if r.word().map_err(|_| MemFsError::InvalidArgs)? != 1 {
            return Err(MemFsError::InvalidArgs);
        }
        let inode = r.word().map_err(|_| MemFsError::InvalidArgs)?;
        let cursor = r.word().map_err(|_| MemFsError::InvalidArgs)?;
        let readable = r.flag().map_err(|_| MemFsError::InvalidArgs)?;
        let writable = r.flag().map_err(|_| MemFsError::InvalidArgs)?;
        let kind = match r.word().map_err(|_| MemFsError::InvalidArgs)? {
            1 => NodeKind::File,
            2 => NodeKind::Directory,
            3 => NodeKind::Symlink,
            _ => return Err(MemFsError::InvalidArgs),
        };
        r.finish().map_err(|_| MemFsError::InvalidArgs)?;
        Ok(Self {
            inode,
            cursor,
            readable,
            writable,
            kind,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Node {
    inode: u64,
    kind: NodeKind,
    attributes: NodeAttributes,
    data: Vec<u8>,
    xattrs: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Dentry {
    parent: u64,
    name: String,
    inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemFs {
    next_inode: u64,
    nodes: BTreeMap<u64, Node>,
    dentries: BTreeMap<(u64, String), u64>,
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemFs {
    pub fn new() -> Self {
        let root = Node {
            inode: ROOT_INODE,
            kind: NodeKind::Directory,
            attributes: NodeAttributes {
                size_bytes: 0,
                storage_allocated_bytes: 0,
                creation_time_nanos: 0,
                modification_time_nanos: 0,
                mode: 0o700,
                uid: 0,
                gid: 0,
            },
            data: Vec::new(),
            xattrs: BTreeMap::new(),
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(ROOT_INODE, root);
        Self {
            next_inode: ROOT_INODE + 1,
            nodes,
            dentries: BTreeMap::new(),
        }
    }

    pub fn open(&mut self, base: u64, path: &str, flags: u32) -> Result<OpenedNode, MemFsError> {
        let readable = flags & 0x1 != 0;
        let writable = flags & 0x2 != 0;
        let create = flags & 0x8 != 0;
        let truncate = flags & 0x10 != 0;
        let require_directory = flags & 0x20 != 0;
        let no_follow = flags & 0x40 != 0;
        let resolved = if no_follow {
            self.resolve_no_follow(base, path)
        } else {
            self.resolve(base, path)
        };
        let inode = match resolved {
            Ok(inode) => inode,
            Err(MemFsError::NotFound) if create => {
                self.create_path(base, path, require_directory)?
            }
            Err(error) => return Err(error),
        };
        let node = self.nodes.get_mut(&inode).ok_or(MemFsError::NotFound)?;
        if require_directory && node.kind != NodeKind::Directory {
            return Err(MemFsError::NotDirectory);
        }
        if !require_directory && node.kind == NodeKind::Directory {
            return Err(MemFsError::IsDirectory);
        }
        if truncate {
            if !writable {
                return Err(MemFsError::AccessDenied);
            }
            node.data.clear();
            update_size(node);
        }
        Ok(OpenedNode {
            inode,
            cursor: 0,
            readable,
            writable,
            kind: node.kind,
        })
    }

    pub fn read(&self, handle: &mut OpenedNode, count: u64) -> Result<Vec<u8>, MemFsError> {
        if !handle.readable {
            return Err(MemFsError::AccessDenied);
        }
        let node = self.nodes.get(&handle.inode).ok_or(MemFsError::NotFound)?;
        if node.kind == NodeKind::Directory {
            return Err(MemFsError::IsDirectory);
        }
        let start = usize::try_from(handle.cursor).map_err(|_| MemFsError::InvalidArgs)?;
        let requested = usize::try_from(count).map_err(|_| MemFsError::InvalidArgs)?;
        let end = start.saturating_add(requested).min(node.data.len());
        let bytes = if start >= node.data.len() {
            Vec::new()
        } else {
            node.data[start..end].to_vec()
        };
        handle.cursor = handle.cursor.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }

    pub fn write(&mut self, handle: &mut OpenedNode, bytes: &[u8]) -> Result<u64, MemFsError> {
        if !handle.writable {
            return Err(MemFsError::AccessDenied);
        }
        let node = self
            .nodes
            .get_mut(&handle.inode)
            .ok_or(MemFsError::NotFound)?;
        if node.kind == NodeKind::Directory {
            return Err(MemFsError::IsDirectory);
        }
        let start = usize::try_from(handle.cursor).map_err(|_| MemFsError::NoSpace)?;
        let end = start.checked_add(bytes.len()).ok_or(MemFsError::NoSpace)?;
        if node.data.len() < end {
            node.data.resize(end, 0);
        }
        node.data[start..end].copy_from_slice(bytes);
        handle.cursor = end as u64;
        update_size(node);
        Ok(bytes.len() as u64)
    }

    pub fn set_len(&mut self, handle: &OpenedNode, len: u64) -> Result<(), MemFsError> {
        if !handle.writable {
            return Err(MemFsError::AccessDenied);
        }
        let node = self
            .nodes
            .get_mut(&handle.inode)
            .ok_or(MemFsError::NotFound)?;
        if node.kind == NodeKind::Directory {
            return Err(MemFsError::IsDirectory);
        }
        let len = usize::try_from(len).map_err(|_| MemFsError::NoSpace)?;
        node.data.resize(len, 0);
        update_size(node);
        Ok(())
    }

    pub fn seek(
        &self,
        handle: &mut OpenedNode,
        offset: i64,
        whence: u8,
    ) -> Result<u64, MemFsError> {
        let node = self.nodes.get(&handle.inode).ok_or(MemFsError::NotFound)?;
        let base = match whence {
            0 => 0i128,
            1 => i128::from(handle.cursor),
            2 => node.data.len() as i128,
            _ => return Err(MemFsError::InvalidArgs),
        };
        let next = base + i128::from(offset);
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(MemFsError::InvalidArgs);
        }
        handle.cursor = next as u64;
        Ok(handle.cursor)
    }

    pub fn attributes(&self, inode: u64) -> Result<NodeAttributes, MemFsError> {
        self.nodes
            .get(&inode)
            .map(|node| node.attributes)
            .ok_or(MemFsError::NotFound)
    }

    pub fn set_metadata(
        &mut self,
        inode: u64,
        attributes: NodeAttributes,
    ) -> Result<(), MemFsError> {
        let node = self.nodes.get_mut(&inode).ok_or(MemFsError::NotFound)?;
        node.attributes.creation_time_nanos = attributes.creation_time_nanos;
        node.attributes.modification_time_nanos = attributes.modification_time_nanos;
        node.attributes.mode = attributes.mode & 0o7777;
        node.attributes.uid = attributes.uid;
        node.attributes.gid = attributes.gid;
        Ok(())
    }

    pub fn read_entries(&self, inode: u64) -> Result<Vec<DirectoryEntry>, MemFsError> {
        let directory = self.nodes.get(&inode).ok_or(MemFsError::NotFound)?;
        if directory.kind != NodeKind::Directory {
            return Err(MemFsError::NotDirectory);
        }
        Ok(self
            .dentries
            .iter()
            .filter(|((parent, _), _)| *parent == inode)
            .filter_map(|((_, name), child)| self.nodes.get(child).map(|node| (name, node)))
            .map(|(name, node)| DirectoryEntry {
                name: name.clone(),
                kind: node.kind,
                attributes: node.attributes,
            })
            .collect())
    }

    pub fn unlink(&mut self, directory: u64, name: &str) -> Result<(), MemFsError> {
        validate_component(name)?;
        self.require_directory(directory)?;
        let key = (directory, name.to_string());
        let inode = *self.dentries.get(&key).ok_or(MemFsError::NotFound)?;
        if self.has_children(inode) {
            return Err(MemFsError::NotEmpty);
        }
        self.dentries.remove(&key);
        self.remove_unlinked_inode(inode);
        Ok(())
    }

    pub fn link(&mut self, base: u64, source: &str, target: &str) -> Result<(), MemFsError> {
        let source_inode = self.resolve_no_follow(base, source)?;
        if self
            .nodes
            .get(&source_inode)
            .is_none_or(|node| node.kind == NodeKind::Directory)
        {
            return Err(MemFsError::AccessDenied);
        }
        let (target_parent, target_name) = self.resolve_parent(base, target)?;
        let key = (target_parent, target_name.to_string());
        if self.dentries.contains_key(&key) {
            return Err(MemFsError::AlreadyExists);
        }
        self.dentries.insert(key, source_inode);
        Ok(())
    }

    pub fn symlink(&mut self, base: u64, target: &str, link_path: &str) -> Result<(), MemFsError> {
        validate_symlink_target(target)?;
        let (parent, name) = self.resolve_parent(base, link_path)?;
        let key = (parent, name.to_string());
        if self.dentries.contains_key(&key) {
            return Err(MemFsError::AlreadyExists);
        }
        let inode = self.next_inode;
        self.next_inode = inode.checked_add(1).ok_or(MemFsError::NoSpace)?;
        let data = target.as_bytes().to_vec();
        self.nodes.insert(
            inode,
            Node {
                inode,
                kind: NodeKind::Symlink,
                attributes: NodeAttributes {
                    size_bytes: data.len() as u64,
                    storage_allocated_bytes: data.len() as u64,
                    creation_time_nanos: 0,
                    modification_time_nanos: 0,
                    mode: 0o777,
                    uid: 0,
                    gid: 0,
                },
                data,
                xattrs: BTreeMap::new(),
            },
        );
        self.dentries.insert(key, inode);
        Ok(())
    }

    pub fn readlink(&self, base: u64, path: &str) -> Result<String, MemFsError> {
        let inode = self.resolve_no_follow(base, path)?;
        let node = self.nodes.get(&inode).ok_or(MemFsError::NotFound)?;
        if node.kind != NodeKind::Symlink {
            return Err(MemFsError::InvalidArgs);
        }
        core::str::from_utf8(&node.data)
            .map(ToString::to_string)
            .map_err(|_| MemFsError::InvalidArgs)
    }

    pub fn get_xattr(&self, inode: u64, name: &str) -> Result<Vec<u8>, MemFsError> {
        validate_xattr_name(name)?;
        self.nodes
            .get(&inode)
            .ok_or(MemFsError::NotFound)?
            .xattrs
            .get(name)
            .cloned()
            .ok_or(MemFsError::NotFound)
    }

    pub fn set_xattr(
        &mut self,
        inode: u64,
        name: &str,
        value: &[u8],
        flags: u32,
    ) -> Result<(), MemFsError> {
        validate_xattr_name(name)?;
        if value.len() > 65_536 || flags & !3 != 0 || flags == 3 {
            return Err(MemFsError::InvalidArgs);
        }
        let node = self.nodes.get_mut(&inode).ok_or(MemFsError::NotFound)?;
        let exists = node.xattrs.contains_key(name);
        if flags == 1 && exists || flags == 2 && !exists {
            return Err(if exists {
                MemFsError::AlreadyExists
            } else {
                MemFsError::NotFound
            });
        }
        node.xattrs.insert(name.to_string(), value.to_vec());
        Ok(())
    }

    pub fn list_xattrs(&self, inode: u64) -> Result<Vec<String>, MemFsError> {
        Ok(self
            .nodes
            .get(&inode)
            .ok_or(MemFsError::NotFound)?
            .xattrs
            .keys()
            .cloned()
            .collect())
    }

    pub fn remove_xattr(&mut self, inode: u64, name: &str) -> Result<(), MemFsError> {
        validate_xattr_name(name)?;
        self.nodes
            .get_mut(&inode)
            .ok_or(MemFsError::NotFound)?
            .xattrs
            .remove(name)
            .map(|_| ())
            .ok_or(MemFsError::NotFound)
    }

    pub fn rename(&mut self, base: u64, source: &str, target: &str) -> Result<(), MemFsError> {
        let source_inode = self.resolve(base, source)?;
        if source_inode == ROOT_INODE {
            return Err(MemFsError::InvalidArgs);
        }
        let (source_parent, source_name) = self.resolve_parent(base, source)?;
        let source_key = (source_parent, source_name.to_string());
        let (target_parent, target_name) = self.resolve_parent(base, target)?;
        let target_key = (target_parent, target_name.to_string());
        let source_kind = self
            .nodes
            .get(&source_inode)
            .ok_or(MemFsError::NotFound)?
            .kind;
        if source_kind == NodeKind::Directory {
            let mut ancestor = target_parent;
            loop {
                if ancestor == source_inode {
                    return Err(MemFsError::InvalidArgs);
                }
                if ancestor == ROOT_INODE {
                    break;
                }
                ancestor = self.directory_parent(ancestor)?;
            }
        }
        let replaced = self
            .dentries
            .get(&target_key)
            .and_then(|inode| self.nodes.get(inode).map(|node| (*inode, node.kind)));
        if let Some((inode, kind)) = replaced {
            if inode == source_inode && source_key == target_key {
                return Ok(());
            }
            if source_kind == NodeKind::Directory && kind != NodeKind::Directory {
                return Err(MemFsError::NotDirectory);
            }
            if source_kind != NodeKind::Directory && kind == NodeKind::Directory {
                return Err(MemFsError::IsDirectory);
            }
            if self.has_children(inode) {
                return Err(MemFsError::NotEmpty);
            }
            self.dentries.remove(&target_key);
            self.remove_unlinked_inode(inode);
        }
        self.dentries
            .remove(&source_key)
            .ok_or(MemFsError::NotFound)?;
        self.dentries.insert(target_key, source_inode);
        Ok(())
    }

    pub fn backing_bytes(&self, handle: &OpenedNode) -> Result<&[u8], MemFsError> {
        let node = self.nodes.get(&handle.inode).ok_or(MemFsError::NotFound)?;
        if node.kind == NodeKind::Directory {
            return Err(MemFsError::IsDirectory);
        }
        Ok(&node.data)
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(4);
        w.word(self.next_inode);
        w.word(self.nodes.len() as u64);
        for node in self.nodes.values() {
            w.word(node.inode);
            w.word(match node.kind {
                NodeKind::File => 1,
                NodeKind::Directory => 2,
                NodeKind::Symlink => 3,
            });
            w.word(node.attributes.size_bytes);
            w.word(node.attributes.storage_allocated_bytes);
            w.word(node.attributes.creation_time_nanos);
            w.word(node.attributes.modification_time_nanos);
            w.word(node.attributes.mode as u64);
            w.word(node.attributes.uid as u64);
            w.word(node.attributes.gid as u64);
            w.bytes(&node.data);
            w.word(node.xattrs.len() as u64);
            for (name, value) in &node.xattrs {
                w.text(name);
                w.bytes(value);
            }
        }
        w.word(self.dentries.len() as u64);
        for dentry in self.dentries() {
            w.word(dentry.parent);
            w.text(&dentry.name);
            w.word(dentry.inode);
        }
        w.finish()
    }

    pub fn adopt(bytes: &[u8]) -> Result<Self, MemFsError> {
        let mut r = Decoder::new(bytes);
        let version = r.word().map_err(|_| MemFsError::InvalidArgs)?;
        if version != 1 && version != 2 && version != 3 && version != 4 {
            return Err(MemFsError::InvalidArgs);
        }
        let next_inode = r.word().map_err(|_| MemFsError::InvalidArgs)?;
        let count = r.count(4096).map_err(|_| MemFsError::InvalidArgs)?;
        let mut nodes = BTreeMap::new();
        let mut dentries = BTreeMap::new();
        for _ in 0..count {
            let inode = r.word().map_err(|_| MemFsError::InvalidArgs)?;
            let legacy_dentry = if version == 1 {
                let parent = r.word().map_err(|_| MemFsError::InvalidArgs)?;
                let name = r
                    .text(255)
                    .map_err(|_| MemFsError::InvalidArgs)?
                    .to_string();
                Some((parent, name))
            } else {
                None
            };
            let kind = match r.word().map_err(|_| MemFsError::InvalidArgs)? {
                1 => NodeKind::File,
                2 => NodeKind::Directory,
                3 if version >= 3 => NodeKind::Symlink,
                _ => return Err(MemFsError::InvalidArgs),
            };
            let mut attributes = NodeAttributes {
                size_bytes: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                storage_allocated_bytes: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                creation_time_nanos: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                modification_time_nanos: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                mode: u32::try_from(r.word().map_err(|_| MemFsError::InvalidArgs)?)
                    .map_err(|_| MemFsError::InvalidArgs)?,
                uid: 0,
                gid: 0,
            };
            if version >= 4 {
                attributes.uid = u32::try_from(r.word().map_err(|_| MemFsError::InvalidArgs)?)
                    .map_err(|_| MemFsError::InvalidArgs)?;
                attributes.gid = u32::try_from(r.word().map_err(|_| MemFsError::InvalidArgs)?)
                    .map_err(|_| MemFsError::InvalidArgs)?;
            }
            let data = r
                .bytes(16 * 1024 * 1024)
                .map_err(|_| MemFsError::InvalidArgs)?
                .to_vec();
            let mut xattrs = BTreeMap::new();
            if version >= 3 {
                for _ in 0..r.count(256).map_err(|_| MemFsError::InvalidArgs)? {
                    let name = r
                        .text(255)
                        .map_err(|_| MemFsError::InvalidArgs)?
                        .to_string();
                    validate_xattr_name(&name)?;
                    let value = r
                        .bytes(65_536)
                        .map_err(|_| MemFsError::InvalidArgs)?
                        .to_vec();
                    if xattrs.insert(name, value).is_some() {
                        return Err(MemFsError::InvalidArgs);
                    }
                }
            }
            nodes.insert(
                inode,
                Node {
                    inode,
                    kind,
                    attributes,
                    data,
                    xattrs,
                },
            );
            if let Some((parent, name)) = legacy_dentry {
                if inode != ROOT_INODE {
                    dentries.insert((parent, name), inode);
                }
            }
        }
        if version >= 2 {
            let count = r.count(16_384).map_err(|_| MemFsError::InvalidArgs)?;
            for _ in 0..count {
                let parent = r.word().map_err(|_| MemFsError::InvalidArgs)?;
                let name = r
                    .text(255)
                    .map_err(|_| MemFsError::InvalidArgs)?
                    .to_string();
                let inode = r.word().map_err(|_| MemFsError::InvalidArgs)?;
                if dentries.insert((parent, name), inode).is_some() {
                    return Err(MemFsError::InvalidArgs);
                }
            }
        }
        r.finish().map_err(|_| MemFsError::InvalidArgs)?;
        let fs = Self {
            next_inode,
            nodes,
            dentries,
        };
        fs.validate()?;
        Ok(fs)
    }

    fn validate(&self) -> Result<(), MemFsError> {
        let root = self.nodes.get(&ROOT_INODE).ok_or(MemFsError::InvalidArgs)?;
        if root.kind != NodeKind::Directory {
            return Err(MemFsError::InvalidArgs);
        }
        for node in self.nodes.values() {
            if node.kind == NodeKind::File && node.attributes.size_bytes != node.data.len() as u64 {
                return Err(MemFsError::InvalidArgs);
            }
        }
        for ((parent, name), inode) in &self.dentries {
            validate_component(name)?;
            if *inode == ROOT_INODE || !self.nodes.contains_key(inode) {
                return Err(MemFsError::InvalidArgs);
            }
            if self
                .nodes
                .get(parent)
                .is_none_or(|node| node.kind != NodeKind::Directory)
            {
                return Err(MemFsError::InvalidArgs);
            }
        }
        for inode in self
            .nodes
            .keys()
            .copied()
            .filter(|inode| *inode != ROOT_INODE)
        {
            let links = self
                .dentries
                .values()
                .filter(|child| **child == inode)
                .count();
            if links == 0 {
                return Err(MemFsError::InvalidArgs);
            }
            if self.nodes[&inode].kind == NodeKind::Directory && links != 1 {
                return Err(MemFsError::InvalidArgs);
            }
        }
        for inode in self
            .nodes
            .values()
            .filter(|node| node.kind == NodeKind::Directory)
            .map(|node| node.inode)
        {
            self.validate_directory_ancestry(inode)?;
        }
        Ok(())
    }

    fn resolve(&self, base: u64, path: &str) -> Result<u64, MemFsError> {
        self.resolve_path(base, path, true)
    }

    fn resolve_no_follow(&self, base: u64, path: &str) -> Result<u64, MemFsError> {
        self.resolve_path(base, path, false)
    }

    fn resolve_path(&self, base: u64, path: &str, follow_final: bool) -> Result<u64, MemFsError> {
        let components = validate_relative_path(path)?
            .into_iter()
            .map(ToString::to_string)
            .collect();
        self.walk(base, components, follow_final, 0)
    }

    fn walk(
        &self,
        mut current: u64,
        components: Vec<String>,
        follow_final: bool,
        depth: usize,
    ) -> Result<u64, MemFsError> {
        if depth > 40 || !self.nodes.contains_key(&current) {
            return Err(if depth > 40 {
                MemFsError::InvalidArgs
            } else {
                MemFsError::NotFound
            });
        }
        let count = components.len();
        for (index, component) in components.iter().enumerate() {
            match component.as_str() {
                "." => continue,
                ".." => {
                    self.require_directory(current)?;
                    current = self.directory_parent(current)?;
                    continue;
                }
                _ => {}
            }
            self.require_directory(current)?;
            let parent = current;
            current = self
                .dentries
                .get(&(parent, component.clone()))
                .copied()
                .ok_or(MemFsError::NotFound)?;
            let final_component = index + 1 == count;
            let node = self.nodes.get(&current).ok_or(MemFsError::NotFound)?;
            if node.kind == NodeKind::Symlink && (!final_component || follow_final) {
                let target =
                    core::str::from_utf8(&node.data).map_err(|_| MemFsError::InvalidArgs)?;
                let mut target_components = target
                    .split('/')
                    .filter(|component| !component.is_empty())
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                target_components.extend(components[index + 1..].iter().cloned());
                let start = if target.starts_with('/') {
                    ROOT_INODE
                } else {
                    parent
                };
                return self.walk(start, target_components, follow_final, depth + 1);
            }
        }
        Ok(current)
    }

    fn create_path(&mut self, base: u64, path: &str, directory: bool) -> Result<u64, MemFsError> {
        let (parent, name) = self.resolve_parent(base, path)?;
        let key = (parent, name.to_string());
        if self.dentries.contains_key(&key) {
            return Err(MemFsError::AlreadyExists);
        }
        let inode = self.next_inode;
        self.next_inode = inode.checked_add(1).ok_or(MemFsError::NoSpace)?;
        self.nodes.insert(
            inode,
            Node {
                inode,
                kind: if directory {
                    NodeKind::Directory
                } else {
                    NodeKind::File
                },
                attributes: NodeAttributes {
                    size_bytes: 0,
                    storage_allocated_bytes: 0,
                    creation_time_nanos: 0,
                    modification_time_nanos: 0,
                    mode: if directory { 0o700 } else { 0o600 },
                    uid: 0,
                    gid: 0,
                },
                data: Vec::new(),
                xattrs: BTreeMap::new(),
            },
        );
        self.dentries.insert(key, inode);
        Ok(inode)
    }

    fn resolve_parent<'a>(&self, base: u64, path: &'a str) -> Result<(u64, &'a str), MemFsError> {
        let components = validate_relative_path(path)?;
        let (name, parents) = components.split_last().ok_or(MemFsError::InvalidArgs)?;
        let mut parent = base;
        for component in parents {
            parent = self.resolve(parent, component)?;
        }
        self.require_directory(parent)?;
        Ok((parent, name))
    }

    fn require_directory(&self, inode: u64) -> Result<(), MemFsError> {
        match self.nodes.get(&inode) {
            Some(node) if node.kind == NodeKind::Directory => Ok(()),
            Some(_) => Err(MemFsError::NotDirectory),
            None => Err(MemFsError::NotFound),
        }
    }

    fn has_children(&self, inode: u64) -> bool {
        self.dentries.keys().any(|(parent, _)| *parent == inode)
    }

    fn remove_unlinked_inode(&mut self, inode: u64) {
        if inode != ROOT_INODE && !self.dentries.values().any(|child| *child == inode) {
            self.nodes.remove(&inode);
        }
    }

    fn directory_parent(&self, inode: u64) -> Result<u64, MemFsError> {
        if inode == ROOT_INODE {
            return Ok(ROOT_INODE);
        }
        self.dentries
            .iter()
            .find(|(_, child)| **child == inode)
            .map(|((parent, _), _)| *parent)
            .ok_or(MemFsError::InvalidArgs)
    }

    fn validate_directory_ancestry(&self, inode: u64) -> Result<(), MemFsError> {
        let mut current = inode;
        for _ in 0..self.nodes.len() {
            if current == ROOT_INODE {
                return Ok(());
            }
            current = self.directory_parent(current)?;
        }
        Err(MemFsError::InvalidArgs)
    }

    fn dentries(&self) -> impl Iterator<Item = Dentry> + '_ {
        self.dentries.iter().map(|((parent, name), inode)| Dentry {
            parent: *parent,
            name: name.clone(),
            inode: *inode,
        })
    }
}

fn update_size(node: &mut Node) {
    node.attributes.size_bytes = node.data.len() as u64;
    node.attributes.storage_allocated_bytes = node.data.len() as u64;
}

fn validate_component(component: &str) -> Result<(), MemFsError> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.len() > 255
        || component.contains('/')
    {
        return Err(MemFsError::InvalidArgs);
    }
    Ok(())
}

fn validate_symlink_target(target: &str) -> Result<(), MemFsError> {
    if target.is_empty() || target.len() > 4095 || target.as_bytes().contains(&0) {
        return Err(MemFsError::InvalidArgs);
    }
    Ok(())
}

fn validate_xattr_name(name: &str) -> Result<(), MemFsError> {
    if name.is_empty() || name.len() > 255 || name.as_bytes().contains(&0) || !name.contains('.') {
        return Err(MemFsError::InvalidArgs);
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<Vec<&str>, MemFsError> {
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Err(MemFsError::InvalidArgs);
    }
    let components = path.split('/').collect::<Vec<_>>();
    for component in &components {
        validate_component(component)?;
    }
    Ok(components)
}
