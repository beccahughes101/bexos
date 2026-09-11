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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeAttributes {
    pub size_bytes: u64,
    pub storage_allocated_bytes: u64,
    pub creation_time_nanos: u64,
    pub modification_time_nanos: u64,
    pub mode: u32,
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
    parent: u64,
    name: String,
    kind: NodeKind,
    attributes: NodeAttributes,
    data: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemFs {
    next_inode: u64,
    nodes: BTreeMap<u64, Node>,
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
            parent: ROOT_INODE,
            name: String::new(),
            kind: NodeKind::Directory,
            attributes: NodeAttributes {
                size_bytes: 0,
                storage_allocated_bytes: 0,
                creation_time_nanos: 0,
                modification_time_nanos: 0,
                mode: 0o700,
            },
            data: Vec::new(),
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(ROOT_INODE, root);
        Self {
            next_inode: ROOT_INODE + 1,
            nodes,
        }
    }

    pub fn open(&mut self, base: u64, path: &str, flags: u32) -> Result<OpenedNode, MemFsError> {
        let readable = flags & 0x1 != 0;
        let writable = flags & 0x2 != 0;
        let create = flags & 0x8 != 0;
        let truncate = flags & 0x10 != 0;
        let require_directory = flags & 0x20 != 0;
        let inode = match self.resolve(base, path) {
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

    pub fn read_entries(&self, inode: u64) -> Result<Vec<DirectoryEntry>, MemFsError> {
        let directory = self.nodes.get(&inode).ok_or(MemFsError::NotFound)?;
        if directory.kind != NodeKind::Directory {
            return Err(MemFsError::NotDirectory);
        }
        Ok(self
            .nodes
            .values()
            .filter(|node| node.inode != ROOT_INODE && node.parent == inode)
            .map(|node| DirectoryEntry {
                name: node.name.clone(),
                kind: node.kind,
                attributes: node.attributes,
            })
            .collect())
    }

    pub fn unlink(&mut self, directory: u64, name: &str) -> Result<(), MemFsError> {
        validate_component(name)?;
        let inode = self
            .nodes
            .values()
            .find(|node| node.parent == directory && node.name == name)
            .map(|node| node.inode)
            .ok_or(MemFsError::NotFound)?;
        if self
            .nodes
            .values()
            .any(|node| node.parent == inode && node.inode != inode)
        {
            return Err(MemFsError::NotEmpty);
        }
        self.nodes.remove(&inode);
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
        w.word(1);
        w.word(self.next_inode);
        w.word(self.nodes.len() as u64);
        for node in self.nodes.values() {
            w.word(node.inode);
            w.word(node.parent);
            w.text(&node.name);
            w.word(match node.kind {
                NodeKind::File => 1,
                NodeKind::Directory => 2,
            });
            w.word(node.attributes.size_bytes);
            w.word(node.attributes.storage_allocated_bytes);
            w.word(node.attributes.creation_time_nanos);
            w.word(node.attributes.modification_time_nanos);
            w.word(node.attributes.mode as u64);
            w.bytes(&node.data);
        }
        w.finish()
    }

    pub fn adopt(bytes: &[u8]) -> Result<Self, MemFsError> {
        let mut r = Decoder::new(bytes);
        if r.word().map_err(|_| MemFsError::InvalidArgs)? != 1 {
            return Err(MemFsError::InvalidArgs);
        }
        let next_inode = r.word().map_err(|_| MemFsError::InvalidArgs)?;
        let count = r.count(4096).map_err(|_| MemFsError::InvalidArgs)?;
        let mut nodes = BTreeMap::new();
        for _ in 0..count {
            let inode = r.word().map_err(|_| MemFsError::InvalidArgs)?;
            let parent = r.word().map_err(|_| MemFsError::InvalidArgs)?;
            let name = r
                .text(255)
                .map_err(|_| MemFsError::InvalidArgs)?
                .to_string();
            let kind = match r.word().map_err(|_| MemFsError::InvalidArgs)? {
                1 => NodeKind::File,
                2 => NodeKind::Directory,
                _ => return Err(MemFsError::InvalidArgs),
            };
            let attributes = NodeAttributes {
                size_bytes: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                storage_allocated_bytes: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                creation_time_nanos: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                modification_time_nanos: r.word().map_err(|_| MemFsError::InvalidArgs)?,
                mode: u32::try_from(r.word().map_err(|_| MemFsError::InvalidArgs)?)
                    .map_err(|_| MemFsError::InvalidArgs)?,
            };
            let data = r
                .bytes(16 * 1024 * 1024)
                .map_err(|_| MemFsError::InvalidArgs)?
                .to_vec();
            nodes.insert(
                inode,
                Node {
                    inode,
                    parent,
                    name,
                    kind,
                    attributes,
                    data,
                },
            );
        }
        r.finish().map_err(|_| MemFsError::InvalidArgs)?;
        let fs = Self { next_inode, nodes };
        fs.validate()?;
        Ok(fs)
    }

    fn validate(&self) -> Result<(), MemFsError> {
        let root = self.nodes.get(&ROOT_INODE).ok_or(MemFsError::InvalidArgs)?;
        if root.parent != ROOT_INODE || root.kind != NodeKind::Directory || !root.name.is_empty() {
            return Err(MemFsError::InvalidArgs);
        }
        for node in self.nodes.values() {
            if node.inode != ROOT_INODE {
                validate_component(&node.name)?;
                let parent = self
                    .nodes
                    .get(&node.parent)
                    .ok_or(MemFsError::InvalidArgs)?;
                if parent.kind != NodeKind::Directory {
                    return Err(MemFsError::InvalidArgs);
                }
            }
            if node.kind == NodeKind::File && node.attributes.size_bytes != node.data.len() as u64 {
                return Err(MemFsError::InvalidArgs);
            }
        }
        Ok(())
    }

    fn resolve(&self, base: u64, path: &str) -> Result<u64, MemFsError> {
        let components = validate_relative_path(path)?;
        let mut current = base;
        if !self.nodes.contains_key(&current) {
            return Err(MemFsError::NotFound);
        }
        for component in components {
            let parent = self.nodes.get(&current).ok_or(MemFsError::NotFound)?;
            if parent.kind != NodeKind::Directory {
                return Err(MemFsError::NotDirectory);
            }
            current = self
                .nodes
                .values()
                .find(|node| node.parent == current && node.name == component)
                .map(|node| node.inode)
                .ok_or(MemFsError::NotFound)?;
        }
        Ok(current)
    }

    fn create_path(&mut self, base: u64, path: &str, directory: bool) -> Result<u64, MemFsError> {
        let components = validate_relative_path(path)?;
        let (name, parents) = components.split_last().ok_or(MemFsError::InvalidArgs)?;
        let mut parent = base;
        for component in parents {
            parent = self.resolve(parent, component)?;
        }
        if self
            .nodes
            .values()
            .any(|node| node.parent == parent && node.name == *name)
        {
            return Err(MemFsError::AlreadyExists);
        }
        let parent_node = self.nodes.get(&parent).ok_or(MemFsError::NotFound)?;
        if parent_node.kind != NodeKind::Directory {
            return Err(MemFsError::NotDirectory);
        }
        let inode = self.next_inode;
        self.next_inode = inode.checked_add(1).ok_or(MemFsError::NoSpace)?;
        self.nodes.insert(
            inode,
            Node {
                inode,
                parent,
                name: (*name).to_string(),
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
                },
                data: Vec::new(),
            },
        );
        Ok(inode)
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
