use bexos_userspace::{Channel, Memory, Socket, fs};
use fs_fidl::{FileAttributes, FsStatus, NodeKind, OpenFlags};
use starnix_kernel::{
    EACCES, EBADF, EEXIST, EINVAL, EIO, EISDIR, EMFILE, ENOENT, ENOSPC, ENOTDIR, ENOTEMPTY, EROFS,
};
use std::{string::String, vec::Vec};

pub const AT_FDCWD: i32 = -100;
const MAX_FDS: usize = 256;
const PATH_MAX: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stat {
    pub inode: u64,
    pub mode: u32,
    pub links: u64,
    pub size: u64,
    pub blocks: u64,
    pub created: u64,
    pub modified: u64,
    pub kind: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub inode: u64,
    pub name: String,
    pub kind: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DescriptorSnapshot {
    Stdio {
        source: u8,
    },
    File {
        path: String,
        flags: u32,
        offset: u64,
    },
    Directory {
        path: String,
        flags: u32,
    },
    Null {
        flags: u32,
    },
    Zero {
        flags: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub cwd: String,
    pub descriptors: Vec<(u16, DescriptorSnapshot)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StdioKind {
    Console,
    Socket(u64),
}

enum Descriptor {
    Stdio { kind: StdioKind, source: u8 },
    File { channel: Channel, path: String },
    Directory { channel: Channel, path: String },
    Null,
    Zero,
}

struct Fd {
    descriptor: Descriptor,
    flags: u32,
}

#[derive(Clone)]
struct Mount {
    path: String,
    directory: Channel,
    readonly: bool,
}

pub struct Vfs {
    mounts: Vec<Mount>,
    fds: Vec<Option<Fd>>,
    cwd: String,
}

impl Drop for Vfs {
    fn drop(&mut self) {
        let mut closed = Vec::new();
        for mount in &self.mounts {
            close_once(mount.directory.0, &mut closed);
        }
        for fd in self.fds.iter_mut().filter_map(Option::take) {
            match fd.descriptor {
                Descriptor::Stdio {
                    kind: StdioKind::Socket(raw),
                    ..
                }
                | Descriptor::File {
                    channel: Channel(raw),
                    ..
                }
                | Descriptor::Directory {
                    channel: Channel(raw),
                    ..
                } => close_once(raw, &mut closed),
                _ => {}
            }
        }
    }
}

fn close_once(raw: u64, closed: &mut Vec<u64>) {
    if raw != 0 && !closed.contains(&raw) {
        closed.push(raw);
        let _ = Memory::close(raw);
    }
}

fn errno(status: FsStatus) -> i64 {
    match status {
        FsStatus::NotFound => ENOENT,
        FsStatus::NotDirectory => ENOTDIR,
        FsStatus::IsDirectory => EISDIR,
        FsStatus::NotEmpty => ENOTEMPTY,
        FsStatus::NoSpace => ENOSPC,
        FsStatus::ReadOnly => EROFS,
        FsStatus::AccessDenied | FsStatus::Locked => EACCES,
        FsStatus::InvalidArgs | FsStatus::BadState => EINVAL,
        FsStatus::AlreadyExists => EEXIST,
        _ => EIO,
    }
}

fn normalize(base: &str, path: &str) -> Result<String, i64> {
    if path.is_empty() || path.len() > PATH_MAX || path.as_bytes().contains(&0) {
        return Err(if path.is_empty() { ENOENT } else { EINVAL });
    }
    let mut parts: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').filter(|part| !part.is_empty()).collect()
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    Ok(out)
}

impl Vfs {
    pub fn new(
        namespace: &[(String, u64)],
        root_source: &str,
        root_subpath: &str,
        stdio: [StdioKind; 3],
        command_cwd: bool,
    ) -> Result<Self, i64> {
        let selected = namespace
            .iter()
            .find(|(path, _)| path == root_source)
            .ok_or(ENOENT)?;
        let (_, rights) = Memory::object_info(selected.1).map_err(|_| EBADF)?;
        let root_handle = Memory::duplicate(selected.1, rights).map_err(|_| EBADF)?;
        let root = if root_subpath.trim_matches('/').is_empty() {
            Channel(root_handle)
        } else {
            let opened = fs::open(
                Channel(root_handle),
                root_subpath.trim_matches('/'),
                OpenFlags::RIGHT_READABLE.0 | OpenFlags::DIRECTORY.0,
            )
            .map_err(errno);
            let _ = Memory::close(root_handle);
            opened?
        };
        let mut mounts = vec![Mount {
            path: "/".into(),
            directory: root,
            readonly: root_source == "/pkg",
        }];
        for (path, raw) in namespace {
            if raw == &0 || path == "/" || path == root_source {
                continue;
            }
            let (_, rights) = Memory::object_info(*raw).map_err(|_| EBADF)?;
            let duplicate = Memory::duplicate(*raw, rights).map_err(|_| EBADF)?;
            mounts.push(Mount {
                path: path.trim_end_matches('/').into(),
                directory: Channel(duplicate),
                readonly: path == "/pkg" || path.starts_with("/deps/"),
            });
        }
        mounts.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
        let mut fds = Vec::with_capacity(MAX_FDS);
        for (source, kind) in stdio.into_iter().enumerate() {
            fds.push(Some(Fd {
                descriptor: Descriptor::Stdio {
                    kind,
                    source: source as u8,
                },
                flags: 0,
            }));
        }
        Ok(Self {
            mounts,
            fds,
            cwd: if command_cwd && namespace.iter().any(|(path, _)| path == "/cwd") {
                "/cwd".into()
            } else {
                "/".into()
            },
        })
    }

    fn resolve(&self, dirfd: i32, path: &str) -> Result<(String, &Mount, String), i64> {
        let base = if path.starts_with('/') || dirfd == AT_FDCWD {
            self.cwd.as_str()
        } else {
            match self.fds.get(dirfd as usize).and_then(Option::as_ref) {
                Some(Fd {
                    descriptor: Descriptor::Directory { path, .. },
                    ..
                }) => path,
                Some(_) => return Err(ENOTDIR),
                None => return Err(EBADF),
            }
        };
        let absolute = normalize(base, path)?;
        let mount = self
            .mounts
            .iter()
            .find(|mount| {
                mount.path == "/"
                    || absolute == mount.path
                    || absolute
                        .strip_prefix(&mount.path)
                        .is_some_and(|tail| tail.starts_with('/'))
            })
            .ok_or(ENOENT)?;
        let relative = if mount.path == "/" {
            absolute.trim_start_matches('/').to_string()
        } else {
            absolute
                .strip_prefix(&mount.path)
                .unwrap()
                .trim_start_matches('/')
                .to_string()
        };
        Ok((
            absolute,
            mount,
            if relative.is_empty() {
                ".".into()
            } else {
                relative
            },
        ))
    }

    fn allocate(&mut self, fd: Fd, minimum: usize) -> Result<i32, i64> {
        for index in minimum..self.fds.len() {
            if self.fds[index].is_none() {
                self.fds[index] = Some(fd);
                return Ok(index as i32);
            }
        }
        if self.fds.len() >= MAX_FDS {
            return Err(EMFILE);
        }
        while self.fds.len() < minimum {
            self.fds.push(None);
        }
        self.fds.push(Some(fd));
        Ok((self.fds.len() - 1) as i32)
    }

    pub fn open(&mut self, dirfd: i32, path: &str, linux_flags: u32) -> Result<i32, i64> {
        if path == "/dev/null" {
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Null,
                    flags: linux_flags,
                },
                0,
            );
        }
        if path == "/dev/zero" {
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Zero,
                    flags: linux_flags,
                },
                0,
            );
        }
        let (absolute, mount, relative) = self.resolve(dirfd, path)?;
        let access = linux_flags & 3;
        let mut flags = if access == 0 {
            OpenFlags::RIGHT_READABLE.0
        } else if access == 1 {
            OpenFlags::RIGHT_WRITABLE.0
        } else {
            OpenFlags::RIGHT_READABLE.0 | OpenFlags::RIGHT_WRITABLE.0
        };
        if linux_flags & 0x40 != 0 {
            flags |= OpenFlags::CREATE.0;
        }
        if linux_flags & 0x200 != 0 {
            flags |= OpenFlags::TRUNCATE.0;
        }
        let directory = linux_flags & 0x1_0000 != 0;
        if directory {
            flags |= OpenFlags::DIRECTORY.0;
        }
        if mount.readonly
            && flags & (OpenFlags::RIGHT_WRITABLE.0 | OpenFlags::CREATE.0 | OpenFlags::TRUNCATE.0)
                != 0
        {
            return Err(EROFS);
        }
        let channel = fs::open(mount.directory, &relative, flags).map_err(errno)?;
        self.allocate(
            Fd {
                descriptor: if directory {
                    Descriptor::Directory {
                        channel,
                        path: absolute,
                    }
                } else {
                    Descriptor::File {
                        channel,
                        path: absolute,
                    }
                },
                flags: linux_flags,
            },
            0,
        )
    }

    pub fn close(&mut self, fd: i32) -> Result<(), i64> {
        let entry = self
            .fds
            .get_mut(usize::try_from(fd).map_err(|_| EBADF)?)
            .and_then(Option::take)
            .ok_or(EBADF)?;
        match entry.descriptor {
            Descriptor::Stdio {
                kind: StdioKind::Socket(raw),
                ..
            } => Memory::close(raw).map_err(|_| EIO),
            Descriptor::File { channel, .. } | Descriptor::Directory { channel, .. } => {
                fs::close(channel).map_err(errno)
            }
            _ => Ok(()),
        }
    }

    pub fn close_range(&mut self, first: u32, last: u32) -> Result<(), i64> {
        if first > last {
            return Err(EINVAL);
        }
        for fd in first..=last.min((MAX_FDS - 1) as u32) {
            let _ = self.close(fd as i32);
        }
        Ok(())
    }

    pub fn duplicate(&mut self, fd: i32, minimum: usize, exact: Option<usize>) -> Result<i32, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let descriptor = match &entry.descriptor {
            Descriptor::Stdio {
                kind: StdioKind::Console,
                source,
            } => Descriptor::Stdio {
                kind: StdioKind::Console,
                source: *source,
            },
            Descriptor::Stdio {
                kind: StdioKind::Socket(raw),
                source,
            } => {
                let (_, rights) = Memory::object_info(*raw).map_err(|_| EBADF)?;
                Descriptor::Stdio {
                    kind: StdioKind::Socket(Memory::duplicate(*raw, rights).map_err(|_| EBADF)?),
                    source: *source,
                }
            }
            Descriptor::File { channel, path } => {
                let (_, rights) = Memory::object_info(channel.0).map_err(|_| EBADF)?;
                Descriptor::File {
                    channel: Channel(Memory::duplicate(channel.0, rights).map_err(|_| EBADF)?),
                    path: path.clone(),
                }
            }
            Descriptor::Directory { channel, path } => {
                let (_, rights) = Memory::object_info(channel.0).map_err(|_| EBADF)?;
                Descriptor::Directory {
                    channel: Channel(Memory::duplicate(channel.0, rights).map_err(|_| EBADF)?),
                    path: path.clone(),
                }
            }
            Descriptor::Null => Descriptor::Null,
            Descriptor::Zero => Descriptor::Zero,
        };
        let copy = Fd {
            descriptor,
            flags: entry.flags,
        };
        if let Some(target) = exact {
            if target >= MAX_FDS {
                return Err(EBADF);
            }
            if target == fd as usize {
                return Ok(fd);
            }
            while self.fds.len() <= target {
                self.fds.push(None);
            }
            let _ = self.close(target as i32);
            self.fds[target] = Some(copy);
            Ok(target as i32)
        } else {
            self.allocate(copy, minimum)
        }
    }

    pub fn flags(&self, fd: i32) -> Result<u32, i64> {
        self.fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .map(|fd| fd.flags)
            .ok_or(EBADF)
    }

    pub fn set_flags(&mut self, fd: i32, flags: u32) -> Result<(), i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        entry.flags = (entry.flags & !0x840) | (flags & 0x840);
        Ok(())
    }

    pub fn read(&mut self, fd: i32, count: usize) -> Result<Vec<u8>, i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        match &mut entry.descriptor {
            Descriptor::Stdio {
                kind: StdioKind::Socket(raw),
                ..
            } => {
                let socket = Socket(*raw);
                loop {
                    match socket.read(count.min(32768) as u32) {
                        Ok(bytes) => return Ok(bytes),
                        Err(kernel_fidl::Status::ErrTimedOut) => {
                            socket.wait_io(false, -1).map_err(|_| EIO)?;
                        }
                        Err(kernel_fidl::Status::ErrPeerClosed) => return Ok(Vec::new()),
                        Err(_) => return Err(EIO),
                    }
                }
            }
            Descriptor::Stdio {
                kind: StdioKind::Console,
                ..
            } => Ok(Vec::new()),
            Descriptor::File { channel, .. } => fs::read(*channel, count as u64).map_err(errno),
            Descriptor::Directory { .. } => Err(EISDIR),
            Descriptor::Null => Ok(Vec::new()),
            Descriptor::Zero => Ok(vec![0; count]),
        }
    }

    pub fn write(&mut self, fd: i32, bytes: &[u8]) -> Result<usize, i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        match &mut entry.descriptor {
            Descriptor::Stdio {
                kind: StdioKind::Socket(raw),
                ..
            } => Socket(*raw)
                .write(bytes)
                .map(|n| n as usize)
                .map_err(|_| EIO),
            Descriptor::Stdio {
                kind: StdioKind::Console,
                ..
            } => {
                bexos_userspace::log(&String::from_utf8_lossy(bytes));
                Ok(bytes.len())
            }
            Descriptor::File { channel, .. } => {
                if entry.flags & 0x400 != 0 {
                    fs::seek_with_whence(*channel, 0, 2).map_err(errno)?;
                }
                fs::write(*channel, bytes).map_err(errno)?;
                Ok(bytes.len())
            }
            Descriptor::Directory { .. } => Err(EISDIR),
            Descriptor::Null | Descriptor::Zero => Ok(bytes.len()),
        }
    }

    pub fn seek(&mut self, fd: i32, offset: i64, whence: u8) -> Result<u64, i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        match &mut entry.descriptor {
            Descriptor::File { channel, .. } => {
                fs::seek_with_whence(*channel, offset, whence).map_err(errno)
            }
            _ => Err(starnix_kernel::ESPIPE),
        }
    }

    pub fn read_at(&mut self, fd: i32, offset: u64, count: usize) -> Result<Vec<u8>, i64> {
        let original = self.seek(fd, 0, 1)?;
        self.seek(fd, offset as i64, 0)?;
        let result = self.read(fd, count);
        let _ = self.seek(fd, original as i64, 0);
        result
    }

    pub fn write_at(&mut self, fd: i32, offset: u64, bytes: &[u8]) -> Result<usize, i64> {
        let original = self.seek(fd, 0, 1)?;
        self.seek(fd, offset as i64, 0)?;
        let result = self.write(fd, bytes);
        let _ = self.seek(fd, original as i64, 0);
        result
    }

    pub fn sync(&mut self, fd: i32) -> Result<(), i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        match &mut entry.descriptor {
            Descriptor::File { channel, .. } => fs::sync_file(*channel).map_err(errno),
            _ => Err(EINVAL),
        }
    }

    pub fn truncate_fd(&mut self, fd: i32, length: u64) -> Result<(), i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        match &mut entry.descriptor {
            Descriptor::File { channel, .. } => fs::set_len(*channel, length).map_err(errno),
            Descriptor::Directory { .. } => Err(EISDIR),
            _ => Err(EINVAL),
        }
    }

    pub fn truncate_path(&mut self, path: &str, length: u64) -> Result<(), i64> {
        let fd = self.open(AT_FDCWD, path, 2)?;
        let result = self.truncate_fd(fd, length);
        let _ = self.close(fd);
        result
    }

    pub fn stat_fd(&self, fd: i32) -> Result<Stat, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        match &entry.descriptor {
            Descriptor::File { channel, path } => fs::attributes(*channel)
                .map(|attr| stat(path, attr, NodeKind::File))
                .map_err(errno),
            Descriptor::Directory { channel, path } => fs::attributes(*channel)
                .map(|attr| stat(path, attr, NodeKind::Directory))
                .map_err(errno),
            Descriptor::Stdio { .. } => Ok(device_stat(0o20_666)),
            Descriptor::Null | Descriptor::Zero => Ok(device_stat(0o20_666)),
        }
    }

    pub fn stat_path(&self, dirfd: i32, path: &str) -> Result<Stat, i64> {
        if matches!(path, "/dev/null" | "/dev/zero") {
            return Ok(device_stat(0o20_666));
        }
        let (absolute, mount, relative) = self.resolve(dirfd, path)?;
        let file = fs::open(mount.directory, &relative, OpenFlags::RIGHT_READABLE.0)
            .or_else(|_| {
                fs::open(
                    mount.directory,
                    &relative,
                    OpenFlags::RIGHT_READABLE.0 | OpenFlags::DIRECTORY.0,
                )
            })
            .map_err(errno)?;
        let attr = fs::attributes(file).map_err(errno)?;
        let kind = if fs::read_entries(file).is_ok() {
            NodeKind::Directory
        } else {
            NodeKind::File
        };
        let _ = fs::close(file);
        Ok(stat(&absolute, attr, kind))
    }

    pub fn entries(&self, fd: i32) -> Result<Vec<DirectoryEntry>, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::Directory { channel, path } = &entry.descriptor else {
            return Err(ENOTDIR);
        };
        let entries = fs::read_entries(*channel).map_err(errno)?;
        let mut output = vec![
            DirectoryEntry {
                inode: path_hash(path),
                name: ".".into(),
                kind: 4,
            },
            DirectoryEntry {
                inode: path_hash(path.rsplit_once('/').map_or("/", |(parent, _)| {
                    if parent.is_empty() { "/" } else { parent }
                })),
                name: "..".into(),
                kind: 4,
            },
        ];
        output.extend(entries.into_iter().map(|entry| DirectoryEntry {
            inode: path_hash(&format!("{path}/{}", entry.name)),
            name: entry.name,
            kind: if entry.kind == NodeKind::Directory {
                4
            } else {
                8
            },
        }));
        Ok(output)
    }

    pub fn unlink(&self, dirfd: i32, path: &str, directory: bool) -> Result<(), i64> {
        let (absolute, mount, _) = self.resolve(dirfd, path)?;
        if mount.readonly {
            return Err(EROFS);
        }
        let (parent, leaf) = absolute.rsplit_once('/').ok_or(EINVAL)?;
        if leaf.is_empty() {
            return Err(EINVAL);
        }
        let (_, parent_mount, relative) =
            self.resolve(AT_FDCWD, if parent.is_empty() { "/" } else { parent })?;
        let parent = fs::open(
            parent_mount.directory,
            &relative,
            OpenFlags::RIGHT_WRITABLE.0 | OpenFlags::DIRECTORY.0,
        )
        .map_err(errno)?;
        if directory {
            let target = fs::open(
                parent,
                leaf,
                OpenFlags::RIGHT_READABLE.0 | OpenFlags::DIRECTORY.0,
            )
            .map_err(errno)?;
            if !fs::read_entries(target).map_err(errno)?.is_empty() {
                let _ = fs::close(target);
                let _ = fs::close(parent);
                return Err(ENOTEMPTY);
            }
            let _ = fs::close(target);
        }
        let result = fs::unlink(parent, leaf).map_err(errno);
        let _ = fs::close(parent);
        result
    }

    pub fn mkdir(&self, dirfd: i32, path: &str) -> Result<(), i64> {
        let (_, mount, relative) = self.resolve(dirfd, path)?;
        if mount.readonly {
            return Err(EROFS);
        }
        let directory = fs::open(
            mount.directory,
            &relative,
            OpenFlags::RIGHT_READABLE.0
                | OpenFlags::RIGHT_WRITABLE.0
                | OpenFlags::CREATE.0
                | OpenFlags::DIRECTORY.0,
        )
        .map_err(errno)?;
        fs::close(directory).map_err(errno)
    }

    pub fn chdir(&mut self, path: &str) -> Result<(), i64> {
        let (absolute, mount, relative) = self.resolve(AT_FDCWD, path)?;
        let directory = fs::open(
            mount.directory,
            &relative,
            OpenFlags::RIGHT_READABLE.0 | OpenFlags::DIRECTORY.0,
        )
        .map_err(errno)?;
        let _ = fs::close(directory);
        self.cwd = absolute;
        Ok(())
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn backing(&self, fd: i32) -> Result<(u64, u64), i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        match &entry.descriptor {
            Descriptor::File { channel, .. } => fs::backing(*channel).map_err(errno),
            _ => Err(EBADF),
        }
    }

    pub fn snapshot(&mut self) -> Result<Snapshot, i64> {
        let mut descriptors = Vec::new();
        for index in 0..self.fds.len() {
            let Some(entry) = self.fds[index].as_ref() else {
                continue;
            };
            let descriptor = match &entry.descriptor {
                Descriptor::Stdio { source, .. } => DescriptorSnapshot::Stdio { source: *source },
                Descriptor::File { channel, path } => DescriptorSnapshot::File {
                    path: path.clone(),
                    flags: entry.flags,
                    offset: fs::seek_with_whence(*channel, 0, 1).map_err(errno)?,
                },
                Descriptor::Directory { path, .. } => DescriptorSnapshot::Directory {
                    path: path.clone(),
                    flags: entry.flags,
                },
                Descriptor::Null => DescriptorSnapshot::Null { flags: entry.flags },
                Descriptor::Zero => DescriptorSnapshot::Zero { flags: entry.flags },
            };
            descriptors.push((index as u16, descriptor));
        }
        Ok(Snapshot {
            cwd: self.cwd.clone(),
            descriptors,
        })
    }

    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<(), i64> {
        for fd in 3..self.fds.len() {
            let _ = self.close(fd as i32);
        }
        self.chdir(&snapshot.cwd)?;
        for (target, descriptor) in &snapshot.descriptors {
            let target = usize::from(*target);
            if target < 3 {
                continue;
            }
            let temporary = match descriptor {
                DescriptorSnapshot::Stdio { source } => {
                    self.duplicate(i32::from(*source), 0, None)?
                }
                DescriptorSnapshot::File {
                    path,
                    flags,
                    offset,
                } => {
                    let fd = self.open(AT_FDCWD, path, *flags & !(0x40 | 0x80 | 0x200))?;
                    self.seek(fd, *offset as i64, 0)?;
                    fd
                }
                DescriptorSnapshot::Directory { path, flags } => {
                    self.open(AT_FDCWD, path, (*flags & !(0x40 | 0x80 | 0x200)) | 0x1_0000)?
                }
                DescriptorSnapshot::Null { flags } => self.open(AT_FDCWD, "/dev/null", *flags)?,
                DescriptorSnapshot::Zero { flags } => self.open(AT_FDCWD, "/dev/zero", *flags)?,
            };
            if temporary as usize != target {
                self.duplicate(temporary, 0, Some(target))?;
                self.close(temporary)?;
            }
        }
        Ok(())
    }
}

