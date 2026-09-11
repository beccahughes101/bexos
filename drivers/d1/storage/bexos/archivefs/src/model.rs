mod migration;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bexos_app_archive::{Compression, OpenArchive, TrustedKey};

pub const QEMU_TEST_KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
pub const QEMU_TEST_PUBLIC_KEY: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArchiveFsError {
    Corrupt,
    NotFound,
    NotDirectory,
    IsDirectory,
    ReadOnly,
    AccessDenied,
    InvalidArgs,
}

#[derive(Clone, Debug)]
enum Node {
    Directory { children: BTreeMap<String, u64> },
    File { data: FileData, mode: u32 },
}

#[derive(Clone, Debug)]
enum FileData {
    Owned(Vec<u8>),
    Archive {
        offset: usize,
        len: usize,
    },
    CompressedArchive {
        offset: usize,
        len: usize,
        uncompressed_len: usize,
    },
}

impl FileData {
    fn len(&self) -> usize {
        match self {
            Self::Owned(bytes) => bytes.len(),
            Self::Archive { len, .. } => *len,
            Self::CompressedArchive {
                uncompressed_len, ..
            } => *uncompressed_len,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileHandle {
    inode: u64,
    offset: u64,
}

impl FileHandle {
    pub fn inode(&self) -> u64 {
        self.inode
    }
}

#[derive(Clone, Debug)]
pub struct OpenNode {
    pub inode: u64,
    pub file: Option<FileHandle>,
}

#[derive(Clone, Debug)]
pub struct ArchiveFs {
    archive_bytes: Vec<u8>,
    content_root: [u8; 32],
    nodes: Vec<Node>,
}

impl ArchiveFs {
    pub fn mount(bytes: &[u8], expected_root: Option<[u8; 32]>) -> Result<Self, ArchiveFsError> {
        Self::mount_owned(bytes.to_vec(), expected_root)
    }

    pub fn mount_owned(
        bytes: Vec<u8>,
        expected_root: Option<[u8; 32]>,
    ) -> Result<Self, ArchiveFsError> {
        let trusted = [TrustedKey {
            key_id: QEMU_TEST_KEY_ID,
            public_key: &QEMU_TEST_PUBLIC_KEY,
        }];
        let archive =
            OpenArchive::parse_and_verify(&bytes, &trusted).map_err(|_| ArchiveFsError::Corrupt)?;
        let content_root = archive.content_root();
        if let Some(expected) = expected_root {
            if content_root != expected {
                return Err(ArchiveFsError::Corrupt);
            }
        }
        let mut entries = Vec::new();
        let mut needs_archive_bytes = false;
        for entry in archive.entries() {
            let data = match entry.compression {
                Compression::None => {
                    if entry.stored_len != entry.uncompressed_size {
                        return Err(ArchiveFsError::Corrupt);
                    }
                    needs_archive_bytes = true;
                    FileData::Archive {
                        offset: entry.stored_offset as usize,
                        len: entry.stored_len as usize,
                    }
                }
                Compression::Zstd => {
                    needs_archive_bytes = true;
                    FileData::CompressedArchive {
                        offset: entry.stored_offset as usize,
                        len: entry.stored_len as usize,
                        uncompressed_len: entry.uncompressed_size as usize,
                    }
                }
            };
            entries.push((entry.path.clone(), data, entry.mode));
        }
        drop(archive);
        let mut fs = Self::from_archive_bytes(
            if needs_archive_bytes {
                bytes
            } else {
                Vec::new()
            },
            content_root,
        );
        let mut dirs = BTreeSet::new();
        dirs.insert(String::new());
        for (path, data, mode) in entries {
            let mut parent = String::new();
            let parts = path.split('/').collect::<Vec<_>>();
            for component in &parts[..parts.len() - 1] {
                let next = if parent.is_empty() {
                    (*component).to_string()
                } else {
                    parent.clone() + "/" + component
                };
                if dirs.insert(next.clone()) {
                    fs.insert_directory(&parent, component)?;
                }
                parent = next;
            }
            fs.insert_file(&parent, parts.last().unwrap(), data, mode)?;
        }
        Ok(fs)
    }

    fn from_archive_bytes(archive_bytes: Vec<u8>, content_root: [u8; 32]) -> Self {
        Self {
            archive_bytes,
            content_root,
            nodes: Vec::from([Node::Directory {
                children: BTreeMap::new(),
            }]),
        }
    }

    pub fn root_inode(&self) -> u64 {
        0
    }

    pub fn open(&self, base: u64, path: &str, flags: u32) -> Result<OpenNode, ArchiveFsError> {
        if flags & (0x2 | 0x8 | 0x10) != 0 {
            return Err(ArchiveFsError::ReadOnly);
        }
        let inode = self.resolve(base, path)?;
        match self.node(inode)? {
            Node::Directory { .. } => {
                if flags & 0x20 == 0 {
                    return Err(ArchiveFsError::IsDirectory);
                }
                Ok(OpenNode { inode, file: None })
            }
            Node::File { .. } => {
                if flags & 0x20 != 0 {
                    return Err(ArchiveFsError::NotDirectory);
                }
                Ok(OpenNode {
                    inode,
                    file: Some(FileHandle { inode, offset: 0 }),
                })
            }
        }
    }

    pub fn attributes(&self, inode: u64) -> Result<NodeAttributes, ArchiveFsError> {
        Ok(match self.node(inode)? {
            Node::Directory { .. } => NodeAttributes {
                size_bytes: 0,
                storage_allocated_bytes: 0,
                creation_time_nanos: 0,
                modification_time_nanos: 0,
                mode: 0o555,
            },
            Node::File { data, mode } => NodeAttributes {
                size_bytes: data.len() as u64,
                storage_allocated_bytes: data.len() as u64,
                creation_time_nanos: 0,
                modification_time_nanos: 0,
                mode: *mode,
            },
        })
    }

    pub fn read(&mut self, handle: &mut FileHandle, count: u64) -> Result<Vec<u8>, ArchiveFsError> {
        let bytes = self.materialize_file(handle.inode)?;
        let start = (handle.offset as usize).min(bytes.len());
        let end = bytes.len().min(start.saturating_add(count as usize));
        handle.offset = end as u64;
        Ok(bytes[start..end].to_vec())
    }

    pub fn seek(
        &self,
        handle: &mut FileHandle,
        offset: i64,
        whence: u8,
    ) -> Result<u64, ArchiveFsError> {
        let size = match self.node(handle.inode)? {
            Node::File { data, .. } => data.len() as i64,
            Node::Directory { .. } => return Err(ArchiveFsError::IsDirectory),
        };
        let base = match whence {
            0 => 0,
            1 => handle.offset as i64,
            2 => size,
            _ => return Err(ArchiveFsError::InvalidArgs),
        };
        let new_offset = base
            .checked_add(offset)
            .ok_or(ArchiveFsError::InvalidArgs)?;
        if new_offset < 0 {
            return Err(ArchiveFsError::InvalidArgs);
        }
        handle.offset = new_offset as u64;
        Ok(handle.offset)
    }

    pub fn backing_bytes(&mut self, handle: &FileHandle) -> Result<Vec<u8>, ArchiveFsError> {
        Ok(self.materialize_file(handle.inode)?.to_vec())
    }

    pub fn same_archive(&self, other: &Self) -> bool {
        self.content_root == other.content_root && self.archive_bytes == other.archive_bytes
    }

    pub fn matches_verified_archive(&self, bytes: &[u8], expected_root: Option<[u8; 32]>) -> bool {
        self.archive_bytes == bytes
            && expected_root.is_none_or(|expected| self.content_root == expected)
    }

    pub fn content_root(&self) -> [u8; 32] {
        self.content_root
    }

    pub fn read_entries(&self, inode: u64) -> Result<Vec<DirectoryEntry>, ArchiveFsError> {
        let Node::Directory { children } = self.node(inode)? else {
            return Err(ArchiveFsError::NotDirectory);
        };
        children
            .iter()
            .map(|(name, child)| {
                let attributes = self.attributes(*child)?;
                let kind = match self.node(*child)? {
                    Node::Directory { .. } => NodeKind::Directory,
                    Node::File { .. } => NodeKind::File,
                };
                Ok(DirectoryEntry {
                    name: name.clone(),
                    kind,
                    attributes,
                })
            })
            .collect()
    }

    fn insert_directory(&mut self, parent_path: &str, name: &str) -> Result<u64, ArchiveFsError> {
        let parent = self.resolve(0, parent_path)?;
        let inode = self.nodes.len() as u64;
        self.nodes.push(Node::Directory {
            children: BTreeMap::new(),
        });
        self.insert_child(parent, name, inode)?;
        Ok(inode)
    }

    fn insert_file(
        &mut self,
        parent_path: &str,
        name: &str,
        data: FileData,
        mode: u32,
    ) -> Result<u64, ArchiveFsError> {
        let parent = self.resolve(0, parent_path)?;
        let inode = self.nodes.len() as u64;
        self.nodes.push(Node::File { data, mode });
        self.insert_child(parent, name, inode)?;
        Ok(inode)
    }

    fn insert_child(&mut self, parent: u64, name: &str, child: u64) -> Result<(), ArchiveFsError> {
        let Some(Node::Directory { children }) = self.nodes.get_mut(parent as usize) else {
            return Err(ArchiveFsError::NotDirectory);
        };
        if children.insert(name.to_string(), child).is_some() {
            return Err(ArchiveFsError::Corrupt);
        }
        Ok(())
    }

    fn resolve(&self, base: u64, path: &str) -> Result<u64, ArchiveFsError> {
        if path.is_empty() {
            return Ok(base);
        }
        if path.starts_with('/') || path.ends_with('/') {
            return Err(ArchiveFsError::InvalidArgs);
        }
        let mut inode = base;
        for component in path.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(ArchiveFsError::InvalidArgs);
            }
            let Node::Directory { children } = self.node(inode)? else {
                return Err(ArchiveFsError::NotDirectory);
            };
            inode = *children.get(component).ok_or(ArchiveFsError::NotFound)?;
        }
        Ok(inode)
    }

    fn node(&self, inode: u64) -> Result<&Node, ArchiveFsError> {
        self.nodes
            .get(inode as usize)
            .ok_or(ArchiveFsError::NotFound)
    }

    fn materialize_file(&mut self, inode: u64) -> Result<&[u8], ArchiveFsError> {
        let materialized = match self.node(inode)? {
            Node::Directory { .. } => return Err(ArchiveFsError::IsDirectory),
            Node::File {
                data: FileData::Owned(_),
                ..
            } => None,
            Node::File {
                data: FileData::Archive { offset, len },
                ..
            } => Some(
                self.archive_bytes
                    .get(*offset..offset.saturating_add(*len))
                    .ok_or(ArchiveFsError::Corrupt)?
                    .to_vec(),
            ),
            Node::File {
                data:
                    FileData::CompressedArchive {
                        offset,
                        len,
                        uncompressed_len,
                    },
                ..
            } => Some({
                let stored = self
                    .archive_bytes
                    .get(*offset..offset.saturating_add(*len))
                    .ok_or(ArchiveFsError::Corrupt)?;
                decompress_zstd(stored, *uncompressed_len)?
            }),
        };
        if let Some(bytes) = materialized {
            let Node::File { data, .. } = self
                .nodes
                .get_mut(inode as usize)
                .ok_or(ArchiveFsError::NotFound)?
            else {
                return Err(ArchiveFsError::IsDirectory);
            };
            *data = FileData::Owned(bytes);
        }
        match self.node(inode)? {
            Node::File {
                data: FileData::Owned(bytes),
                ..
            } => Ok(bytes),
            _ => Err(ArchiveFsError::Corrupt),
        }
    }
}

fn decompress_zstd(bytes: &[u8], expected_len: usize) -> Result<Vec<u8>, ArchiveFsError> {
    use ruzstd::io::Read;
    let mut decoder =
        ruzstd::decoding::StreamingDecoder::new(bytes).map_err(|_| ArchiveFsError::Corrupt)?;
    let mut out = Vec::new();
    out.try_reserve_exact(expected_len)
        .map_err(|_| ArchiveFsError::AccessDenied)?;
    #[cfg(feature = "guest")]
    if expected_len != 0 {
        bexos_userspace::Memory::commit_range(out.as_mut_ptr() as u64, expected_len as u64)
            .map_err(|_| ArchiveFsError::AccessDenied)?;
    }
    Read::read_to_end(&mut decoder, &mut out).map_err(|_| ArchiveFsError::Corrupt)?;
    if out.len() != expected_len {
        return Err(ArchiveFsError::Corrupt);
    }
    Ok(out)
}
