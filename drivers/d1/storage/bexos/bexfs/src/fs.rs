mod migration;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use aes_gcm_siv::{
    Aes256GcmSiv, Key, Nonce,
    aead::{AeadInOut, KeyInit},
};
use rosefs_core::aead::{MetadataKey, open_dentry_name, seal_dentry_name};
use rosefs_core::block::{BlockDevice, BlockIoError};
use rosefs_core::btree::NodeRegionAllocator;
use rosefs_core::container::Container;
use rosefs_core::fmt::{
    CAP_BOOTABLE, CAP_DEFAULT_ENCRYPTED, DEFAULT_BTREE_LEAF_NODE_SIZE_BYTES, PhysicalDeviceDisk,
};

use crate::block::BEXFS_BLOCK_SIZE;
use crate::format::{BexfsHeader, HeaderError, KEY_CHECK_PLAINTEXT, SlotDescriptor, nonce};
use crate::key::LockedVolumeKey;

const NAMESPACE_MAGIC: [u8; 8] = *b"BEXNS001";
const NAMESPACE_EXTENTS_MAGIC: [u8; 8] = *b"BEXNS002";
const NAMESPACE_SPARSE_MAGIC: [u8; 8] = *b"BEXNS003";
const NAMESPACE_DENTRIES_MAGIC: [u8; 8] = *b"BEXNS004";
const EXTERNAL_DATA_THRESHOLD: usize = 128 * 1024;
const EXTERNAL_READ_WINDOW_BYTES: usize = 2 * 1024 * 1024;
const EXTERNAL_WRITE_WINDOW_BYTES: usize = 2 * 1024 * 1024;
const SPARSE_EXTENT_SIZE: usize = 4096;
const ROOT_INODE: u64 = 1;

fn zeroed_io_buffer(len: usize) -> Result<Vec<u8>, BexFsError> {
    #[cfg(all(feature = "guest", target_os = "linux"))]
    {
        let mut bytes = Vec::with_capacity(len);
        if len != 0 {
            bexos_userspace::Memory::commit_range(bytes.as_mut_ptr() as u64, len as u64)
                .map_err(|_| BexFsError::NoSpace)?;
            unsafe {
                core::ptr::write_bytes(bytes.as_mut_ptr(), 0, len);
                bytes.set_len(len);
            }
        }
        Ok(bytes)
    }
    #[cfg(not(all(feature = "guest", target_os = "linux")))]
    {
        Ok(vec![0; len])
    }
}