fn path_hash(path: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash.max(1)
}

fn stat(path: &str, attr: FileAttributes, kind: NodeKind) -> Stat {
    Stat {
        inode: path_hash(path),
        mode: attr.mode
            | if kind == NodeKind::Directory {
                0o040000
            } else {
                0o100000
            },
        links: 1,
        size: attr.size_bytes,
        blocks: attr.storage_allocated_bytes.div_ceil(512),
        created: attr.creation_time_nanos,
        modified: attr.modification_time_nanos,
        kind: if kind == NodeKind::Directory { 4 } else { 8 },
    }
}

fn device_stat(mode: u32) -> Stat {
    Stat {
        inode: 1,
        mode,
        links: 1,
        size: 0,
        blocks: 0,
        created: 0,
        modified: 0,
        kind: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_root_bounded_and_canonical() {
        assert_eq!(normalize("/data/a", "../b").unwrap(), "/data/b");
        assert_eq!(normalize("/", "../../pkg/bin").unwrap(), "/pkg/bin");
        assert_eq!(normalize("/tmp", "/data//x/./y").unwrap(), "/data/x/y");
    }

    #[test]
    fn path_inodes_are_stable_and_distinct() {
        assert_eq!(path_hash("/data/a"), path_hash("/data/a"));
        assert_ne!(path_hash("/data/a"), path_hash("/data/b"));
    }
}
