//! Versioned node records; backing archives and owned files use bounded chunks.
use super::*;
use alloc::vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
const CHUNK: usize = 32 * 1024 - 64;
const ARCHIVE_CHUNK: u64 = 1 << 39;
impl ArchiveFs {
    pub fn empty_checkpoint() -> Self {
        Self {
            archive_bytes: Vec::new(),
            content_root: [0; 32],
            nodes: Vec::new(),
        }
    }
    pub fn checkpoint_keys(&self) -> Vec<u64> {
        let mut keys = vec![0];
        keys.extend(
            (0..self.archive_bytes.len().div_ceil(CHUNK))
                .map(|chunk| ARCHIVE_CHUNK | chunk as u64 + 1),
        );
        for (i, n) in self.nodes.iter().enumerate() {
            let base = (i as u64 + 1) << 16;
            keys.push(base);
            if let Node::File {
                data: FileData::Owned(data),
                ..
            } = n
            {
                keys.extend((0..data.len().div_ceil(CHUNK)).map(|c| base + c as u64 + 1));
            }
        }
        keys
    }
    pub fn checkpoint_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key == 0 {
            let mut w = Encoder::new();
            w.word(2);
            w.word(self.archive_bytes.len() as u64);
            w.bytes(&self.content_root);
            return Ok(Some(w.finish()));
        }
        if key & ARCHIVE_CHUNK != 0 {
            let chunk = (key & !ARCHIVE_CHUNK)
                .checked_sub(1)
                .ok_or(Error::InvalidData)? as usize;
            let start = chunk.checked_mul(CHUNK).ok_or(Error::Capacity)?;
            return Ok(self
                .archive_bytes
                .get(start..self.archive_bytes.len().min(start.saturating_add(CHUNK)))
                .map(|bytes| bytes.to_vec()));
        }
        let i = (key >> 16).checked_sub(1).ok_or(Error::InvalidData)? as usize;
        let Some(node) = self.nodes.get(i) else {
            return Ok(None);
        };
        let chunk = key & 0xffff;
        let mut w = Encoder::new();
        if chunk == 0 {
            match node {
                Node::Directory { children } => {
                    w.word(1);
                    w.word(children.len() as u64);
                    for (name, inode) in children {
                        w.text(name);
                        w.word(*inode);
                    }
                }
                Node::File { data, mode } => match data {
                    FileData::Owned(data) => {
                        w.word(2);
                        w.word(*mode as u64);
                        w.word(data.len() as u64);
                    }
                    FileData::Archive { offset, len } => {
                        w.word(3);
                        w.word(*mode as u64);
                        w.word(*offset as u64);
                        w.word(*len as u64);
                    }
                    FileData::CompressedArchive {
                        offset,
                        len,
                        uncompressed_len,
                    } => {
                        w.word(4);
                        w.word(*mode as u64);
                        w.word(*offset as u64);
                        w.word(*len as u64);
                        w.word(*uncompressed_len as u64);
                    }
                },
            }
        } else if let Node::File {
            data: FileData::Owned(bytes),
            ..
        } = node
        {
            let start = (chunk as usize - 1) * CHUNK;
            return Ok(bytes
                .get(start..bytes.len().min(start + CHUNK))
                .map(|s| s.to_vec()));
        } else {
            return Err(Error::InvalidData);
        }
        Ok(Some(w.finish()))
    }
    pub fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let bytes = bytes.ok_or(Error::InvalidData)?;
        if key == 0 {
            let mut r = Decoder::new(bytes);
            let version = r.word()?;
            self.archive_bytes.resize(r.count(32 * 1024 * 1024)?, 0);
            self.content_root = match version {
                1 => [0; 32],
                2 => r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?,
                _ => return Err(Error::UnsupportedVersion),
            };
            r.finish()?;
            return Ok(());
        }
        if key & ARCHIVE_CHUNK != 0 {
            let chunk = (key & !ARCHIVE_CHUNK)
                .checked_sub(1)
                .ok_or(Error::InvalidData)? as usize;
            let start = chunk.checked_mul(CHUNK).ok_or(Error::Capacity)?;
            if bytes.len() > CHUNK {
                return Err(Error::InvalidData);
            }
            self.archive_bytes
                .get_mut(start..start.saturating_add(bytes.len()))
                .ok_or(Error::InvalidData)?
                .copy_from_slice(bytes);
            return Ok(());
        }
        let i = (key >> 16).checked_sub(1).ok_or(Error::InvalidData)? as usize;
        if i > self.nodes.len() || i >= 16384 {
            return Err(Error::InvalidData);
        }
        let chunk = key & 0xffff;
        if chunk == 0 {
            let mut r = Decoder::new(bytes);
            let node = match r.word()? {
                1 => {
                    let mut children = BTreeMap::new();
                    for _ in 0..r.count(4096)? {
                        let name = r.text(256)?.to_string();
                        let inode = r.word()?;
                        if name.is_empty()
                            || name.contains('/')
                            || children.insert(name, inode).is_some()
                        {
                            return Err(Error::InvalidData);
                        }
                    }
                    Node::Directory { children }
                }
                2 => {
                    let mode = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                    let len = r.count(8 * 1024 * 1024)?;
                    Node::File {
                        data: FileData::Owned(alloc::vec![0;len]),
                        mode,
                    }
                }
                3 => {
                    let mode = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                    let offset = r.count(32 * 1024 * 1024)?;
                    let len = r.count(8 * 1024 * 1024)?;
                    Node::File {
                        data: FileData::Archive { offset, len },
                        mode,
                    }
                }
                4 => {
                    let mode = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                    let offset = r.count(32 * 1024 * 1024)?;
                    let len = r.count(8 * 1024 * 1024)?;
                    let uncompressed_len = r.count(8 * 1024 * 1024)?;
                    Node::File {
                        data: FileData::CompressedArchive {
                            offset,
                            len,
                            uncompressed_len,
                        },
                        mode,
                    }
                }
                _ => return Err(Error::InvalidData),
            };
            r.finish()?;
            if i == self.nodes.len() {
                self.nodes.push(node);
            } else {
                self.nodes[i] = node;
            }
        } else {
            let Some(Node::File {
                data: FileData::Owned(data),
                ..
            }) = self.nodes.get_mut(i)
            else {
                return Err(Error::InvalidData);
            };
            let start = (chunk as usize - 1) * CHUNK;
            if bytes.len() > CHUNK {
                return Err(Error::InvalidData);
            }
            data.get_mut(start..start + bytes.len())
                .ok_or(Error::InvalidData)?
                .copy_from_slice(bytes);
        }
        Ok(())
    }
    pub fn validate_checkpoint(&self) -> Result<(), Error> {
        if !matches!(self.nodes.first(), Some(Node::Directory { .. })) {
            return Err(Error::InvalidData);
        }
        let mut parents = alloc::vec![0; self.nodes.len()];
        for node in &self.nodes {
            if let Node::File { data, .. } = node {
                let range = match data {
                    FileData::Owned(_) => None,
                    FileData::Archive { offset, len }
                    | FileData::CompressedArchive { offset, len, .. } => Some((*offset, *len)),
                };
                if range.is_some_and(|(offset, len)| {
                    offset
                        .checked_add(len)
                        .is_none_or(|end| end > self.archive_bytes.len())
                }) {
                    return Err(Error::InvalidData);
                }
            }
            if let Node::Directory { children } = node {
                for id in children.values() {
                    if *id == 0 || *id as usize >= parents.len() {
                        return Err(Error::InvalidData);
                    }
                    parents[*id as usize] += 1;
                }
            }
        }
        if parents.iter().skip(1).any(|p| *p != 1) {
            return Err(Error::InvalidData);
        }
        let mut reached = alloc::vec![false; self.nodes.len()];
        let mut pending = alloc::vec![0usize];
        while let Some(id) = pending.pop() {
            if reached[id] {
                return Err(Error::InvalidData);
            }
            reached[id] = true;
            if let Node::Directory { children } = &self.nodes[id] {
                pending.extend(children.values().map(|id| *id as usize));
            }
        }
        if reached.iter().any(|seen| !seen) {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
}
impl FileHandle {
    pub fn checkpoint(&self, w: &mut Encoder) {
        w.word(self.inode);
        w.word(self.offset);
    }
    pub fn adopt(r: &mut Decoder<'_>) -> Result<Self, Error> {
        Ok(Self {
            inode: r.word()?,
            offset: r.word()?,
        })
    }
}