fn external_window<'a>(
    device: &dyn BlockDevice,
    data_lba: u64,
    external_read_len: usize,
    window_start: usize,
    loaded: &'a mut Option<(usize, Rc<Vec<u8>>)>,
) -> Result<&'a Rc<Vec<u8>>, BexFsError> {
    if loaded.as_ref().map(|(start, _)| *start) != Some(window_start) {
        let read_len = (external_read_len - window_start).min(EXTERNAL_READ_WINDOW_BYTES);
        let mut bytes = zeroed_io_buffer(read_len)?;
        device.read_at(
            data_lba + (window_start / BEXFS_BLOCK_SIZE as usize) as u64,
            &mut bytes,
        )?;
        *loaded = Some((window_start, Rc::new(bytes)));
    }
    Ok(&loaded.as_ref().unwrap().1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BexFsError {
    Io,
    Corrupt,
    Locked,
    NoSpace,
    NotFound,
    NotDirectory,
    IsDirectory,
    NotEmpty,
    AlreadyExists,
    AccessDenied,
    InvalidArgs,
    ReadOnly,
}

impl From<BlockIoError> for BexFsError {
    fn from(_: BlockIoError) -> Self {
        Self::Io
    }
}

impl From<HeaderError> for BexFsError {
    fn from(_: HeaderError) -> Self {
        Self::Corrupt
    }
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
pub struct FileHandle {
    inode: u64,
    cursor: u64,
    readable: bool,
    writable: bool,
}

impl FileHandle {
    pub const fn inode(&self) -> u64 {
        self.inode
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Node {
    inode: u64,
    kind: NodeKind,
    attributes: NodeAttributes,
    data: SparseData,
    xattrs: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SparseData {
    logical_size: u64,
    extents: BTreeMap<u64, ExtentData>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ExtentData {
    Owned(Vec<u8>),
    Shared {
        bytes: Rc<Vec<u8>>,
        offset: usize,
        len: usize,
    },
}

impl ExtentData {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Shared { bytes, offset, len } => &bytes[*offset..*offset + *len],
        }
    }

    fn make_owned(&mut self) -> &mut Vec<u8> {
        if let Self::Shared { .. } = self {
            *self = Self::Owned(self.as_slice().to_vec());
        }
        let Self::Owned(bytes) = self else {
            unreachable!()
        };
        bytes
    }
}

impl SparseData {
    fn from_dense(bytes: Vec<u8>) -> Self {
        let logical_size = bytes.len() as u64;
        let mut extents = BTreeMap::new();
        for (chunk, data) in bytes.chunks(SPARSE_EXTENT_SIZE).enumerate() {
            if !data.iter().all(|byte| *byte == 0) {
                let mut extent = vec![0; SPARSE_EXTENT_SIZE];
                extent[..data.len()].copy_from_slice(data);
                extents.insert(chunk as u64, ExtentData::Owned(extent));
            }
        }
        Self {
            logical_size,
            extents,
        }
    }

    fn len(&self) -> u64 {
        self.logical_size
    }

    fn allocated_bytes(&self) -> u64 {
        self.extents.len() as u64 * SPARSE_EXTENT_SIZE as u64
    }

    fn read_at(&self, offset: u64, count: usize) -> Vec<u8> {
        if offset >= self.logical_size || count == 0 {
            return Vec::new();
        }
        let available = self.logical_size - offset;
        let count = count.min(available.min(usize::MAX as u64) as usize);
        let mut out = vec![0; count];
        let mut written = 0usize;
        while written < out.len() {
            let absolute = offset + written as u64;
            let chunk = absolute / SPARSE_EXTENT_SIZE as u64;
            let chunk_offset = (absolute % SPARSE_EXTENT_SIZE as u64) as usize;
            let copy = (SPARSE_EXTENT_SIZE - chunk_offset).min(out.len() - written);
            if let Some(extent) = self.extents.get(&chunk) {
                out[written..written + copy]
                    .copy_from_slice(&extent.as_slice()[chunk_offset..chunk_offset + copy]);
            }
            written += copy;
        }
        out
    }

    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), BexFsError> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(BexFsError::NoSpace)?;
        let mut copied = 0usize;
        while copied < bytes.len() {
            let absolute = offset + copied as u64;
            let chunk = absolute / SPARSE_EXTENT_SIZE as u64;
            let chunk_offset = (absolute % SPARSE_EXTENT_SIZE as u64) as usize;
            let copy = (SPARSE_EXTENT_SIZE - chunk_offset).min(bytes.len() - copied);
            let extent = self
                .extents
                .entry(chunk)
                .or_insert_with(|| ExtentData::Owned(vec![0; SPARSE_EXTENT_SIZE]))
                .make_owned();
            extent[chunk_offset..chunk_offset + copy]
                .copy_from_slice(&bytes[copied..copied + copy]);
            copied += copy;
        }
        self.logical_size = self.logical_size.max(end);
        Ok(())
    }

    fn truncate(&mut self, len: u64) {
        self.logical_size = len;
        let keep_chunks = len.div_ceil(SPARSE_EXTENT_SIZE as u64);
        self.extents.retain(|chunk, _| *chunk < keep_chunks);
        if !len.is_multiple_of(SPARSE_EXTENT_SIZE as u64) && keep_chunks != 0 {
            if let Some(tail) = self.extents.get_mut(&(keep_chunks - 1)) {
                let valid = (len % SPARSE_EXTENT_SIZE as u64) as usize;
                tail.make_owned()[valid..].fill(0);
            }
        }
    }

    fn dense_chunk(&self, chunk: u64, max_len: usize) -> Option<Vec<u8>> {
        let start = chunk.checked_mul(max_len as u64)?;
        if start >= self.logical_size {
            return None;
        }
        let count = max_len.min((self.logical_size - start).min(max_len as u64) as usize);
        Some(self.read_at(start, count))
    }

    fn iter_allocated(&self) -> impl Iterator<Item = (u64, &[u8])> {
        self.extents
            .iter()
            .map(|(chunk, bytes)| (*chunk, bytes.as_slice()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Namespace {
    next_inode: u64,
    nodes: BTreeMap<u64, Node>,
    dentries: BTreeMap<(u64, String), u64>,
}

impl Namespace {
    fn empty() -> Self {
        let root = Node {
            inode: ROOT_INODE,
            kind: NodeKind::Directory,
            attributes: NodeAttributes {
                size_bytes: 0,
                storage_allocated_bytes: 0,
                creation_time_nanos: 0,
                modification_time_nanos: 0,
                mode: 0o755,
                uid: 0,
                gid: 0,
            },
            data: SparseData::default(),
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
}

pub struct FormatOptions<'a> {
    pub label: &'a str,
    pub volume_uuid: [u8; 16],
    pub device_uuid: [u8; 16],
}

pub struct BexFs {
    header_lba: u64,
    header: BexfsHeader,
    key: LockedVolumeKey,
    namespace: Namespace,
    read_only: bool,
    dirty: BTreeSet<u64>,
    migration_format: u64,
}

impl BexFs {
    pub fn format(
        device: &mut dyn BlockDevice,
        key: LockedVolumeKey,
        options: FormatOptions<'_>,
    ) -> Result<Self, BexFsError> {
        validate_device(device)?;
        let physical = PhysicalDeviceDisk {
            device_id: 0,
            stripe_slot_index: 0,
            device_uuid: options.device_uuid,
            total_sectors: device.num_blocks().saturating_mul(8),
            health_flags: 0,
        };
        let mut container = Container::format(
            device,
            vec![physical],
            0,
            true,
            DEFAULT_BTREE_LEAF_NODE_SIZE_BYTES,
        )
        .map_err(|_| BexFsError::Corrupt)?;
        container
            .keyslots_mut()
            .add_tpm(0)
            .map_err(|_| BexFsError::Corrupt)?;
        let available = container.allocator().free_blocks();
        if available < 16 {
            return Err(BexFsError::NoSpace);
        }
        let volume_index = container
            .create_volume(
                options.volume_uuid,
                label_bytes(options.label)?,
                CAP_BOOTABLE | CAP_DEFAULT_ENCRYPTED,
                0,
                available,
            )
            .map_err(|_| BexFsError::NoSpace)?;
        let header_lba = container
            .allocator_mut()
            .alloc_region(2)
            .map_err(|_| BexFsError::NoSpace)?;
        let remaining = container.allocator().free_blocks();
        let slot_blocks = remaining / 2;
        if slot_blocks < 2 {
            return Err(BexFsError::NoSpace);
        }
        let slot_a_lba = container
            .allocator_mut()
            .alloc_region(slot_blocks)
            .map_err(|_| BexFsError::NoSpace)?;
        let slot_b_lba = container
            .allocator_mut()
            .alloc_region(slot_blocks)
            .map_err(|_| BexFsError::NoSpace)?;
        container
            .charge_pages(volume_index, 2 + slot_blocks * 2)
            .map_err(|_| BexFsError::NoSpace)?;
        container
            .volume_mut(volume_index)
            .ok_or(BexFsError::Corrupt)?
            .descriptor
            .fs_superblock_offset = header_lba;
        container.sync(device).map_err(|_| BexFsError::Io)?;

        let metadata_key = MetadataKey::new(key.expose()).map_err(|_| BexFsError::Locked)?;
        let key_check = seal_dentry_name(&metadata_key, &KEY_CHECK_PLAINTEXT, &[0u8; 12])
            .map_err(|_| BexFsError::Locked)?;
        let mut header = BexfsHeader {
            generation: 1,
            active_slot: 0,
            slots: [
                SlotDescriptor {
                    lba: slot_a_lba,
                    blocks: slot_blocks,
                    bytes: 0,
                    nonce: nonce(1, 0),
                },
                SlotDescriptor {
                    lba: slot_b_lba,
                    blocks: slot_blocks,
                    bytes: 0,
                    nonce: nonce(0, 1),
                },
            ],
            key_check: key_check.try_into().map_err(|_| BexFsError::Corrupt)?,
            volume_uuid: options.volume_uuid,
        };
        let mut namespace = Namespace::empty();
        write_namespace_slot(device, key.expose(), &mut namespace, &mut header.slots[0])?;
        write_header_pair(device, header_lba, &header)?;
        Ok(Self {
            header_lba,
            header,
            key,
            namespace,
            dirty: BTreeSet::new(),
            read_only: false,
            migration_format: 4,
        })
    }

    pub fn mount(
        device: &mut dyn BlockDevice,
        key: LockedVolumeKey,
        label: &str,
        read_only: bool,
    ) -> Result<Self, BexFsError> {
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount validate device\n");
        validate_device(device)?;
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount load container\n");
        let container = Container::load(device).map_err(|_| BexFsError::Corrupt)?;
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount scan volumes\n");
        let mut found = None;
        for volume in container.volumes() {
            let stored_label = volume.descriptor.label;
            let end = stored_label
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(stored_label.len());
            if &stored_label[..end] == label.as_bytes() {
                found = Some((
                    volume.descriptor.fs_superblock_offset,
                    volume.descriptor.volume_uuid,
                ));
                break;
            }
        }
        let (header_lba, volume_uuid) = found.ok_or(BexFsError::NotFound)?;
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount read header\n");
        let header = read_header_pair(device, header_lba)?;
        if header.volume_uuid != volume_uuid {
            return Err(BexFsError::Corrupt);
        }
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount metadata key\n");
        let metadata_key = MetadataKey::new(key.expose()).map_err(|_| BexFsError::Locked)?;
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount key check\n");
        let key_check = open_dentry_name(&metadata_key, &header.key_check, &[0u8; 12])
            .map_err(|_| BexFsError::Locked)?;
        if key_check.as_slice() != KEY_CHECK_PLAINTEXT {
            return Err(BexFsError::Locked);
        }
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount read namespace\n");
        let namespace = read_namespace_slot(
            device,
            key.expose(),
            &header.slots[header.active_slot as usize],
        )?;
        #[cfg(feature = "std")]
        bexos_userspace::log("bexfs: fs mount ok\n");
        Ok(Self {
            header_lba,
            header,
            key,
            namespace,
            dirty: BTreeSet::new(),
            read_only,
            migration_format: 4,
        })
    }

    pub fn generation(&self) -> u64 {
        self.header.generation
    }

    pub fn usage(&self) -> NodeAttributes {
        let mut usage = NodeAttributes {
            size_bytes: 0,
            storage_allocated_bytes: 0,
            creation_time_nanos: 0,
            modification_time_nanos: 0,
            mode: 0,
            uid: 0,
            gid: 0,
        };
        for node in self.namespace.nodes.values() {
            usage.size_bytes = usage.size_bytes.saturating_add(node.attributes.size_bytes);
            usage.storage_allocated_bytes = usage
                .storage_allocated_bytes
                .saturating_add(node.attributes.storage_allocated_bytes);
        }
        usage
    }

    pub fn copy_to_new_device(
        &mut self,
        current_device: &mut dyn BlockDevice,
        new_device: &mut dyn BlockDevice,
        label: &str,
        volume_uuid: [u8; 16],
        device_uuid: [u8; 16],
    ) -> Result<Self, BexFsError> {
        self.sync(current_device)?;
        let mut next = Self::format(
            new_device,
            LockedVolumeKey::new(self.key.expose()).map_err(|_| BexFsError::Locked)?,
            FormatOptions {
                label,
                volume_uuid,
                device_uuid,
            },
        )?;
        next.namespace = self.namespace.clone();
        next.dirty.insert(0);
        for inode in next.namespace.nodes.keys().copied() {
            next.dirty.insert(inode << 16);
        }
        next.sync(new_device)?;
        Ok(next)
    }

    pub fn root_inode(&self) -> u64 {
        ROOT_INODE
    }

    pub fn open(&mut self, base: u64, path: &str, flags: u32) -> Result<FileHandle, BexFsError> {
        let readable = flags & 0x1 != 0;
        let writable = flags & 0x2 != 0;
        if writable && self.read_only {
            return Err(BexFsError::ReadOnly);
        }
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
            Err(BexFsError::NotFound) if create => {
                self.create_path(base, path, require_directory)?
            }
            Err(error) => return Err(error),
        };
        let node = self
            .namespace
            .nodes
            .get_mut(&inode)
            .ok_or(BexFsError::Corrupt)?;
        if require_directory && node.kind != NodeKind::Directory {
            return Err(BexFsError::NotDirectory);
        }
        if !require_directory && node.kind == NodeKind::Directory {
            return Err(BexFsError::IsDirectory);
        }
        if truncate {
            if !writable {
                return Err(BexFsError::AccessDenied);
            }
            self.dirty.insert(inode << 16);
            self.dirty.extend(
                (0..node.data.len().div_ceil(migration::CHUNK as u64))
                    .map(|i| (inode << 16) | i as u64 + 1),
            );
            node.data.truncate(0);
            update_size(node);
        }
        Ok(FileHandle {
            inode,
            cursor: 0,
            readable,
            writable,
        })
    }

    pub fn read(&self, handle: &mut FileHandle, count: u64) -> Result<Vec<u8>, BexFsError> {
        if !handle.readable {
            return Err(BexFsError::AccessDenied);
        }
        let node = self
            .namespace
            .nodes
            .get(&handle.inode)
            .ok_or(BexFsError::NotFound)?;
        if node.kind == NodeKind::Directory {
            return Err(BexFsError::IsDirectory);
        }
        let requested = usize::try_from(count).map_err(|_| BexFsError::InvalidArgs)?;
        let bytes = node.data.read_at(handle.cursor, requested);
        handle.cursor = handle.cursor.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }

    pub fn write(&mut self, handle: &mut FileHandle, bytes: &[u8]) -> Result<u64, BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        if !handle.writable {
            return Err(BexFsError::AccessDenied);
        }
        let node = self
            .namespace
            .nodes
            .get_mut(&handle.inode)
            .ok_or(BexFsError::NotFound)?;
        if node.kind == NodeKind::Directory {
            return Err(BexFsError::IsDirectory);
        }
        let start = handle.cursor;
        let end = start
            .checked_add(bytes.len() as u64)
            .ok_or(BexFsError::NoSpace)?;
        self.dirty.insert(handle.inode << 16);
        self.dirty.extend(
            (start / migration::CHUNK as u64..end.div_ceil(migration::CHUNK as u64))
                .map(|i| (handle.inode << 16) | i as u64 + 1),
        );
        node.data.write_at(start, bytes)?;
        handle.cursor = end;
        update_size(node);
        Ok(bytes.len() as u64)
    }

    pub fn set_len(&mut self, handle: &FileHandle, len: u64) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        if !handle.writable {
            return Err(BexFsError::AccessDenied);
        }
        let node = self
            .namespace
            .nodes
            .get_mut(&handle.inode)
            .ok_or(BexFsError::NotFound)?;
        if node.kind == NodeKind::Directory {
            return Err(BexFsError::IsDirectory);
        }
        let old_len = node.data.len();
        node.data.truncate(len);
        self.dirty.insert(handle.inode << 16);
        let first = old_len.min(len) / migration::CHUNK as u64;
        let last = old_len.max(len).div_ceil(migration::CHUNK as u64);
        self.dirty
            .extend((first..last).map(|i| (handle.inode << 16) | i as u64 + 1));
        update_size(node);
        Ok(())
    }

    pub fn seek(
        &self,
        handle: &mut FileHandle,
        offset: i64,
        whence: u8,
    ) -> Result<u64, BexFsError> {
        let node = self
            .namespace
            .nodes
            .get(&handle.inode)
            .ok_or(BexFsError::NotFound)?;
        let base = match whence {
            0 => 0i128,
            1 => i128::from(handle.cursor),
            2 => node.data.len() as i128,
            _ => return Err(BexFsError::InvalidArgs),
        };
        let next = base + i128::from(offset);
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(BexFsError::InvalidArgs);
        }
        handle.cursor = next as u64;
        Ok(handle.cursor)
    }

    pub fn attributes(&self, inode: u64) -> Result<NodeAttributes, BexFsError> {
        self.namespace
            .nodes
            .get(&inode)
            .map(|node| node.attributes)
            .ok_or(BexFsError::NotFound)
    }

    pub fn set_metadata(
        &mut self,
        inode: u64,
        mode: u32,
        uid: u32,
        gid: u32,
        modification_time_nanos: u64,
    ) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        let node = self
            .namespace
            .nodes
            .get_mut(&inode)
            .ok_or(BexFsError::NotFound)?;
        node.attributes.mode = mode & 0o7777;
        node.attributes.uid = uid;
        node.attributes.gid = gid;
        node.attributes.modification_time_nanos = modification_time_nanos;
        self.dirty.insert(inode << 16);
        Ok(())
    }

    pub fn kind_no_follow(&self, base: u64, path: &str) -> Result<NodeKind, BexFsError> {
        let inode = self.inode_no_follow(base, path)?;
        self.namespace
            .nodes
            .get(&inode)
            .map(|node| node.kind)
            .ok_or(BexFsError::NotFound)
    }

    pub fn inode_no_follow(&self, base: u64, path: &str) -> Result<u64, BexFsError> {
        self.resolve_no_follow(base, path)
    }

    pub fn remove_tree(&mut self, base: u64, path: &str) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        let (parent, name) = self.resolve_parent(base, path)?;
        self.remove_tree_entry(parent, name)
    }

    pub fn read_entries(&self, inode: u64) -> Result<Vec<DirectoryEntry>, BexFsError> {
        let directory = self
            .namespace
            .nodes
            .get(&inode)
            .ok_or(BexFsError::NotFound)?;
        if directory.kind != NodeKind::Directory {
            return Err(BexFsError::NotDirectory);
        }
        Ok(self
            .namespace
            .dentries
            .iter()
            .filter(|((parent, _), _)| *parent == inode)
            .filter_map(|((_, name), child)| {
                self.namespace.nodes.get(child).map(|node| (name, node))
            })
            .map(|(name, node)| DirectoryEntry {
                name: name.clone(),
                kind: node.kind,
                attributes: node.attributes,
            })
            .collect())
    }

    pub fn unlink(&mut self, directory: u64, name: &str) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        validate_component(name)?;
        self.require_directory(directory)?;
        let key = (directory, name.to_string());
        let inode = *self
            .namespace
            .dentries
            .get(&key)
            .ok_or(BexFsError::NotFound)?;
        if self.has_children(inode) {
            return Err(BexFsError::NotEmpty);
        }
        self.namespace.dentries.remove(&key);
        self.remove_unlinked_inode(inode);
        self.dirty.insert(0);
        Ok(())
    }

    pub fn link(&mut self, base: u64, source: &str, target: &str) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        let source_inode = self.resolve_no_follow(base, source)?;
        if self
            .namespace
            .nodes
            .get(&source_inode)
            .is_none_or(|node| node.kind == NodeKind::Directory)
        {
            return Err(BexFsError::AccessDenied);
        }
        let (target_parent, target_name) = self.resolve_parent(base, target)?;
        let key = (target_parent, target_name.to_string());
        if self.namespace.dentries.contains_key(&key) {
            return Err(BexFsError::AlreadyExists);
        }
        self.namespace.dentries.insert(key, source_inode);
        self.dirty.insert(0);
        Ok(())
    }

    pub fn symlink(&mut self, base: u64, target: &str, link_path: &str) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        validate_symlink_target(target)?;
        let (parent, name) = self.resolve_parent(base, link_path)?;
        let key = (parent, name.to_string());
        if self.namespace.dentries.contains_key(&key) {
            return Err(BexFsError::AlreadyExists);
        }
        let inode = self.namespace.next_inode;
        self.namespace.next_inode = inode.checked_add(1).ok_or(BexFsError::NoSpace)?;
        let data = SparseData::from_dense(target.as_bytes().to_vec());
        self.namespace.nodes.insert(
            inode,
            Node {
                inode,
                kind: NodeKind::Symlink,
                attributes: NodeAttributes {
                    size_bytes: data.len(),
                    storage_allocated_bytes: data.allocated_bytes(),
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
        self.namespace.dentries.insert(key, inode);
        self.dirty.insert(0);
        self.dirty.insert(inode << 16);
        Ok(())
    }

    pub fn readlink(&self, base: u64, path: &str) -> Result<String, BexFsError> {
        let inode = self.resolve_no_follow(base, path)?;
        let node = self
            .namespace
            .nodes
            .get(&inode)
            .ok_or(BexFsError::NotFound)?;
        if node.kind != NodeKind::Symlink {
            return Err(BexFsError::InvalidArgs);
        }
        let bytes = node.data.read_at(0, node.data.len() as usize);
        core::str::from_utf8(&bytes)
            .map(ToString::to_string)
            .map_err(|_| BexFsError::Corrupt)
    }

    pub fn get_xattr(&self, inode: u64, name: &str) -> Result<Vec<u8>, BexFsError> {
        validate_xattr_name(name)?;
        self.namespace
            .nodes
            .get(&inode)
            .ok_or(BexFsError::NotFound)?
            .xattrs
            .get(name)
            .cloned()
            .ok_or(BexFsError::NotFound)
    }

    pub fn set_xattr(
        &mut self,
        inode: u64,
        name: &str,
        value: &[u8],
        flags: u32,
    ) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        validate_xattr_name(name)?;
        if value.len() > 65_536 || flags & !3 != 0 || flags == 3 {
            return Err(BexFsError::InvalidArgs);
        }
        let node = self
            .namespace
            .nodes
            .get_mut(&inode)
            .ok_or(BexFsError::NotFound)?;
        let exists = node.xattrs.contains_key(name);
        if flags == 1 && exists || flags == 2 && !exists {
            return Err(if exists {
                BexFsError::AlreadyExists
            } else {
                BexFsError::NotFound
            });
        }
        node.xattrs.insert(name.to_string(), value.to_vec());
        self.dirty.insert(inode << 16);
        Ok(())
    }

    pub fn list_xattrs(&self, inode: u64) -> Result<Vec<String>, BexFsError> {
        Ok(self
            .namespace
            .nodes
            .get(&inode)
            .ok_or(BexFsError::NotFound)?
            .xattrs
            .keys()
            .cloned()
            .collect())
    }

    pub fn remove_xattr(&mut self, inode: u64, name: &str) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        validate_xattr_name(name)?;
        self.namespace
            .nodes
            .get_mut(&inode)
            .ok_or(BexFsError::NotFound)?
            .xattrs
            .remove(name)
            .map(|_| self.dirty.insert(inode << 16))
            .ok_or(BexFsError::NotFound)?;
        Ok(())
    }

    pub fn rename(&mut self, base: u64, source: &str, target: &str) -> Result<(), BexFsError> {
        if self.read_only {
            return Err(BexFsError::ReadOnly);
        }
        let source_inode = self.resolve(base, source)?;
        if source_inode == ROOT_INODE {
            return Err(BexFsError::InvalidArgs);
        }
        let (source_parent, source_name) = self.resolve_parent(base, source)?;
        let source_key = (source_parent, source_name.to_string());
        let (target_parent, target_name) = self.resolve_parent(base, target)?;
        let target_key = (target_parent, target_name.to_string());
        let source_kind = self
            .namespace
            .nodes
            .get(&source_inode)
            .ok_or(BexFsError::NotFound)?
            .kind;
        if source_kind == NodeKind::Directory {
            let mut ancestor = target_parent;
            loop {
                if ancestor == source_inode {
                    return Err(BexFsError::InvalidArgs);
                }
                if ancestor == ROOT_INODE {
                    break;
                }
                ancestor = self.directory_parent(ancestor)?;
            }
        }
        let replaced = self.namespace.dentries.get(&target_key).and_then(|inode| {
            self.namespace
                .nodes
                .get(inode)
                .map(|node| (*inode, node.kind))
        });
        if let Some((inode, kind)) = replaced {
            if inode == source_inode && source_key == target_key {
                return Ok(());
            }
            if source_kind == NodeKind::Directory && kind != NodeKind::Directory {
                return Err(BexFsError::NotDirectory);
            }
            if source_kind != NodeKind::Directory && kind == NodeKind::Directory {
                return Err(BexFsError::IsDirectory);
            }
            if self.has_children(inode) {
                return Err(BexFsError::NotEmpty);
            }
            self.namespace.dentries.remove(&target_key);
            self.remove_unlinked_inode(inode);
        }
        self.namespace
            .dentries
            .remove(&source_key)
            .ok_or(BexFsError::NotFound)?;
        self.namespace.dentries.insert(target_key, source_inode);
        self.dirty.insert(0);
        Ok(())
    }

    pub fn sync(&mut self, device: &mut dyn BlockDevice) -> Result<(), BexFsError> {
        if self.read_only {
            return Ok(());
        }
        self.dirty.insert(0);
        MetadataKey::new(self.key.expose()).map_err(|_| BexFsError::Locked)?;
        let inactive = 1usize - self.header.active_slot as usize;
        let generation = self.header.generation.saturating_add(1);
        self.header.slots[inactive].nonce = nonce(generation, inactive as u8);
        write_namespace_slot(
            device,
            self.key.expose(),
            &mut self.namespace,
            &mut self.header.slots[inactive],
        )?;
        device.flush()?;
        self.header.generation = generation;
        self.header.active_slot = inactive as u8;
        write_header_pair(device, self.header_lba, &self.header)
    }

    pub fn close(&mut self, device: &mut dyn BlockDevice) -> Result<(), BexFsError> {
        self.sync(device)
    }

    fn resolve(&self, base: u64, path: &str) -> Result<u64, BexFsError> {
        if path == "." {
            return self
                .namespace
                .nodes
                .contains_key(&base)
                .then_some(base)
                .ok_or(BexFsError::NotFound);
        }
        self.resolve_path(base, path, true)
    }

    fn resolve_no_follow(&self, base: u64, path: &str) -> Result<u64, BexFsError> {
        self.resolve_path(base, path, false)
    }

    fn resolve_path(&self, base: u64, path: &str, follow_final: bool) -> Result<u64, BexFsError> {
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
    ) -> Result<u64, BexFsError> {
        if depth > 40 || !self.namespace.nodes.contains_key(&current) {
            return Err(if depth > 40 {
                BexFsError::InvalidArgs
            } else {
                BexFsError::NotFound
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
                .namespace
                .dentries
                .get(&(parent, component.clone()))
                .copied()
                .ok_or(BexFsError::NotFound)?;
            let final_component = index + 1 == count;
            let node = self
                .namespace
                .nodes
                .get(&current)
                .ok_or(BexFsError::NotFound)?;
            if node.kind == NodeKind::Symlink && (!final_component || follow_final) {
                let bytes = node.data.read_at(0, node.data.len() as usize);
                let target = core::str::from_utf8(&bytes).map_err(|_| BexFsError::Corrupt)?;
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

    fn create_path(&mut self, base: u64, path: &str, directory: bool) -> Result<u64, BexFsError> {
        let (parent, name) = self.resolve_parent(base, path)?;
        let key = (parent, name.to_string());
        if self.namespace.dentries.contains_key(&key) {
            return Err(BexFsError::AlreadyExists);
        }
        let inode = self.namespace.next_inode;
        self.namespace.next_inode = inode.checked_add(1).ok_or(BexFsError::NoSpace)?;
        self.dirty.insert(0);
        self.dirty.insert(inode << 16);
        self.namespace.nodes.insert(
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
                    mode: if directory { 0o755 } else { 0o600 },
                    uid: 0,
                    gid: 0,
                },
                data: SparseData::default(),
                xattrs: BTreeMap::new(),
            },
        );
        self.namespace.dentries.insert(key, inode);
        Ok(inode)
    }

    fn resolve_parent<'a>(&self, base: u64, path: &'a str) -> Result<(u64, &'a str), BexFsError> {
        let components = validate_relative_path(path)?;
        let (name, parents) = components.split_last().ok_or(BexFsError::InvalidArgs)?;
        let mut parent = base;
        for component in parents {
            parent = self.resolve(parent, component)?;
        }
        self.require_directory(parent)?;
        Ok((parent, name))
    }

    fn require_directory(&self, inode: u64) -> Result<(), BexFsError> {
        match self.namespace.nodes.get(&inode) {
            Some(node) if node.kind == NodeKind::Directory => Ok(()),
            Some(_) => Err(BexFsError::NotDirectory),
            None => Err(BexFsError::NotFound),
        }
    }

    fn has_children(&self, inode: u64) -> bool {
        self.namespace
            .dentries
            .keys()
            .any(|(parent, _)| *parent == inode)
    }

    fn remove_unlinked_inode(&mut self, inode: u64) {
        if inode == ROOT_INODE
            || self
                .namespace
                .dentries
                .values()
                .any(|child| *child == inode)
        {
            return;
        }
        self.dirty.insert(inode << 16);
        if let Some(node) = self.namespace.nodes.remove(&inode) {
            self.dirty.extend(
                (0..node.data.len().div_ceil(migration::CHUNK as u64))
                    .map(|chunk| (inode << 16) | chunk + 1),
            );
        }
    }

    fn remove_tree_entry(&mut self, parent: u64, name: &str) -> Result<(), BexFsError> {
        let key = (parent, name.to_string());
        let inode = *self
            .namespace
            .dentries
            .get(&key)
            .ok_or(BexFsError::NotFound)?;
        let children = self
            .namespace
            .dentries
            .keys()
            .filter(|(child_parent, _)| *child_parent == inode)
            .map(|(_, child_name)| child_name.clone())
            .collect::<Vec<_>>();
        for child in children {
            self.remove_tree_entry(inode, &child)?;
        }
        self.namespace.dentries.remove(&key);
        self.remove_unlinked_inode(inode);
        self.dirty.insert(0);
        Ok(())
    }

    fn directory_parent(&self, inode: u64) -> Result<u64, BexFsError> {
        if inode == ROOT_INODE {
            return Ok(ROOT_INODE);
        }
        self.namespace
            .dentries
            .iter()
            .find(|(_, child)| **child == inode)
            .map(|((parent, _), _)| *parent)
            .ok_or(BexFsError::Corrupt)
    }
}

fn validate_device(device: &dyn BlockDevice) -> Result<(), BexFsError> {
    if device.block_size() != BEXFS_BLOCK_SIZE || device.num_blocks() < 2048 {
        return Err(BexFsError::InvalidArgs);
    }
    Ok(())
}

fn label_bytes(label: &str) -> Result<[u8; 256], BexFsError> {
    if label.is_empty() || label.len() > 255 {
        return Err(BexFsError::InvalidArgs);
    }
    let mut bytes = [0u8; 256];
    bytes[..label.len()].copy_from_slice(label.as_bytes());
    Ok(bytes)
}

fn validate_component(component: &str) -> Result<(), BexFsError> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.len() > 255
        || component.contains('/')
    {
        return Err(BexFsError::InvalidArgs);
    }
    Ok(())
}

