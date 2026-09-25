//! Bounded synchronous client for the common BexOS filesystem protocol.

use bexos_userspace::{Channel, Memory};
use fs_fidl::{FidlDecode, FidlEncode, FsStatus, HandleRef};

pub struct Node {
    channel: Channel,
}

pub struct Directory(Node);
pub struct File(Node);

impl Node {
    pub fn from_channel(channel: Channel) -> Self {
        Self { channel }
    }

    pub fn into_channel(self) -> Channel {
        let channel = self.channel;
        core::mem::forget(self);
        channel
    }

    pub fn get_attr(&self) -> Result<fs_fidl::FileAttributes, FsStatus> {
        let reply = self.call(1, &fs_fidl::NodeGetAttrRequest {})?;
        let response = fs_fidl::NodeGetAttrResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)?;
        Ok(response.attr)
    }

    pub fn set_attr(&self, attr: fs_fidl::FileAttributes) -> Result<(), FsStatus> {
        let reply = self.call(2, &fs_fidl::NodeSetAttrRequest { attr })?;
        let response = fs_fidl::NodeSetAttrResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }

    pub fn get_xattr(&self, name: &str) -> Result<Vec<u8>, FsStatus> {
        let reply = self.call(4, &fs_fidl::NodeGetXattrRequest { name })?;
        let response = fs_fidl::NodeGetXattrResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)?;
        Ok(response.value.to_vec())
    }

    pub fn set_xattr(&self, name: &str, value: &[u8], flags: u32) -> Result<(), FsStatus> {
        let reply = self.call(5, &fs_fidl::NodeSetXattrRequest { name, value, flags })?;
        let response = fs_fidl::NodeSetXattrResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }

    pub fn list_xattrs(&self) -> Result<Vec<String>, FsStatus> {
        let reply = self.call(6, &fs_fidl::NodeListXattrsRequest {})?;
        let response = fs_fidl::NodeListXattrsResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)?;
        (0..response.names.len())
            .map(|index| {
                response
                    .names
                    .get(index)
                    .map(String::from)
                    .map_err(|_| FsStatus::Corrupt)
            })
            .collect()
    }

    pub fn remove_xattr(&self, name: &str) -> Result<(), FsStatus> {
        let reply = self.call(7, &fs_fidl::NodeRemoveXattrRequest { name })?;
        let response = fs_fidl::NodeRemoveXattrResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }

    fn call<Q: FidlEncode>(
        &self,
        ordinal: u64,
        request: &Q,
    ) -> Result<bexos_userspace::Message, FsStatus> {
        let mut bytes = vec![0; 65_536];
        let encoded = request
            .encode(&mut bytes[8..], &mut [])
            .map_err(|_| FsStatus::InvalidArgs)?;
        bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
        self.channel
            .send(&bytes[..encoded.bytes + 8], &[])
            .map_err(|_| FsStatus::Io)?;
        self.channel.recv_blocking().map_err(|_| FsStatus::Io)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = Memory::close(self.channel.0);
    }
}

impl Directory {
    pub fn from_channel(channel: Channel) -> Self {
        Self(Node::from_channel(channel))
    }

    pub fn into_channel(self) -> Channel {
        self.0.into_channel()
    }

    pub fn node(&self) -> &Node {
        &self.0
    }

    pub fn open_node(&self, path: &str, flags: u32) -> Result<Node, FsStatus> {
        if path.is_empty() || path.len() > 255 {
            return Err(FsStatus::InvalidArgs);
        }
        let (client, server) = Channel::pair().map_err(|_| FsStatus::Io)?;
        let request = fs_fidl::DirectoryOpenRequest {
            path,
            flags: fs_fidl::OpenFlags(flags),
            object: HandleRef { raw: server.0 },
        };
        let mut bytes = vec![0; 1024];
        let mut handles = [HandleRef { raw: 0 }; 1];
        let encoded = request
            .encode(&mut bytes[8..], &mut handles)
            .map_err(|_| FsStatus::InvalidArgs)?;
        bytes[..8].copy_from_slice(&20u64.to_le_bytes());
        if self
            .0
            .channel
            .send(&bytes[..encoded.bytes + 8], &[server.0])
            .is_err()
        {
            let _ = Memory::close(client.0);
            let _ = Memory::close(server.0);
            return Err(FsStatus::Io);
        }
        let node = Node::from_channel(client);
        node.get_attr()?;
        Ok(node)
    }

    pub fn open_directory(&self, path: &str, create: bool) -> Result<Self, FsStatus> {
        let flags = 0x1 | 0x20 | if create { 0x8 | 0x2 } else { 0 };
        Ok(Self(self.open_node(path, flags)?))
    }

    pub fn open_file(&self, path: &str, create: bool, truncate: bool) -> Result<File, FsStatus> {
        let flags = 0x1
            | if create || truncate { 0x2 } else { 0 }
            | if create { 0x8 } else { 0 }
            | if truncate { 0x10 } else { 0 };
        Ok(File(self.open_node(path, flags)?))
    }

    pub fn open_no_follow(&self, path: &str) -> Result<Node, FsStatus> {
        self.open_node(path, 0x1 | 0x40)
    }

