//! Logical volume state. Keys travel as protected VMO descriptors, never records.
use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
pub const CHUNK: usize = bexos_userspace::live_migration::MAX_RECORD_DATA;
impl BexFs {
    pub fn checkpoint_keys(&self) -> Vec<u64> {
        let mut keys = vec![0];
        for n in self.namespace.nodes.values() {
            keys.push(n.inode << 16);
            let mut chunks = BTreeSet::new();
            for (extent, bytes) in n.data.iter_allocated() {
                let start = extent * SPARSE_EXTENT_SIZE as u64;
                let end = (start + bytes.len() as u64).min(n.data.len());
                for chunk in start / CHUNK as u64..end.div_ceil(CHUNK as u64) {
                    let lo = start.max(chunk * CHUNK as u64) - start;
                    let hi = end.min((chunk + 1) * CHUNK as u64) - start;
                    if bytes[lo as usize..hi as usize]
                        .iter()
                        .any(|byte| *byte != 0)
                    {
                        chunks.insert(chunk);
                    }
                }
            }
            keys.extend(chunks.into_iter().map(|i| (n.inode << 16) | i + 1));
        }
        keys
    }
    pub fn checkpoint_key(&self) -> &[u8; 32] {
        self.key.expose()
    }
    pub fn activate_key(&mut self, key: LockedVolumeKey) {
        self.key = key;
    }
    pub fn checkpoint_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        if key == 0 {
            w.word(4);
            w.word(self.header_lba);
            w.bytes(&self.header.encode(160));
            w.word(self.read_only as u64);
            w.word(self.namespace.next_inode);
            w.word(self.namespace.dentries.len() as u64);
            for ((parent, name), inode) in &self.namespace.dentries {
                w.word(*parent);
                w.text(name);
                w.word(*inode);
            }
        } else {
            let Some(n) = self.namespace.nodes.get(&(key >> 16)) else {
                return Ok(None);
            };
            let chunk = key & 0xffff;
            if chunk != 0 {
                let start = (chunk - 1) * CHUNK as u64;
                if start >= n.data.len() {
                    return Ok(None);
                }
                return Ok(n
                    .data
                    .dense_chunk(chunk - 1, CHUNK)
                    .filter(|bytes| bytes.iter().any(|byte| *byte != 0)));
            }
            w.word(n.inode);
            w.word(match n.kind {
                NodeKind::File => 1,
                NodeKind::Directory => 2,
                NodeKind::Symlink => 3,
            });
            for value in [
                n.attributes.size_bytes,
                n.attributes.storage_allocated_bytes,
                n.attributes.creation_time_nanos,
                n.attributes.modification_time_nanos,
                n.attributes.mode as u64,
                n.attributes.uid as u64,
                n.attributes.gid as u64,
                n.data.len(),
            ] {
                w.word(value);
            }
            w.word(n.xattrs.len() as u64);
            for (name, value) in &n.xattrs {
                w.text(name);
                w.bytes(value);
            }
        }
        Ok(Some(w.finish()))
    }
    pub fn adopt_metadata(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let migration_format = r.word()?;
        if !matches!(migration_format, 1..=4) {
            return Err(Error::UnsupportedVersion);
        }
        let header_lba = r.word()?;
        let header = BexfsHeader::decode(r.bytes(160)?).map_err(|_| Error::InvalidData)?;
        let read_only = r.flag()?;
        let next_inode = r.word()?;
        let mut dentries = BTreeMap::new();
        if migration_format >= 2 {
            for _ in 0..r.count(4_000_000)? {
                let parent = r.word()?;
                let name = r.text(255)?.to_string();
                let inode = r.word()?;
                if dentries.insert((parent, name), inode).is_some() {
                    return Err(Error::InvalidData);
                }
            }
        }
        r.finish()?;
        Ok(Self {
            header_lba,
            header,
            read_only,
            namespace: Namespace {
                next_inode,
                nodes: BTreeMap::new(),
                dentries,
            },
            key: LockedVolumeKey::new(&[0; 32]).unwrap(),
            dirty: Default::default(),
            migration_format,
        })
    }
    pub fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key == 0 {
            let mut next = Self::adopt_metadata(bytes.ok_or(Error::InvalidData)?)?;
            core::mem::swap(&mut self.namespace.nodes, &mut next.namespace.nodes);
            core::mem::swap(&mut self.key, &mut next.key);
            *self = next;
            return Ok(());
        }
        let inode = key >> 16;
        let chunk = key & 0xffff;
        if inode == 0 || inode >= self.namespace.next_inode {
            return Err(Error::InvalidData);
        }
        let Some(bytes) = bytes else {
            if chunk == 0 {
                self.namespace.nodes.remove(&inode);
            } else if let Some(node) = self.namespace.nodes.get_mut(&inode) {
                let start = (chunk - 1) * CHUNK as u64;
                if start < node.data.len() {
                    let count = (node.data.len() - start).min(CHUNK as u64) as usize;
                    node.data
                        .write_at(start, &vec![0; count])
                        .map_err(|_| Error::InvalidData)?;
                }
            }
            return Ok(());
        };
        if chunk == 0 {
            let mut r = Decoder::new(bytes);
            if r.word()? != inode {
                return Err(Error::InvalidData);
            }
            let legacy_dentry = if self.migration_format == 1 {
                Some((r.word()?, r.text(256)?.to_string()))
            } else {
                None
            };
            let kind = match r.word()? {
                1 => NodeKind::File,
                2 => NodeKind::Directory,
                3 if self.migration_format >= 3 => NodeKind::Symlink,
                _ => return Err(Error::InvalidData),
            };
            let mut attributes = NodeAttributes {
                size_bytes: r.word()?,
                storage_allocated_bytes: r.word()?,
                creation_time_nanos: r.word()?,
                modification_time_nanos: r.word()?,
                mode: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                uid: 0,
                gid: 0,
            };
            if self.migration_format >= 4 {
                attributes.uid = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                attributes.gid = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            }
            // The record key reserves 16 bits for the chunk number. Logical
            // sparse length need not fit the receiver's allocated-state cap.
            let len = r.count(CHUNK * u16::MAX as usize)? as u64;
            let mut xattrs = BTreeMap::new();
            if self.migration_format >= 3 {
                for _ in 0..r.count(256)? {
                    let name = r.text(255)?.to_string();
                    validate_xattr_name(&name).map_err(|_| Error::InvalidData)?;
                    let value = r.bytes(65_536)?.to_vec();
                    if xattrs.insert(name, value).is_some() {
                        return Err(Error::InvalidData);
                    }
                }
            }
            r.finish()?;
            let mut data = self
                .namespace
                .nodes
                .remove(&inode)
                .map_or_else(SparseData::default, |n| n.data);
            data.truncate(len);
            self.namespace.nodes.insert(
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
                    self.namespace.dentries.insert((parent, name), inode);
                }
            }
        } else {
            let data = &mut self
                .namespace
                .nodes
                .get_mut(&inode)
                .ok_or(Error::InvalidData)?
                .data;
            let start = (chunk - 1) * CHUNK as u64;
            if bytes.len() > CHUNK {
                return Err(Error::InvalidData);
            }
            data.write_at(start, bytes)
                .map_err(|_| Error::InvalidData)?;
        }
        Ok(())
    }
    pub fn validate_checkpoint(&self) -> Result<(), Error> {
        validate_namespace(&self.namespace).map_err(|_| Error::InvalidData)
    }
    pub fn take_changes(&mut self) -> Vec<u64> {
        core::mem::take(&mut self.dirty).into_iter().collect()
    }
}
impl FileHandle {
    pub fn checkpoint(&self, w: &mut Encoder) {
        w.word(self.inode);
        w.word(self.cursor);
        w.word(self.readable as u64);
        w.word(self.writable as u64);
    }
    pub fn adopt(r: &mut Decoder<'_>) -> Result<Self, Error> {
        Ok(Self {
            inode: r.word()?,
            cursor: r.word()?,
            readable: r.flag()?,
            writable: r.flag()?,
        })
    }
}