fn validate_symlink_target(target: &str) -> Result<(), BexFsError> {
    if target.is_empty() || target.len() > 4095 || target.as_bytes().contains(&0) {
        return Err(BexFsError::InvalidArgs);
    }
    Ok(())
}

fn validate_xattr_name(name: &str) -> Result<(), BexFsError> {
    if name.is_empty() || name.len() > 255 || name.as_bytes().contains(&0) || !name.contains('.') {
        return Err(BexFsError::InvalidArgs);
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<Vec<&str>, BexFsError> {
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Err(BexFsError::InvalidArgs);
    }
    let components = path.split('/').collect::<Vec<_>>();
    for component in &components {
        validate_component(component)?;
    }
    Ok(components)
}

fn update_size(node: &mut Node) {
    node.attributes.size_bytes = node.data.len();
    node.attributes.storage_allocated_bytes = node.data.allocated_bytes();
}

fn write_namespace_slot(
    device: &dyn BlockDevice,
    key: &[u8; 32],
    namespace: &Namespace,
    slot: &mut SlotDescriptor,
) -> Result<(), BexFsError> {
    let (mut sealed, external, external_len) = serialize_namespace_with_extents(namespace)?;
    seal_namespace_in_place(key, &slot.nonce, &mut sealed).map_err(|_| BexFsError::Corrupt)?;
    let sealed_len = sealed.len() as u64;
    let sealed_blocks = sealed_len.div_ceil(u64::from(BEXFS_BLOCK_SIZE));
    let capacity = slot.blocks.saturating_mul(u64::from(BEXFS_BLOCK_SIZE));
    let total_len = u64::from(BEXFS_BLOCK_SIZE)
        .checked_add(sealed_blocks.saturating_mul(u64::from(BEXFS_BLOCK_SIZE)))
        .and_then(|n| n.checked_add(external_len))
        .ok_or(BexFsError::NoSpace)?;
    if total_len > capacity {
        return Err(BexFsError::NoSpace);
    }
    let mut header = vec![0u8; BEXFS_BLOCK_SIZE as usize];
    header[..8].copy_from_slice(&NAMESPACE_DENTRIES_MAGIC);
    header[8..16].copy_from_slice(&sealed_len.to_le_bytes());
    header[16..24].copy_from_slice(&external_len.to_le_bytes());
    device.write_at(slot.lba, &header)?;

    let sealed_write_len = (sealed_blocks * u64::from(BEXFS_BLOCK_SIZE)) as usize;
    sealed.resize(sealed_write_len, 0);
    device.write_at(slot.lba + 1, &sealed)?;

    if external_len != 0 {
        write_external_extents(
            device,
            slot.lba + 1 + sealed_blocks,
            &external,
            external_len,
        )?;
    }
    slot.bytes = total_len;
    Ok(())
}

struct ExternalWriteRef<'a> {
    offset: u64,
    bytes: &'a [u8],
}