    pub fn read_entries(&self) -> Result<Vec<OwnedDirEntry>, FsStatus> {
        let reply = self.0.call(21, &fs_fidl::DirectoryReadEntriesRequest {})?;
        let response = fs_fidl::DirectoryReadEntriesResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)?;
        (0..response.entries.len())
            .map(|index| {
                let entry = response.entries.get(index).map_err(|_| FsStatus::Corrupt)?;
                Ok(OwnedDirEntry {
                    name: entry.name.to_string(),
                    kind: entry.kind,
                    attr: entry.attr,
                })
            })
            .collect()
    }

    pub fn unlink(&self, name: &str) -> Result<(), FsStatus> {
        let reply = self.0.call(22, &fs_fidl::DirectoryUnlinkRequest { name })?;
        let response = fs_fidl::DirectoryUnlinkResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }

    pub fn rename(&self, source: &str, target: &str) -> Result<(), FsStatus> {
        let reply = self
            .0
            .call(23, &fs_fidl::DirectoryRenameRequest { source, target })?;
        let response = fs_fidl::DirectoryRenameResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }

    pub fn link(&self, source: &str, target: &str) -> Result<(), FsStatus> {
        let reply = self
            .0
            .call(24, &fs_fidl::DirectoryLinkRequest { source, target })?;
        let response = fs_fidl::DirectoryLinkResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }

    pub fn symlink(&self, target: &str, link_path: &str) -> Result<(), FsStatus> {
        let reply = self
            .0
            .call(25, &fs_fidl::DirectorySymlinkRequest { target, link_path })?;
        let response = fs_fidl::DirectorySymlinkResponse::decode(&reply.bytes, &[])
            .map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }

    /// Remove one relative subtree without following symbolic links.
    pub fn remove_tree(&self, path: &str) -> Result<(), FsStatus> {
        let (parent_path, name) = path.rsplit_once('/').unwrap_or(("", path));
        if name.is_empty() {
            return Err(FsStatus::InvalidArgs);
        }
        let parent = if parent_path.is_empty() {
            None
        } else {
            Some(self.open_directory(parent_path, false)?)
        };
        let directory = parent.as_ref().unwrap_or(self);
        let entry = directory
            .read_entries()?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or(FsStatus::NotFound)?;
        if entry.kind == fs_fidl::NodeKind::Directory {
            let child = self.open_directory(path, false)?;
            for entry in child.read_entries()? {
                child.remove_tree(&entry.name)?;
            }
        }
        directory.unlink(name)
    }
}

impl File {
    pub fn node(&self) -> &Node {
        &self.0
    }

    pub fn write_all(&self, mut bytes: &[u8]) -> Result<(), FsStatus> {
        while !bytes.is_empty() {
            let chunk = &bytes[..bytes.len().min(32 * 1024)];
            let reply = self
                .0
                .call(11, &fs_fidl::FileWriteRequest { data: chunk })?;
            let response = fs_fidl::FileWriteResponse::decode(&reply.bytes, &[])
                .map_err(|_| FsStatus::Corrupt)?;
            status(response.status)?;
            let written = usize::try_from(response.written).map_err(|_| FsStatus::Io)?;
            if written == 0 || written > chunk.len() {
                return Err(FsStatus::Io);
            }
            bytes = &bytes[written..];
        }
        Ok(())
    }

    /// Read the complete file with an explicit upper bound.
    pub fn read_all(&self, maximum: usize) -> Result<Vec<u8>, FsStatus> {
        let size = usize::try_from(self.0.get_attr()?.size_bytes).map_err(|_| FsStatus::Io)?;
        if size > maximum {
            return Err(FsStatus::InvalidArgs);
        }
        let mut result = Vec::with_capacity(size);
        while result.len() < size {
            let reply = self.0.call(
                10,
                &fs_fidl::FileReadRequest {
                    count: u64::try_from((size - result.len()).min(32 * 1024))
                        .map_err(|_| FsStatus::Io)?,
                },
            )?;
            let response = fs_fidl::FileReadResponse::decode(&reply.bytes, &[])
                .map_err(|_| FsStatus::Corrupt)?;
            status(response.status)?;
            if response.data.is_empty() {
                return Err(FsStatus::Corrupt);
            }
            result.extend_from_slice(&response.data);
            if result.len() > size || result.len() > maximum {
                return Err(FsStatus::Corrupt);
            }
        }
        Ok(result)
    }

    pub fn sync(&self) -> Result<(), FsStatus> {
        let reply = self.0.call(14, &fs_fidl::FileSyncRequest {})?;
        let response =
            fs_fidl::FileSyncResponse::decode(&reply.bytes, &[]).map_err(|_| FsStatus::Corrupt)?;
        status(response.status)
    }
}

pub struct OwnedDirEntry {
    pub name: String,
    pub kind: fs_fidl::NodeKind,
    pub attr: fs_fidl::FileAttributes,
}

fn status(value: FsStatus) -> Result<(), FsStatus> {
    if value == FsStatus::Ok {
        Ok(())
    } else {
        Err(value)
    }
}