fn write_external_extents(
    device: &dyn BlockDevice,
    data_lba: u64,
    extents: &[ExternalWriteRef<'_>],
    external_len: u64,
) -> Result<(), BexFsError> {
    let block_size = u64::from(BEXFS_BLOCK_SIZE);
    let write_len = external_len.next_multiple_of(block_size);
    let mut window = zeroed_io_buffer(EXTERNAL_WRITE_WINDOW_BYTES)?;
    let mut extent_index = 0usize;
    let mut window_start = 0u64;
    while window_start < write_len {
        let window_len = usize::try_from((write_len - window_start).min(window.len() as u64))
            .map_err(|_| BexFsError::NoSpace)?;
        window[..window_len].fill(0);
        let window_end = window_start + window_len as u64;
        while extent_index < extents.len() {
            let extent = &extents[extent_index];
            let extent_end = extent
                .offset
                .checked_add(extent.bytes.len() as u64)
                .ok_or(BexFsError::NoSpace)?;
            if extent.offset >= window_end {
                break;
            }
            if extent_end > window_start {
                let copy_start = extent.offset.max(window_start);
                let copy_end = extent_end.min(window_end);
                let source_start = (copy_start - extent.offset) as usize;
                let destination_start = (copy_start - window_start) as usize;
                let count = (copy_end - copy_start) as usize;
                window[destination_start..destination_start + count]
                    .copy_from_slice(&extent.bytes[source_start..source_start + count]);
            }
            if extent_end <= window_end {
                extent_index += 1;
            } else {
                break;
            }
        }
        device.write_at(data_lba + window_start / block_size, &window[..window_len])?;
        window_start = window_end;
    }
    Ok(())
}

fn seal_namespace_in_place(key: &[u8; 32], nonce: &[u8], bytes: &mut Vec<u8>) -> Result<(), ()> {
    let nonce = Nonce::try_from(nonce).map_err(|_| ())?;
    let key = Key::<Aes256GcmSiv>::try_from(key.as_slice()).map_err(|_| ())?;
    let cipher = Aes256GcmSiv::new(&key);
    cipher.encrypt_in_place(&nonce, b"", bytes).map_err(|_| ())
}

fn open_namespace_in_place(key: &[u8; 32], nonce: &[u8], bytes: &mut Vec<u8>) -> Result<(), ()> {
    let nonce = Nonce::try_from(nonce).map_err(|_| ())?;
    let key = Key::<Aes256GcmSiv>::try_from(key.as_slice()).map_err(|_| ())?;
    let cipher = Aes256GcmSiv::new(&key);
    cipher.decrypt_in_place(&nonce, b"", bytes).map_err(|_| ())
}

fn read_namespace_slot(
    device: &dyn BlockDevice,
    key: &[u8; 32],
    slot: &SlotDescriptor,
) -> Result<Namespace, BexFsError> {
    if slot.bytes < 16 || slot.bytes > slot.blocks.saturating_mul(u64::from(BEXFS_BLOCK_SIZE)) {
        return Err(BexFsError::Corrupt);
    }
    let mut header = vec![0u8; BEXFS_BLOCK_SIZE as usize];
    device.read_at(slot.lba, &mut header)?;
    if header.get(..8) == Some(&NAMESPACE_EXTENTS_MAGIC)
        || header.get(..8) == Some(&NAMESPACE_SPARSE_MAGIC)
        || header.get(..8) == Some(&NAMESPACE_DENTRIES_MAGIC)
    {
        return read_namespace_slot_with_extents(device, key, slot, &header);
    }
    #[cfg(feature = "std")]
    bexos_userspace::log(&alloc::format!(
        "bexfs: fs namespace slot bytes={} blocks={}\n",
        slot.bytes,
        slot.blocks
    ));
    let read_len =
        (slot.bytes as usize).div_ceil(BEXFS_BLOCK_SIZE as usize) * BEXFS_BLOCK_SIZE as usize;
    let mut bytes = vec![0u8; read_len];
    #[cfg(feature = "std")]
    bexos_userspace::log("bexfs: fs namespace read block\n");
    device.read_at(slot.lba, &mut bytes)?;
    bytes.truncate(slot.bytes as usize);
    #[cfg(feature = "std")]
    bexos_userspace::log("bexfs: fs namespace decrypt\n");
    open_namespace_in_place(key, &slot.nonce, &mut bytes).map_err(|_| BexFsError::Corrupt)?;
    #[cfg(feature = "std")]
    bexos_userspace::log("bexfs: fs namespace deserialize\n");
    let namespace = deserialize_namespace(&bytes)?;
    #[cfg(feature = "std")]
    bexos_userspace::log("bexfs: fs namespace done\n");
    Ok(namespace)
}

fn read_namespace_slot_with_extents(
    device: &dyn BlockDevice,
    key: &[u8; 32],
    slot: &SlotDescriptor,
    header: &[u8],
) -> Result<Namespace, BexFsError> {
    let sealed_len = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let external_len = u64::from_le_bytes(header[16..24].try_into().unwrap());
    if sealed_len < 16 {
        return Err(BexFsError::Corrupt);
    }
    let sealed_blocks = sealed_len.div_ceil(u64::from(BEXFS_BLOCK_SIZE));
    let external_blocks = external_len.div_ceil(u64::from(BEXFS_BLOCK_SIZE));
    let total_blocks = 1u64
        .checked_add(sealed_blocks)
        .and_then(|n| n.checked_add(external_blocks))
        .ok_or(BexFsError::Corrupt)?;
    if total_blocks > slot.blocks
        || u64::from(BEXFS_BLOCK_SIZE)
            .checked_add(sealed_blocks.saturating_mul(u64::from(BEXFS_BLOCK_SIZE)))
            .and_then(|n| n.checked_add(external_len))
            .is_none_or(|n| n > slot.bytes)
    {
        return Err(BexFsError::Corrupt);
    }
    #[cfg(feature = "std")]
    bexos_userspace::log(&alloc::format!(
        "bexfs: fs namespace v2 metadata={} external={}\n",
        sealed_len,
        external_len
    ));
    let sealed_read_len = (sealed_blocks * u64::from(BEXFS_BLOCK_SIZE)) as usize;
    let mut sealed = vec![0u8; sealed_read_len];
    device.read_at(slot.lba + 1, &mut sealed)?;
    sealed.truncate(sealed_len as usize);
    open_namespace_in_place(key, &slot.nonce, &mut sealed).map_err(|_| BexFsError::Corrupt)?;
    let dentries = header.get(..8) == Some(&NAMESPACE_DENTRIES_MAGIC);
    let sparse = header.get(..8) == Some(&NAMESPACE_SPARSE_MAGIC);
    let (mut namespace, extents) = if dentries {
        deserialize_dentry_namespace(&sealed)?
    } else if sparse {
        deserialize_sparse_namespace(&sealed)?
    } else {
        deserialize_namespace_with_extents(&sealed)?
    };
    let data_lba = slot.lba + 1 + sealed_blocks;
    let external_read_len = (external_blocks * u64::from(BEXFS_BLOCK_SIZE)) as usize;
    let mut loaded_window = None;
    for extent in extents {
        let end = extent
            .offset
            .checked_add(extent.len)
            .ok_or(BexFsError::Corrupt)?;
        if end > external_len || !extent.offset.is_multiple_of(u64::from(BEXFS_BLOCK_SIZE)) {
            return Err(BexFsError::Corrupt);
        }
        let start = extent.offset as usize;
        let end = end as usize;
        let shared = if extent.chunk.is_some() && end - start == SPARSE_EXTENT_SIZE {
            let window_start = start / EXTERNAL_READ_WINDOW_BYTES * EXTERNAL_READ_WINDOW_BYTES;
            let bytes = Rc::clone(external_window(
                device,
                data_lba,
                external_read_len,
                window_start,
                &mut loaded_window,
            )?);
            Some(ExtentData::Shared {
                bytes,
                offset: start - window_start,
                len: SPARSE_EXTENT_SIZE,
            })
        } else {
            None
        };
        let data = if shared.is_none() {
            let mut data = Vec::with_capacity(end - start);
            let mut cursor = start;
            while cursor < end {
                let window_start = cursor / EXTERNAL_READ_WINDOW_BYTES * EXTERNAL_READ_WINDOW_BYTES;
                let bytes = external_window(
                    device,
                    data_lba,
                    external_read_len,
                    window_start,
                    &mut loaded_window,
                )?;
                let in_window = cursor - window_start;
                let copy = (end - cursor).min(bytes.len() - in_window);
                data.extend_from_slice(&bytes[in_window..in_window + copy]);
                cursor += copy;
            }
            Some(data)
        } else {
            None
        };
        let node = namespace
            .nodes
            .get_mut(&extent.inode)
            .ok_or(BexFsError::Corrupt)?;
        if let Some(chunk) = extent.chunk {
            let stored = if let Some(shared) = shared {
                shared
            } else {
                let data = data.unwrap();
                if data.len() > SPARSE_EXTENT_SIZE {
                    return Err(BexFsError::Corrupt);
                }
                let mut stored = vec![0; SPARSE_EXTENT_SIZE];
                stored[..data.len()].copy_from_slice(&data);
                ExtentData::Owned(stored)
            };
            node.data.extents.insert(chunk, stored);
        } else {
            node.data = SparseData::from_dense(data.unwrap());
        }
        update_size(node);
    }
    validate_namespace(&namespace)?;
    Ok(namespace)
}

fn write_header_pair(
    device: &dyn BlockDevice,
    header_lba: u64,
    header: &BexfsHeader,
) -> Result<(), BexFsError> {
    let encoded = header.encode(BEXFS_BLOCK_SIZE as usize);
    device.write_at(header_lba + 1, &encoded)?;
    device.flush()?;
    device.write_at(header_lba, &encoded)?;
    device.flush()?;
    Ok(())
}

fn read_header_pair(device: &dyn BlockDevice, header_lba: u64) -> Result<BexfsHeader, BexFsError> {
    let mut primary = vec![0u8; BEXFS_BLOCK_SIZE as usize];
    let mut backup = vec![0u8; BEXFS_BLOCK_SIZE as usize];
    device.read_at(header_lba, &mut primary)?;
    device.read_at(header_lba + 1, &mut backup)?;
    match (BexfsHeader::decode(&primary), BexfsHeader::decode(&backup)) {
        (Ok(a), Ok(b)) => Ok(if a.generation >= b.generation { a } else { b }),
        (Ok(header), Err(_)) | (Err(_), Ok(header)) => Ok(header),
        (Err(_), Err(_)) => Err(BexFsError::Corrupt),
    }
}

fn serialize_namespace_with_extents(
    namespace: &Namespace,
) -> Result<(Vec<u8>, Vec<ExternalWriteRef<'_>>, u64), BexFsError> {
    let mut out = Vec::new();
    let mut external = Vec::new();
    let mut external_len = 0u64;
    out.extend_from_slice(&NAMESPACE_DENTRIES_MAGIC);
    out.extend_from_slice(&namespace.next_inode.to_le_bytes());
    out.extend_from_slice(&(namespace.nodes.len() as u64).to_le_bytes());
    for node in namespace.nodes.values() {
        out.extend_from_slice(&node.inode.to_le_bytes());
        out.push(match node.kind {
            NodeKind::File => 1,
            NodeKind::Directory => 2,
            NodeKind::Symlink => 3,
        });
        out.extend_from_slice(&node.attributes.mode.to_le_bytes());
        out.extend_from_slice(&node.attributes.uid.to_le_bytes());
        out.extend_from_slice(&node.attributes.gid.to_le_bytes());
        out.extend_from_slice(&node.attributes.creation_time_nanos.to_le_bytes());
        out.extend_from_slice(&node.attributes.modification_time_nanos.to_le_bytes());
        out.extend_from_slice(&node.data.len().to_le_bytes());
        if node.kind == NodeKind::Directory {
            out.extend_from_slice(&0u64.to_le_bytes());
        } else {
            let allocated = node.data.allocated_bytes();
            out.extend_from_slice(&(node.data.extents.len() as u64).to_le_bytes());
            for (chunk, bytes) in node.data.iter_allocated() {
                out.extend_from_slice(&chunk.to_le_bytes());
                out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                if allocated as usize > EXTERNAL_DATA_THRESHOLD {
                    let offset = external_len.next_multiple_of(u64::from(BEXFS_BLOCK_SIZE));
                    external_len = offset
                        .checked_add(bytes.len() as u64)
                        .ok_or(BexFsError::NoSpace)?;
                    external.push(ExternalWriteRef { offset, bytes });
                    out.push(2);
                    out.extend_from_slice(&offset.to_le_bytes());
                } else {
                    out.push(1);
                    out.extend_from_slice(bytes);
                }
            }
        }
        out.extend_from_slice(&(node.xattrs.len() as u64).to_le_bytes());
        for (name, value) in &node.xattrs {
            if name.len() > 255 || value.len() > 65_536 {
                return Err(BexFsError::Corrupt);
            }
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&(value.len() as u32).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(value);
        }
    }
    out.extend_from_slice(&(namespace.dentries.len() as u64).to_le_bytes());
    for ((parent, name), inode) in &namespace.dentries {
        if name.len() > 255 {
            return Err(BexFsError::Corrupt);
        }
        out.extend_from_slice(&parent.to_le_bytes());
        out.extend_from_slice(&inode.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    Ok((out, external, external_len))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExternalDataRef {
    inode: u64,
    chunk: Option<u64>,
    offset: u64,
    len: u64,
}

fn deserialize_dentry_namespace(
    bytes: &[u8],
) -> Result<(Namespace, Vec<ExternalDataRef>), BexFsError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != NAMESPACE_DENTRIES_MAGIC {
        return Err(BexFsError::Corrupt);
    }
    let next_inode = cursor.u64()?;
    let count = usize::try_from(cursor.u64()?).map_err(|_| BexFsError::Corrupt)?;
    if count == 0 || count > 1_000_000 {
        return Err(BexFsError::Corrupt);
    }
    let mut nodes = BTreeMap::new();
    let mut external = Vec::new();
    for _ in 0..count {
        let inode = cursor.u64()?;
        let kind = match cursor.u8()? {
            1 => NodeKind::File,
            2 => NodeKind::Directory,
            3 => NodeKind::Symlink,
            _ => return Err(BexFsError::Corrupt),
        };
        let mode = cursor.u32()?;
        let uid = cursor.u32()?;
        let gid = cursor.u32()?;
        let creation_time_nanos = cursor.u64()?;
        let modification_time_nanos = cursor.u64()?;
        let logical_size = cursor.u64()?;
        let extent_count = cursor.u64()?;
        let mut data = SparseData {
            logical_size,
            extents: BTreeMap::new(),
        };
        for _ in 0..extent_count {
            let chunk = cursor.u64()?;
            let len = usize::try_from(cursor.u32()?).map_err(|_| BexFsError::Corrupt)?;
            if len == 0 || len > SPARSE_EXTENT_SIZE {
                return Err(BexFsError::Corrupt);
            }
            match cursor.u8()? {
                1 => {
                    let bytes = cursor.take(len)?;
                    let mut stored = vec![0; SPARSE_EXTENT_SIZE];
                    stored[..len].copy_from_slice(bytes);
                    if data
                        .extents
                        .insert(chunk, ExtentData::Owned(stored))
                        .is_some()
                    {
                        return Err(BexFsError::Corrupt);
                    }
                }
                2 => external.push(ExternalDataRef {
                    inode,
                    chunk: Some(chunk),
                    offset: cursor.u64()?,
                    len: len as u64,
                }),
                _ => return Err(BexFsError::Corrupt),
            }
        }
        let xattr_count = usize::try_from(cursor.u64()?).map_err(|_| BexFsError::Corrupt)?;
        if xattr_count > 256 {
            return Err(BexFsError::Corrupt);
        }
        let mut xattrs = BTreeMap::new();
        for _ in 0..xattr_count {
            let name_len = cursor.u16()? as usize;
            let value_len = usize::try_from(cursor.u32()?).map_err(|_| BexFsError::Corrupt)?;
            if value_len > 65_536 {
                return Err(BexFsError::Corrupt);
            }
            let name = core::str::from_utf8(cursor.take(name_len)?)
                .map_err(|_| BexFsError::Corrupt)?
                .to_string();
            validate_xattr_name(&name).map_err(|_| BexFsError::Corrupt)?;
            let value = cursor.take(value_len)?.to_vec();
            if xattrs.insert(name, value).is_some() {
                return Err(BexFsError::Corrupt);
            }
        }
        let attributes = NodeAttributes {
            size_bytes: logical_size,
            storage_allocated_bytes: data.allocated_bytes(),
            creation_time_nanos,
            modification_time_nanos,
            mode,
            uid,
            gid,
        };
        if nodes
            .insert(
                inode,
                Node {
                    inode,
                    kind,
                    attributes,
                    data,
                    xattrs,
                },
            )
            .is_some()
        {
            return Err(BexFsError::Corrupt);
        }
    }
    let dentry_count = usize::try_from(cursor.u64()?).map_err(|_| BexFsError::Corrupt)?;
    if dentry_count > 4_000_000 {
        return Err(BexFsError::Corrupt);
    }
    let mut dentries = BTreeMap::new();
    for _ in 0..dentry_count {
        let parent = cursor.u64()?;
        let inode = cursor.u64()?;
        let name_len = cursor.u16()? as usize;
        let name = core::str::from_utf8(cursor.take(name_len)?)
            .map_err(|_| BexFsError::Corrupt)?
            .to_string();
        if dentries.insert((parent, name), inode).is_some() {
            return Err(BexFsError::Corrupt);
        }
    }
    if !cursor.remaining().is_empty() {
        return Err(BexFsError::Corrupt);
    }
    let namespace = Namespace {
        next_inode,
        nodes,
        dentries,
    };
    validate_namespace(&namespace)?;
    Ok((namespace, external))
}

fn deserialize_sparse_namespace(
    bytes: &[u8],
) -> Result<(Namespace, Vec<ExternalDataRef>), BexFsError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != NAMESPACE_SPARSE_MAGIC {
        return Err(BexFsError::Corrupt);
    }
    let next_inode = cursor.u64()?;
    let count = usize::try_from(cursor.u64()?).map_err(|_| BexFsError::Corrupt)?;
    if count == 0 || count > 1_000_000 {
        return Err(BexFsError::Corrupt);
    }
    let mut nodes = BTreeMap::new();
    let mut dentries = BTreeMap::new();
    let mut external = Vec::new();
    for _ in 0..count {
        let inode = cursor.u64()?;
        let parent = cursor.u64()?;
        let kind = match cursor.u8()? {
            1 => NodeKind::File,
            2 => NodeKind::Directory,
            _ => return Err(BexFsError::Corrupt),
        };
        let name_len = cursor.u16()? as usize;
        let mode = cursor.u32()?;
        let creation_time_nanos = cursor.u64()?;
        let modification_time_nanos = cursor.u64()?;
        let logical_size = cursor.u64()?;
        let name = core::str::from_utf8(cursor.take(name_len)?)
            .map_err(|_| BexFsError::Corrupt)?
            .to_string();
        if inode != ROOT_INODE && dentries.insert((parent, name), inode).is_some() {
            return Err(BexFsError::Corrupt);
        }
        let extent_count = cursor.u64()?;
        let mut data = SparseData {
            logical_size,
            extents: BTreeMap::new(),
        };
        for _ in 0..extent_count {
            let chunk = cursor.u64()?;
            let len = usize::try_from(cursor.u32()?).map_err(|_| BexFsError::Corrupt)?;
            if len == 0 || len > SPARSE_EXTENT_SIZE {
                return Err(BexFsError::Corrupt);
            }
            match cursor.u8()? {
                1 => {
                    let bytes = cursor.take(len)?;
                    let mut stored = vec![0; SPARSE_EXTENT_SIZE];
                    stored[..len].copy_from_slice(bytes);
                    if data
                        .extents
                        .insert(chunk, ExtentData::Owned(stored))
                        .is_some()
                    {
                        return Err(BexFsError::Corrupt);
                    }
                }
                2 => {
                    let offset = cursor.u64()?;
                    external.push(ExternalDataRef {
                        inode,
                        chunk: Some(chunk),
                        offset,
                        len: len as u64,
                    });
                }
                _ => return Err(BexFsError::Corrupt),
            }
        }
        let attributes = NodeAttributes {
            size_bytes: logical_size,
            storage_allocated_bytes: data.allocated_bytes(),
            creation_time_nanos,
            modification_time_nanos,
            mode,
            uid: 0,
            gid: 0,
        };
        if nodes
            .insert(
                inode,
                Node {
                    inode,
                    kind,
                    attributes,
                    data,
                    xattrs: BTreeMap::new(),
                },
            )
            .is_some()
        {
            return Err(BexFsError::Corrupt);
        }
    }
    if !cursor.remaining().is_empty() {
        return Err(BexFsError::Corrupt);
    }
    let namespace = Namespace {
        next_inode,
        nodes,
        dentries,
    };
    validate_namespace(&namespace)?;
    Ok((namespace, external))
}

fn deserialize_namespace_with_extents(
    bytes: &[u8],
) -> Result<(Namespace, Vec<ExternalDataRef>), BexFsError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != NAMESPACE_EXTENTS_MAGIC {
        return Err(BexFsError::Corrupt);
    }
    let next_inode = cursor.u64()?;
    let count = usize::try_from(cursor.u64()?).map_err(|_| BexFsError::Corrupt)?;
    if count == 0 || count > 1_000_000 {
        return Err(BexFsError::Corrupt);
    }
    let mut nodes = BTreeMap::new();
    let mut dentries = BTreeMap::new();
    let mut extents = Vec::new();
    for _ in 0..count {
        let inode = cursor.u64()?;
        let parent = cursor.u64()?;
        let kind = match cursor.u8()? {
            1 => NodeKind::File,
            2 => NodeKind::Directory,
            _ => return Err(BexFsError::Corrupt),
        };
        let name_len = cursor.u16()? as usize;
        let mode = cursor.u32()?;
        let creation_time_nanos = cursor.u64()?;
        let modification_time_nanos = cursor.u64()?;
        let data_len = cursor.u64()?;
        let name = core::str::from_utf8(cursor.take(name_len)?)
            .map_err(|_| BexFsError::Corrupt)?
            .to_string();
        if inode != ROOT_INODE && dentries.insert((parent, name), inode).is_some() {
            return Err(BexFsError::Corrupt);
        }
        let data = match cursor.u8()? {
            1 => {
                let data = cursor
                    .take(usize::try_from(data_len).map_err(|_| BexFsError::Corrupt)?)?
                    .to_vec();
                if data.len() as u64 != data_len {
                    return Err(BexFsError::Corrupt);
                }
                data
            }
            2 => {
                let offset = cursor.u64()?;
                extents.push(ExternalDataRef {
                    inode,
                    chunk: None,
                    offset,
                    len: data_len,
                });
                Vec::new()
            }
            _ => return Err(BexFsError::Corrupt),
        };
        let attributes = NodeAttributes {
            size_bytes: data_len,
            storage_allocated_bytes: data_len.div_ceil(4096) * 4096,
            creation_time_nanos,
            modification_time_nanos,
            mode,
            uid: 0,
            gid: 0,
        };
        if nodes
            .insert(
                inode,
                Node {
                    inode,
                    kind,
                    attributes,
                    data: SparseData::from_dense(data),
                    xattrs: BTreeMap::new(),
                },
            )
            .is_some()
        {
            return Err(BexFsError::Corrupt);
        }
    }
    if !cursor.remaining().is_empty() {
        return Err(BexFsError::Corrupt);
    }
    let namespace = Namespace {
        next_inode,
        nodes,
        dentries,
    };
    validate_namespace(&namespace)?;
    Ok((namespace, extents))
}

fn deserialize_namespace(bytes: &[u8]) -> Result<Namespace, BexFsError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != NAMESPACE_MAGIC {
        return Err(BexFsError::Corrupt);
    }
    let next_inode = cursor.u64()?;
    let count = usize::try_from(cursor.u64()?).map_err(|_| BexFsError::Corrupt)?;
    if count == 0 || count > 1_000_000 {
        return Err(BexFsError::Corrupt);
    }
    let mut nodes = BTreeMap::new();
    let mut dentries = BTreeMap::new();
    for _ in 0..count {
        let inode = cursor.u64()?;
        let parent = cursor.u64()?;
        let kind = match cursor.u8()? {
            1 => NodeKind::File,
            2 => NodeKind::Directory,
            _ => return Err(BexFsError::Corrupt),
        };
        let name_len = cursor.u16()? as usize;
        let mode = cursor.u32()?;
        let creation_time_nanos = cursor.u64()?;
        let modification_time_nanos = cursor.u64()?;
        let data_len = usize::try_from(cursor.u64()?).map_err(|_| BexFsError::Corrupt)?;
        let name = core::str::from_utf8(cursor.take(name_len)?)
            .map_err(|_| BexFsError::Corrupt)?
            .to_string();
        if inode != ROOT_INODE && dentries.insert((parent, name), inode).is_some() {
            return Err(BexFsError::Corrupt);
        }
        let data = cursor.take(data_len)?.to_vec();
        let attributes = NodeAttributes {
            size_bytes: data.len() as u64,
            storage_allocated_bytes: data.len().div_ceil(4096) as u64 * 4096,
            creation_time_nanos,
            modification_time_nanos,
            mode,
            uid: 0,
            gid: 0,
        };
        if nodes
            .insert(
                inode,
                Node {
                    inode,
                    kind,
                    attributes,
                    data: SparseData::from_dense(data),
                    xattrs: BTreeMap::new(),
                },
            )
            .is_some()
        {
            return Err(BexFsError::Corrupt);
        }
    }
    if !cursor.remaining().is_empty() {
        return Err(BexFsError::Corrupt);
    }
    let namespace = Namespace {
        next_inode,
        nodes,
        dentries,
    };
    validate_namespace(&namespace)?;
    Ok(namespace)
}

fn validate_namespace(namespace: &Namespace) -> Result<(), BexFsError> {
    if namespace.next_inode <= ROOT_INODE
        || namespace
            .nodes
            .get(&ROOT_INODE)
            .is_none_or(|node| node.kind != NodeKind::Directory)
    {
        return Err(BexFsError::Corrupt);
    }
    for node in namespace.nodes.values() {
        if node.inode >= namespace.next_inode
            || node.kind != NodeKind::Directory && node.attributes.size_bytes != node.data.len()
        {
            return Err(BexFsError::Corrupt);
        }
    }
    for ((parent, name), inode) in &namespace.dentries {
        validate_component(name).map_err(|_| BexFsError::Corrupt)?;
        if *inode == ROOT_INODE
            || !namespace.nodes.contains_key(inode)
            || namespace
                .nodes
                .get(parent)
                .is_none_or(|node| node.kind != NodeKind::Directory)
        {
            return Err(BexFsError::Corrupt);
        }
    }
    for inode in namespace
        .nodes
        .keys()
        .copied()
        .filter(|inode| *inode != ROOT_INODE)
    {
        let links = namespace
            .dentries
            .values()
            .filter(|child| **child == inode)
            .count();
        if links == 0 || namespace.nodes[&inode].kind == NodeKind::Directory && links != 1 {
            return Err(BexFsError::Corrupt);
        }
    }
    for inode in namespace
        .nodes
        .values()
        .filter(|node| node.kind == NodeKind::Directory)
        .map(|node| node.inode)
    {
        let mut current = inode;
        for _ in 0..namespace.nodes.len() {
            if current == ROOT_INODE {
                break;
            }
            current = namespace
                .dentries
                .iter()
                .find(|(_, child)| **child == current)
                .map(|((parent, _), _)| *parent)
                .ok_or(BexFsError::Corrupt)?;
        }
        if current != ROOT_INODE {
            return Err(BexFsError::Corrupt);
        }
    }
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], BexFsError> {
        let end = self.offset.checked_add(len).ok_or(BexFsError::Corrupt)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(BexFsError::Corrupt)?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, BexFsError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, BexFsError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, BexFsError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, BexFsError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.offset..]
    }
}
