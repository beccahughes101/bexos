use crate::events::{
    EPOLL_ALLOWED, EPOLLERR, EPOLLHUP, EPOLLONESHOT, EpollInterest, EpollState, EventFdState,
    TimerFdState,
};
use bexos_userspace::{Channel, Memory, Socket, fs};
use fs_fidl::{FileAttributes, FsStatus, NodeKind, OpenFlags};
use starnix_kernel::{
    EACCES, EADDRINUSE, EBADF, ECONNREFUSED, EEXIST, EINVAL, EIO, EISCONN, EISDIR, EMFILE, ENODATA,
    ENOENT, ENOSPC, ENOTCONN, ENOTDIR, ENOTEMPTY, ENOTSUP, EROFS,
};
use std::{
    collections::BTreeMap,
    string::String,
    sync::{Arc, Mutex},
    vec::Vec,
};

pub const AT_FDCWD: i32 = -100;
const MAX_FDS: usize = 256;
const PATH_MAX: usize = 4096;
const O_APPEND: u32 = 0x400;
const O_NONBLOCK: u32 = 0x800;
const O_CLOEXEC: u32 = 0x8_0000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stat {
    pub inode: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
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
    Socket {
        handle: u64,
        flags: u32,
        readable: bool,
        writable: bool,
        local: String,
        peer: String,
    },
    UnixEndpoint {
        flags: u32,
        bound: String,
        listening: bool,
        pending: Vec<(u64, String)>,
    },
    Synthetic {
        path: String,
        flags: u32,
        data: Vec<u8>,
        offset: u64,
        directory: bool,
    },
    EventFd {
        flags: u32,
        counter: u64,
        semaphore: bool,
    },
    Epoll {
        flags: u32,
        interests: Vec<(i32, u32, u64, bool)>,
    },
    TimerFd {
        flags: u32,
        clock_id: i32,
        remaining: u64,
        interval: u64,
        armed: bool,
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
    Stdio {
        kind: StdioKind,
        source: u8,
    },
    File {
        channel: Channel,
        path: String,
    },
    Directory {
        channel: Channel,
        path: String,
    },
    Null,
    Zero,
    Socket {
        handle: u64,
        readable: bool,
        writable: bool,
        local: String,
        peer: String,
    },
    UnixEndpoint(Arc<Mutex<UnixEndpoint>>),
    Synthetic {
        path: String,
        data: Vec<u8>,
        offset: usize,
        directory: bool,
    },
    Random,
    Full,
    EventFd(Arc<Mutex<EventFdState>>),
    Epoll(Arc<Mutex<EpollState>>),
    TimerFd(Arc<Mutex<TimerFdState>>),
}

#[derive(Default)]
struct UnixEndpoint {
    bound: String,
    listening: bool,
    backlog: usize,
    pending: Vec<(u64, String)>,
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
    executable: String,
    synthetic_overrides: BTreeMap<String, Vec<u8>>,
    synthetic_directory_overrides: BTreeMap<String, Vec<String>>,
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
                }
                | Descriptor::Socket { handle: raw, .. } => close_once(raw, &mut closed),
                Descriptor::UnixEndpoint(endpoint) if Arc::strong_count(&endpoint) == 1 => {
                    if let Ok(endpoint) = endpoint.lock() {
                        for (handle, _) in &endpoint.pending {
                            close_once(*handle, &mut closed);
                        }
                    }
                }
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

fn clone_descriptor(descriptor: &Descriptor) -> Result<Descriptor, i64> {
    Ok(match descriptor {
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
        Descriptor::Socket {
            handle,
            readable,
            writable,
            local,
            peer,
        } => {
            let (_, rights) = Memory::object_info(*handle).map_err(|_| EBADF)?;
            Descriptor::Socket {
                handle: Memory::duplicate(*handle, rights).map_err(|_| EBADF)?,
                readable: *readable,
                writable: *writable,
                local: local.clone(),
                peer: peer.clone(),
            }
        }
        Descriptor::UnixEndpoint(endpoint) => Descriptor::UnixEndpoint(endpoint.clone()),
        Descriptor::Synthetic {
            path,
            data,
            offset,
            directory,
        } => Descriptor::Synthetic {
            path: path.clone(),
            data: data.clone(),
            offset: *offset,
            directory: *directory,
        },
        Descriptor::Random => Descriptor::Random,
        Descriptor::Full => Descriptor::Full,
        Descriptor::EventFd(state) => Descriptor::EventFd(state.clone()),
        Descriptor::Epoll(state) => Descriptor::Epoll(state.clone()),
        Descriptor::TimerFd(state) => Descriptor::TimerFd(state.clone()),
    })
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

fn xattr_errno(status: FsStatus) -> i64 {
    if status == FsStatus::NotFound {
        ENODATA
    } else {
        errno(status)
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
        root_readonly: bool,
        stdio: [StdioKind; 3],
        command_cwd: bool,
        executable: &str,
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
            readonly: root_source == "/pkg" || root_readonly,
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
            executable: executable.into(),
            synthetic_overrides: BTreeMap::new(),
            synthetic_directory_overrides: BTreeMap::new(),
        })
    }

    pub fn set_synthetic(&mut self, path: &str, bytes: Vec<u8>) {
        self.synthetic_overrides.insert(path.into(), bytes);
    }

    pub fn set_synthetic_directory(&mut self, path: &str, entries: Vec<String>) {
        self.synthetic_directory_overrides
            .insert(path.into(), entries);
    }

    fn synthetic_directory(&self, path: &str) -> Option<Vec<String>> {
        self.synthetic_directory_overrides
            .get(path)
            .cloned()
            .or_else(|| synthetic_directory(path))
    }

    pub fn fork_clone(&self) -> Result<Self, i64> {
        let mut copy = Self {
            mounts: Vec::with_capacity(self.mounts.len()),
            fds: Vec::with_capacity(self.fds.len()),
            cwd: self.cwd.clone(),
            executable: self.executable.clone(),
            synthetic_overrides: self.synthetic_overrides.clone(),
            synthetic_directory_overrides: self.synthetic_directory_overrides.clone(),
        };
        for mount in &self.mounts {
            let (_, rights) = Memory::object_info(mount.directory.0).map_err(|_| EBADF)?;
            copy.mounts.push(Mount {
                path: mount.path.clone(),
                directory: Channel(
                    Memory::duplicate(mount.directory.0, rights).map_err(|_| EBADF)?,
                ),
                readonly: mount.readonly,
            });
        }
        for entry in &self.fds {
            copy.fds.push(
                entry
                    .as_ref()
                    .map(|fd| {
                        Ok::<Fd, i64>(Fd {
                            descriptor: clone_descriptor(&fd.descriptor)?,
                            flags: fd.flags,
                        })
                    })
                    .transpose()?,
            );
        }
        Ok(copy)
    }

    fn absolute(&self, dirfd: i32, path: &str) -> Result<String, i64> {
        let base = if path.starts_with('/') || dirfd == AT_FDCWD {
            self.cwd.as_str()
        } else {
            match self.fds.get(dirfd as usize).and_then(Option::as_ref) {
                Some(Fd {
                    descriptor: Descriptor::Directory { path, .. },
                    ..
                })
                | Some(Fd {
                    descriptor:
                        Descriptor::Synthetic {
                            path,
                            directory: true,
                            ..
                        },
                    ..
                }) => path,
                Some(_) => return Err(ENOTDIR),
                None => return Err(EBADF),
            }
        };
        normalize(base, path)
    }

    fn synthetic(&self, path: &str) -> Option<Vec<u8>> {
        self.synthetic_overrides
            .get(path)
            .cloned()
            .or_else(|| synthetic_file(path))
    }

    fn descriptor_path(&self, fd: usize) -> Result<String, i64> {
        let entry = self.fds.get(fd).and_then(Option::as_ref).ok_or(ENOENT)?;
        Ok(match &entry.descriptor {
            Descriptor::File { path, .. }
            | Descriptor::Directory { path, .. }
            | Descriptor::Synthetic { path, .. } => path.clone(),
            Descriptor::Stdio { source, .. } => format!("/dev/fd/{source}"),
            Descriptor::Null => "/dev/null".into(),
            Descriptor::Zero => "/dev/zero".into(),
            Descriptor::Random => "/dev/urandom".into(),
            Descriptor::Full => "/dev/full".into(),
            Descriptor::Socket { handle, .. } => format!("socket:[{handle}]"),
            Descriptor::UnixEndpoint(endpoint) => {
                let endpoint = endpoint.lock().map_err(|_| EIO)?;
                if endpoint.bound.is_empty() {
                    "socket:[unbound]".into()
                } else {
                    endpoint.bound.clone()
                }
            }
            Descriptor::EventFd(_) => "anon_inode:[eventfd]".into(),
            Descriptor::Epoll(_) => "anon_inode:[eventpoll]".into(),
            Descriptor::TimerFd(_) => "anon_inode:[timerfd]".into(),
        })
    }

    fn resolve(&self, dirfd: i32, path: &str) -> Result<(String, &Mount, String), i64> {
        let absolute = self.absolute(dirfd, path)?;
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
        if minimum >= MAX_FDS {
            return Err(EMFILE);
        }
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
        let absolute = self.absolute(dirfd, path)?;
        if absolute == "/dev/null" {
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Null,
                    flags: linux_flags,
                },
                0,
            );
        }
        if absolute == "/dev/zero" {
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Zero,
                    flags: linux_flags,
                },
                0,
            );
        }
        if matches!(absolute.as_str(), "/dev/random" | "/dev/urandom") {
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Random,
                    flags: linux_flags,
                },
                0,
            );
        }
        if absolute == "/dev/full" {
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Full,
                    flags: linux_flags,
                },
                0,
            );
        }
        if absolute == "/dev/tty" {
            return self.duplicate(0, 0, None, false);
        }
        if let Some(directory) = self.synthetic_directory(&absolute) {
            if linux_flags & 3 != 0 {
                return Err(EROFS);
            }
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Synthetic {
                        path: absolute,
                        data: directory.join("\n").into_bytes(),
                        offset: 0,
                        directory: true,
                    },
                    flags: linux_flags,
                },
                0,
            );
        }
        if let Some(data) = self.synthetic(&absolute) {
            if linux_flags & 3 != 0 {
                return Err(EROFS);
            }
            return self.allocate(
                Fd {
                    descriptor: Descriptor::Synthetic {
                        path: absolute,
                        data,
                        offset: 0,
                        directory: false,
                    },
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
        if linux_flags & 0x2_0000 != 0 {
            flags |= OpenFlags::NO_FOLLOW.0;
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
            Descriptor::Socket { handle, .. } => Memory::close(handle).map_err(|_| EIO),
            Descriptor::UnixEndpoint(endpoint) if Arc::strong_count(&endpoint) == 1 => {
                let mut endpoint = endpoint.lock().map_err(|_| EIO)?;
                for (handle, _) in endpoint.pending.drain(..) {
                    let _ = Memory::close(handle);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn close_range(&mut self, first: u32, last: u32, flags: u32) -> Result<(), i64> {
        if first > last {
            return Err(EINVAL);
        }
        if flags & !(2 | 4) != 0 {
            return Err(EINVAL);
        }
        for fd in first..=last.min((MAX_FDS - 1) as u32) {
            if flags & 4 != 0 {
                if let Some(entry) = self.fds.get_mut(fd as usize).and_then(Option::as_mut) {
                    entry.flags |= O_CLOEXEC;
                }
            } else {
                let _ = self.close(fd as i32);
            }
        }
        Ok(())
    }

    pub fn duplicate(
        &mut self,
        fd: i32,
        minimum: usize,
        exact: Option<usize>,
        cloexec: bool,
    ) -> Result<i32, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        if exact == Some(fd as usize) {
            return Ok(fd);
        }
        let descriptor = clone_descriptor(&entry.descriptor)?;
        let copy = Fd {
            descriptor,
            flags: (entry.flags & !O_CLOEXEC) | if cloexec { O_CLOEXEC } else { 0 },
        };
        if let Some(target) = exact {
            if target >= MAX_FDS {
                return Err(EBADF);
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

    pub fn fd_flags(&self, fd: i32) -> Result<u32, i64> {
        Ok(u32::from(self.flags(fd)? & O_CLOEXEC != 0))
    }

    pub fn set_fd_flags(&mut self, fd: i32, flags: u32) -> Result<(), i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        entry.flags = (entry.flags & !O_CLOEXEC) | if flags & 1 != 0 { O_CLOEXEC } else { 0 };
        Ok(())
    }

    pub fn set_flags(&mut self, fd: i32, flags: u32) -> Result<(), i64> {
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        entry.flags = (entry.flags & !(O_APPEND | O_NONBLOCK)) | (flags & (O_APPEND | O_NONBLOCK));
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
            Descriptor::Socket {
                handle, readable, ..
            } => {
                if !*readable {
                    return Err(EBADF);
                }
                let socket = Socket(*handle);
                loop {
                    match socket.read(count.min(65_536) as u32) {
                        Ok(bytes) => return Ok(bytes),
                        Err(kernel_fidl::Status::ErrTimedOut) if entry.flags & 0x800 != 0 => {
                            return Err(starnix_kernel::EAGAIN);
                        }
                        Err(kernel_fidl::Status::ErrTimedOut) => {
                            socket.wait_io(false, -1).map_err(|_| EIO)?;
                        }
                        Err(kernel_fidl::Status::ErrPeerClosed) => return Ok(Vec::new()),
                        Err(_) => return Err(EIO),
                    }
                }
            }
            Descriptor::UnixEndpoint(_) => Err(ENOTCONN),
            Descriptor::Synthetic {
                data,
                offset,
                directory,
                ..
            } => {
                if *directory {
                    return Err(EISDIR);
                }
                let start = (*offset).min(data.len());
                let end = start.saturating_add(count).min(data.len());
                *offset = end;
                Ok(data[start..end].to_vec())
            }
            Descriptor::Random => {
                let mut bytes = vec![0; count];
                let mut value = bexos_userspace::syscall::ticks() ^ (fd as u64).rotate_left(17);
                for byte in &mut bytes {
                    value ^= value << 13;
                    value ^= value >> 7;
                    value ^= value << 17;
                    *byte = value as u8;
                }
                Ok(bytes)
            }
            Descriptor::Full => Ok(vec![0; count]),
            Descriptor::EventFd(state) => {
                if count < 8 {
                    return Err(EINVAL);
                }
                loop {
                    match state.lock().map_err(|_| EIO)?.read() {
                        Ok(value) => return Ok(value.to_ne_bytes().to_vec()),
                        Err(starnix_kernel::EAGAIN) if entry.flags & 0x800 == 0 => {
                            bexos_userspace::yield_now();
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            Descriptor::Epoll(_) => Err(EINVAL),
            Descriptor::TimerFd(state) => {
                if count < 8 {
                    return Err(EINVAL);
                }
                loop {
                    let expirations = state.lock().map_err(|_| EIO)?.expirations()?;
                    if expirations != 0 {
                        return Ok(expirations.to_ne_bytes().to_vec());
                    }
                    if entry.flags & 0x800 != 0 {
                        return Err(starnix_kernel::EAGAIN);
                    }
                    bexos_userspace::yield_now();
                }
            }
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
            Descriptor::Socket {
                handle, writable, ..
            } => {
                if !*writable {
                    return Err(EBADF);
                }
                Socket(*handle)
                    .write(bytes)
                    .map(|count| count as usize)
                    .map_err(|status| match status {
                        kernel_fidl::Status::ErrPeerClosed => starnix_kernel::EPIPE,
                        kernel_fidl::Status::ErrNoMemory if entry.flags & 0x800 != 0 => {
                            starnix_kernel::EAGAIN
                        }
                        _ => EIO,
                    })
            }
            Descriptor::UnixEndpoint(_) => Err(ENOTCONN),
            Descriptor::Synthetic { .. } | Descriptor::Random => Err(EROFS),
            Descriptor::Full => Err(ENOSPC),
            Descriptor::EventFd(state) => {
                if bytes.len() != 8 {
                    return Err(EINVAL);
                }
                let value = u64::from_ne_bytes(bytes.try_into().unwrap());
                loop {
                    if state.lock().map_err(|_| EIO)?.try_write(value)? {
                        return Ok(8);
                    }
                    if entry.flags & 0x800 != 0 {
                        return Err(starnix_kernel::EAGAIN);
                    }
                    bexos_userspace::yield_now();
                }
            }
            Descriptor::Epoll(_) | Descriptor::TimerFd(_) => Err(EINVAL),
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
            Descriptor::Synthetic {
                data,
                offset: cursor,
                directory,
                ..
            } => {
                if *directory {
                    return Err(EISDIR);
                }
                let base = match whence {
                    0 => 0i128,
                    1 => *cursor as i128,
                    2 => data.len() as i128,
                    _ => return Err(EINVAL),
                };
                let next = base + i128::from(offset);
                if !(0..=data.len() as i128).contains(&next) {
                    return Err(EINVAL);
                }
                *cursor = next as usize;
                Ok(*cursor as u64)
            }
            _ => Err(starnix_kernel::ESPIPE),
        }
    }

    pub fn read_at(&mut self, fd: i32, offset: u64, count: usize) -> Result<Vec<u8>, i64> {
        let offset = i64::try_from(offset).map_err(|_| EINVAL)?;
        let original = self.seek(fd, 0, 1)?;
        self.seek(fd, offset, 0)?;
        let result = self.read(fd, count);
        let _ = self.seek(fd, original as i64, 0);
        result
    }

    pub fn write_at(&mut self, fd: i32, offset: u64, bytes: &[u8]) -> Result<usize, i64> {
        let offset = i64::try_from(offset).map_err(|_| EINVAL)?;
        let original = self.seek(fd, 0, 1)?;
        self.seek(fd, offset, 0)?;
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

    /// Read a bounded regular file without exposing a temporary descriptor to
    /// the Linux task. This is used by the ELF loader for PT_INTERP and is also
    /// the common primitive for execve once task replacement is enabled.
    pub fn read_file(&mut self, path: &str, maximum: usize) -> Result<Vec<u8>, i64> {
        self.read_file_at(AT_FDCWD, path, maximum)
    }

    pub fn read_file_at(&mut self, dirfd: i32, path: &str, maximum: usize) -> Result<Vec<u8>, i64> {
        let fd = self.open(dirfd, path, 0)?;
        let result = (|| {
            let size = usize::try_from(self.stat_fd(fd)?.size).map_err(|_| EINVAL)?;
            if size == 0 || size > maximum {
                return Err(if size > maximum {
                    starnix_kernel::ENOMEM
                } else {
                    EINVAL
                });
            }
            self.read_at(fd, 0, size)
        })();
        let _ = self.close(fd);
        result
    }

    pub fn finish_exec(&mut self, executable: &str) {
        self.executable = normalize(&self.cwd, executable).unwrap_or_else(|_| executable.into());
        let close: Vec<_> = self
            .fds
            .iter()
            .enumerate()
            .filter_map(|(fd, entry)| {
                entry
                    .as_ref()
                    .is_some_and(|entry| entry.flags & 0x8_0000 != 0)
                    .then_some(fd as i32)
            })
            .collect();
        for fd in close {
            let _ = self.close(fd);
        }
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
            Descriptor::Socket { .. } => Ok(device_stat(0o140777)),
            Descriptor::UnixEndpoint(_) => Ok(device_stat(0o140777)),
            Descriptor::Synthetic {
                path,
                data,
                directory,
                ..
            } => Ok(synthetic_stat(path, data.len() as u64, *directory)),
            Descriptor::Random | Descriptor::Full => Ok(device_stat(0o20_666)),
            Descriptor::EventFd(_) | Descriptor::Epoll(_) | Descriptor::TimerFd(_) => {
                Ok(device_stat(0o100600))
            }
        }
    }

    pub fn stat_path(&self, dirfd: i32, path: &str) -> Result<Stat, i64> {
        let absolute = self.absolute(dirfd, path)?;
        if matches!(
            absolute.as_str(),
            "/dev/null" | "/dev/zero" | "/dev/random" | "/dev/urandom" | "/dev/full" | "/dev/tty"
        ) {
            return Ok(device_stat(0o20_666));
        }
        if let Some(entries) = self.synthetic_directory(&absolute) {
            return Ok(synthetic_stat(&absolute, entries.len() as u64, true));
        }
        if let Some(data) = self.synthetic(&absolute) {
            return Ok(synthetic_stat(&absolute, data.len() as u64, false));
        }
        let (_, mount, relative) = self.resolve(dirfd, path)?;
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

    pub fn lstat_path(&self, dirfd: i32, path: &str) -> Result<Stat, i64> {
        let absolute = self.absolute(dirfd, path)?;
        match self.readlink_storage(dirfd, path) {
            Ok(target) => {
                let (_, mount, relative) = self.resolve(dirfd, path)?;
                let node = fs::open(
                    mount.directory,
                    &relative,
                    OpenFlags::RIGHT_READABLE.0 | OpenFlags::NO_FOLLOW.0,
                )
                .map_err(errno)?;
                let attr = fs::attributes(node).map_err(errno)?;
                let _ = fs::close(node);
                let mut result = stat(&absolute, attr, NodeKind::Symlink);
                result.size = target.len() as u64;
                return Ok(result);
            }
            Err(EINVAL) => {}
            Err(error) => return Err(error),
        }
        self.stat_path(dirfd, path)
    }

    pub fn chmod_path(&self, dirfd: i32, path: &str, nofollow: bool, mode: u32) -> Result<(), i64> {
        let node = self.xattr_path_node(dirfd, path, nofollow, true)?;
        let result = update_attributes(node, |attr| {
            attr.mode = (attr.mode & !0o7777) | (mode & 0o7777);
        });
        let _ = fs::close(node);
        result
    }

    pub fn chmod_fd(&self, fd: i32, mode: u32) -> Result<(), i64> {
        update_attributes(self.xattr_fd_node(fd)?, |attr| {
            attr.mode = (attr.mode & !0o7777) | (mode & 0o7777);
        })
    }

    pub fn chown_path(
        &self,
        dirfd: i32,
        path: &str,
        nofollow: bool,
        uid: u32,
        gid: u32,
    ) -> Result<(), i64> {
        let node = self.xattr_path_node(dirfd, path, nofollow, true)?;
        let result = update_attributes(node, |attr| {
            if uid != u32::MAX {
                attr.uid = uid;
            }
            if gid != u32::MAX {
                attr.gid = gid;
            }
        });
        let _ = fs::close(node);
        result
    }

    pub fn chown_fd(&self, fd: i32, uid: u32, gid: u32) -> Result<(), i64> {
        update_attributes(self.xattr_fd_node(fd)?, |attr| {
            if uid != u32::MAX {
                attr.uid = uid;
            }
            if gid != u32::MAX {
                attr.gid = gid;
            }
        })
    }

    pub fn entries(&self, fd: i32) -> Result<Vec<DirectoryEntry>, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        if let Descriptor::Synthetic {
            path,
            directory: true,
            ..
        } = &entry.descriptor
        {
            let mut output = dot_entries(path);
            let mut names = self.synthetic_directory(path).ok_or(ENOTDIR)?;
            if path == "/proc/self/fd" {
                names.extend(
                    self.fds
                        .iter()
                        .enumerate()
                        .filter(|(_, descriptor)| descriptor.is_some())
                        .map(|(fd, _)| fd.to_string()),
                );
            }
            output.extend(names.into_iter().map(|name| {
                let child = format!("{}/{}", path.trim_end_matches('/'), name);
                DirectoryEntry {
                    inode: path_hash(&child),
                    name,
                    kind: if self.synthetic_directory(&child).is_some() {
                        4
                    } else {
                        8
                    },
                }
            }));
            return Ok(output);
        }
        let Descriptor::Directory { channel, path } = &entry.descriptor else {
            return Err(ENOTDIR);
        };
        let entries = fs::read_entries(*channel).map_err(errno)?;
        let mut output = dot_entries(path);
        output.extend(entries.into_iter().map(|entry| DirectoryEntry {
            inode: path_hash(&format!("{path}/{}", entry.name)),
            name: entry.name,
            kind: match entry.kind {
                NodeKind::Directory => 4,
                NodeKind::Symlink => 10,
                _ => 8,
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

    pub fn rename(
        &self,
        source_dirfd: i32,
        source: &str,
        target_dirfd: i32,
        target: &str,
    ) -> Result<(), i64> {
        let (_, source_mount, source_relative) = self.resolve(source_dirfd, source)?;
        let source_mount_path = source_mount.path.clone();
        let source_root = source_mount.directory;
        let readonly = source_mount.readonly;
        let (_, target_mount, target_relative) = self.resolve(target_dirfd, target)?;
        if source_mount_path != target_mount.path {
            return Err(starnix_kernel::EXDEV);
        }
        if readonly || target_mount.readonly {
            return Err(EROFS);
        }
        fs::rename(source_root, &source_relative, &target_relative).map_err(errno)
    }

    pub fn link(
        &self,
        source_dirfd: i32,
        source: &str,
        target_dirfd: i32,
        target: &str,
    ) -> Result<(), i64> {
        let (_, source_mount, source_relative) = self.resolve(source_dirfd, source)?;
        let source_mount_path = source_mount.path.clone();
        let source_root = source_mount.directory;
        let readonly = source_mount.readonly;
        let (_, target_mount, target_relative) = self.resolve(target_dirfd, target)?;
        if source_mount_path != target_mount.path {
            return Err(starnix_kernel::EXDEV);
        }
        if readonly || target_mount.readonly {
            return Err(EROFS);
        }
        fs::link(source_root, &source_relative, &target_relative).map_err(errno)
    }

    pub fn symlink(&self, target: &str, dirfd: i32, link_path: &str) -> Result<(), i64> {
        if target.is_empty() || target.len() > PATH_MAX || target.as_bytes().contains(&0) {
            return Err(if target.is_empty() { ENOENT } else { EINVAL });
        }
        let (_, mount, relative) = self.resolve(dirfd, link_path)?;
        if mount.readonly {
            return Err(EROFS);
        }
        fs::symlink(mount.directory, target, &relative).map_err(errno)
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

    pub fn chdir_fd(&mut self, fd: i32) -> Result<(), i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        self.cwd = match &entry.descriptor {
            Descriptor::Directory { path, .. }
            | Descriptor::Synthetic {
                path,
                directory: true,
                ..
            } => path.clone(),
            _ => return Err(ENOTDIR),
        };
        Ok(())
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn readlink(&self, dirfd: i32, path: &str) -> Result<Vec<u8>, i64> {
        let absolute = self.absolute(dirfd, path)?;
        if matches!(
            absolute.as_str(),
            "/proc/self/exe" | "/proc/1/exe" | "/proc/thread-self/exe"
        ) {
            return Ok(self.executable.as_bytes().to_vec());
        }
        if let Some(fd) = absolute
            .strip_prefix("/proc/self/fd/")
            .and_then(|fd| fd.parse::<usize>().ok())
        {
            return self.descriptor_path(fd).map(String::into_bytes);
        }
        self.readlink_storage(dirfd, path).map(String::into_bytes)
    }

    fn readlink_storage(&self, dirfd: i32, path: &str) -> Result<String, i64> {
        let (_, mount, relative) = self.resolve(dirfd, path)?;
        fs::readlink(mount.directory, &relative).map_err(errno)
    }

    fn xattr_path_node(
        &self,
        dirfd: i32,
        path: &str,
        nofollow: bool,
        writable: bool,
    ) -> Result<Channel, i64> {
        let (_, mount, relative) = self.resolve(dirfd, path)?;
        if writable && mount.readonly {
            return Err(EROFS);
        }
        let mut flags = OpenFlags::RIGHT_READABLE.0;
        if writable {
            flags |= OpenFlags::RIGHT_WRITABLE.0;
        }
        if nofollow {
            flags |= OpenFlags::NO_FOLLOW.0;
        }
        fs::open(mount.directory, &relative, flags).map_err(errno)
    }

    fn xattr_fd_node(&self, fd: i32) -> Result<Channel, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        match &entry.descriptor {
            Descriptor::File { channel, .. } | Descriptor::Directory { channel, .. } => {
                Ok(*channel)
            }
            _ => Err(ENOTSUP),
        }
    }

    pub fn get_xattr_path(
        &self,
        dirfd: i32,
        path: &str,
        nofollow: bool,
        name: &str,
    ) -> Result<Vec<u8>, i64> {
        let node = self.xattr_path_node(dirfd, path, nofollow, false)?;
        let result = fs::get_xattr(node, name).map_err(xattr_errno);
        let _ = fs::close(node);
        result
    }

    pub fn get_xattr_fd(&self, fd: i32, name: &str) -> Result<Vec<u8>, i64> {
        fs::get_xattr(self.xattr_fd_node(fd)?, name).map_err(xattr_errno)
    }

    pub fn set_xattr_path(
        &self,
        dirfd: i32,
        path: &str,
        nofollow: bool,
        name: &str,
        value: &[u8],
        flags: u32,
    ) -> Result<(), i64> {
        let node = self.xattr_path_node(dirfd, path, nofollow, true)?;
        let result = fs::set_xattr(node, name, value, flags).map_err(xattr_errno);
        let _ = fs::close(node);
        result
    }

    pub fn set_xattr_fd(&self, fd: i32, name: &str, value: &[u8], flags: u32) -> Result<(), i64> {
        fs::set_xattr(self.xattr_fd_node(fd)?, name, value, flags).map_err(xattr_errno)
    }

    pub fn list_xattrs_path(
        &self,
        dirfd: i32,
        path: &str,
        nofollow: bool,
    ) -> Result<Vec<String>, i64> {
        let node = self.xattr_path_node(dirfd, path, nofollow, false)?;
        let result = fs::list_xattrs(node).map_err(xattr_errno);
        let _ = fs::close(node);
        result
    }

    pub fn list_xattrs_fd(&self, fd: i32) -> Result<Vec<String>, i64> {
        fs::list_xattrs(self.xattr_fd_node(fd)?).map_err(xattr_errno)
    }

    pub fn remove_xattr_path(
        &self,
        dirfd: i32,
        path: &str,
        nofollow: bool,
        name: &str,
    ) -> Result<(), i64> {
        let node = self.xattr_path_node(dirfd, path, nofollow, true)?;
        let result = fs::remove_xattr(node, name).map_err(xattr_errno);
        let _ = fs::close(node);
        result
    }

    pub fn remove_xattr_fd(&self, fd: i32, name: &str) -> Result<(), i64> {
        fs::remove_xattr(self.xattr_fd_node(fd)?, name).map_err(xattr_errno)
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
                Descriptor::Socket {
                    handle,
                    readable,
                    writable,
                    local,
                    peer,
                } => DescriptorSnapshot::Socket {
                    handle: *handle,
                    flags: entry.flags,
                    readable: *readable,
                    writable: *writable,
                    local: local.clone(),
                    peer: peer.clone(),
                },
                Descriptor::UnixEndpoint(endpoint) => {
                    let endpoint = endpoint.lock().map_err(|_| EIO)?;
                    DescriptorSnapshot::UnixEndpoint {
                        flags: entry.flags,
                        bound: endpoint.bound.clone(),
                        listening: endpoint.listening,
                        pending: endpoint.pending.clone(),
                    }
                }
                Descriptor::Synthetic {
                    path,
                    data,
                    offset,
                    directory,
                } => DescriptorSnapshot::Synthetic {
                    path: path.clone(),
                    flags: entry.flags,
                    data: data.clone(),
                    offset: *offset as u64,
                    directory: *directory,
                },
                Descriptor::Random => DescriptorSnapshot::Synthetic {
                    path: "/dev/urandom".into(),
                    flags: entry.flags,
                    data: Vec::new(),
                    offset: 0,
                    directory: false,
                },
                Descriptor::Full => DescriptorSnapshot::Synthetic {
                    path: "/dev/full".into(),
                    flags: entry.flags,
                    data: Vec::new(),
                    offset: 0,
                    directory: false,
                },
                Descriptor::EventFd(state) => {
                    let state = state.lock().map_err(|_| EIO)?;
                    DescriptorSnapshot::EventFd {
                        flags: entry.flags,
                        counter: state.counter,
                        semaphore: state.semaphore,
                    }
                }
                Descriptor::Epoll(state) => {
                    let state = state.lock().map_err(|_| EIO)?;
                    DescriptorSnapshot::Epoll {
                        flags: entry.flags,
                        interests: state
                            .interests
                            .iter()
                            .map(|(fd, interest)| {
                                (*fd, interest.events, interest.data, interest.enabled)
                            })
                            .collect(),
                    }
                }
                Descriptor::TimerFd(state) => {
                    let state = state.lock().map_err(|_| EIO)?;
                    DescriptorSnapshot::TimerFd {
                        flags: entry.flags,
                        clock_id: state.clock_id,
                        remaining: state.remaining()?,
                        interval: state.interval,
                        armed: state.armed,
                    }
                }
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
                    self.duplicate(i32::from(*source), 0, None, false)?
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
                DescriptorSnapshot::Socket {
                    handle,
                    flags,
                    readable,
                    writable,
                    local,
                    peer,
                } => self.allocate(
                    Fd {
                        descriptor: Descriptor::Socket {
                            handle: *handle,
                            readable: *readable,
                            writable: *writable,
                            local: local.clone(),
                            peer: peer.clone(),
                        },
                        flags: *flags,
                    },
                    0,
                )?,
                DescriptorSnapshot::UnixEndpoint {
                    flags,
                    bound,
                    listening,
                    pending,
                } => self.allocate(
                    Fd {
                        descriptor: Descriptor::UnixEndpoint(Arc::new(Mutex::new(UnixEndpoint {
                            bound: bound.clone(),
                            listening: *listening,
                            backlog: pending.len().max(1),
                            pending: pending.clone(),
                        }))),
                        flags: *flags,
                    },
                    0,
                )?,
                DescriptorSnapshot::Synthetic {
                    path,
                    flags,
                    data,
                    offset,
                    directory,
                } => {
                    if matches!(path.as_str(), "/dev/random" | "/dev/urandom" | "/dev/full") {
                        self.open(AT_FDCWD, path, *flags)?
                    } else {
                        let offset = usize::try_from(*offset).map_err(|_| EINVAL)?;
                        if offset > data.len() {
                            return Err(EINVAL);
                        }
                        self.allocate(
                            Fd {
                                descriptor: Descriptor::Synthetic {
                                    path: path.clone(),
                                    data: data.clone(),
                                    offset,
                                    directory: *directory,
                                },
                                flags: *flags,
                            },
                            0,
                        )?
                    }
                }
                DescriptorSnapshot::EventFd {
                    flags,
                    counter,
                    semaphore,
                } => self.allocate(
                    Fd {
                        descriptor: Descriptor::EventFd(Arc::new(Mutex::new(EventFdState {
                            counter: *counter,
                            semaphore: *semaphore,
                        }))),
                        flags: *flags,
                    },
                    0,
                )?,
                DescriptorSnapshot::Epoll { flags, interests } => self.allocate(
                    Fd {
                        descriptor: Descriptor::Epoll(Arc::new(Mutex::new(EpollState {
                            interests: interests
                                .iter()
                                .map(|(fd, events, data, enabled)| {
                                    (
                                        *fd,
                                        EpollInterest {
                                            events: *events,
                                            data: *data,
                                            enabled: *enabled,
                                        },
                                    )
                                })
                                .collect(),
                        }))),
                        flags: *flags,
                    },
                    0,
                )?,
                DescriptorSnapshot::TimerFd {
                    flags,
                    clock_id,
                    remaining,
                    interval,
                    armed,
                } => {
                    let mut state = TimerFdState::new(*clock_id);
                    state.set(*remaining, *interval, false)?;
                    state.armed = *armed;
                    self.allocate(
                        Fd {
                            descriptor: Descriptor::TimerFd(Arc::new(Mutex::new(state))),
                            flags: *flags,
                        },
                        0,
                    )?
                }
            };
            if temporary as usize != target {
                let cloexec = match descriptor {
                    DescriptorSnapshot::File { flags, .. }
                    | DescriptorSnapshot::Directory { flags, .. }
                    | DescriptorSnapshot::Null { flags }
                    | DescriptorSnapshot::Zero { flags }
                    | DescriptorSnapshot::Socket { flags, .. }
                    | DescriptorSnapshot::UnixEndpoint { flags, .. }
                    | DescriptorSnapshot::EventFd { flags, .. }
                    | DescriptorSnapshot::Epoll { flags, .. }
                    | DescriptorSnapshot::TimerFd { flags, .. } => flags & O_CLOEXEC != 0,
                    DescriptorSnapshot::Synthetic { flags, .. } => flags & O_CLOEXEC != 0,
                    DescriptorSnapshot::Stdio { .. } => false,
                };
                self.duplicate(temporary, 0, Some(target), cloexec)?;
                self.close(temporary)?;
            }
        }
        Ok(())
    }

    pub fn socket_pair(&mut self, flags: u32, pipe: bool) -> Result<(i32, i32), i64> {
        let (left, right) = Socket::pair().map_err(|_| ENOSPC)?;
        let left_fd = match self.allocate(
            Fd {
                descriptor: Descriptor::Socket {
                    handle: left.0,
                    readable: !pipe,
                    writable: true,
                    local: String::new(),
                    peer: String::new(),
                },
                flags,
            },
            0,
        ) {
            Ok(fd) => fd,
            Err(error) => {
                let _ = Memory::close(left.0);
                let _ = Memory::close(right.0);
                return Err(error);
            }
        };
        match self.allocate(
            Fd {
                descriptor: Descriptor::Socket {
                    handle: right.0,
                    readable: true,
                    writable: !pipe,
                    local: String::new(),
                    peer: String::new(),
                },
                flags,
            },
            0,
        ) {
            Ok(right_fd) => Ok((left_fd, right_fd)),
            Err(error) => {
                let _ = self.close(left_fd);
                let _ = Memory::close(right.0);
                Err(error)
            }
        }
    }

    pub fn unix_socket(&mut self, flags: u32) -> Result<i32, i64> {
        self.allocate(
            Fd {
                descriptor: Descriptor::UnixEndpoint(Arc::new(Mutex::new(UnixEndpoint::default()))),
                flags,
            },
            0,
        )
    }

    pub fn eventfd(&mut self, initial: u64, flags: u32) -> Result<i32, i64> {
        self.allocate(
            Fd {
                descriptor: Descriptor::EventFd(Arc::new(Mutex::new(EventFdState {
                    counter: initial,
                    semaphore: flags & 1 != 0,
                }))),
                flags: flags & !1,
            },
            0,
        )
    }

    pub fn epoll_create(&mut self, flags: u32) -> Result<i32, i64> {
        self.allocate(
            Fd {
                descriptor: Descriptor::Epoll(Arc::new(Mutex::new(EpollState::default()))),
                flags,
            },
            0,
        )
    }

    pub fn epoll_ctl(
        &self,
        epfd: i32,
        operation: u64,
        fd: i32,
        events: u32,
        data: u64,
    ) -> Result<(), i64> {
        if epfd == fd || events & !EPOLL_ALLOWED != 0 {
            return Err(EINVAL);
        }
        let watched = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        if matches!(watched.descriptor, Descriptor::Epoll(_)) {
            return Err(EINVAL);
        }
        let epoll = self
            .fds
            .get(epfd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::Epoll(state) = &epoll.descriptor else {
            return Err(EINVAL);
        };
        let mut state = state.lock().map_err(|_| EIO)?;
        match operation {
            1 => {
                if state.interests.contains_key(&fd) {
                    return Err(EEXIST);
                }
                state.interests.insert(
                    fd,
                    EpollInterest {
                        events,
                        data,
                        enabled: true,
                    },
                );
            }
            2 => {
                if state.interests.remove(&fd).is_none() {
                    return Err(ENOENT);
                }
            }
            3 => {
                let interest = state.interests.get_mut(&fd).ok_or(ENOENT)?;
                *interest = EpollInterest {
                    events,
                    data,
                    enabled: true,
                };
            }
            _ => return Err(EINVAL),
        }
        Ok(())
    }

    pub fn epoll_wait(
        &self,
        epfd: i32,
        maximum: usize,
        timeout_nanos: Option<u64>,
    ) -> Result<Vec<(u32, u64)>, i64> {
        let state = match &self
            .fds
            .get(epfd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?
            .descriptor
        {
            Descriptor::Epoll(state) => state.clone(),
            _ => return Err(EINVAL),
        };
        let deadline = timeout_nanos.map(|nanos| {
            bexos_userspace::syscall::ticks().saturating_add(
                (nanos as u128 * bexos_userspace::syscall::frequency() as u128 / 1_000_000_000)
                    as u64,
            )
        });
        loop {
            let interests: Vec<_> = state
                .lock()
                .map_err(|_| EIO)?
                .interests
                .iter()
                .map(|(fd, interest)| (*fd, *interest))
                .collect();
            let mut ready = Vec::new();
            for (fd, interest) in interests {
                if !interest.enabled {
                    continue;
                }
                let requested = (interest.events | EPOLLERR | EPOLLHUP) as i16;
                let events = self.readiness(fd, requested).unwrap_or(EPOLLERR as i16) as u16 as u32;
                let events = events & (interest.events | EPOLLERR | EPOLLHUP);
                if events == 0 {
                    continue;
                }
                ready.push((events, interest.data));
                if interest.events & EPOLLONESHOT != 0 {
                    if let Some(interest) = state.lock().map_err(|_| EIO)?.interests.get_mut(&fd) {
                        interest.enabled = false;
                    }
                }
                if ready.len() == maximum {
                    break;
                }
            }
            if !ready.is_empty()
                || deadline.is_some_and(|deadline| bexos_userspace::syscall::ticks() >= deadline)
            {
                return Ok(ready);
            }
            bexos_userspace::yield_now();
        }
    }

    pub fn timerfd_create(&mut self, clock_id: i32, flags: u32) -> Result<i32, i64> {
        let state = TimerFdState::new(clock_id);
        state.now()?;
        self.allocate(
            Fd {
                descriptor: Descriptor::TimerFd(Arc::new(Mutex::new(state))),
                flags,
            },
            0,
        )
    }

    pub fn timerfd_get(&self, fd: i32) -> Result<(u64, u64), i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::TimerFd(state) = &entry.descriptor else {
            return Err(EINVAL);
        };
        let state = state.lock().map_err(|_| EIO)?;
        Ok((state.remaining()?, state.interval))
    }

    pub fn timerfd_set(
        &self,
        fd: i32,
        value: u64,
        interval: u64,
        absolute: bool,
    ) -> Result<(u64, u64), i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::TimerFd(state) = &entry.descriptor else {
            return Err(EINVAL);
        };
        let mut state = state.lock().map_err(|_| EIO)?;
        let old = (state.remaining()?, state.interval);
        state.set(value, interval, absolute)?;
        Ok(old)
    }

    pub fn bind_unix(&mut self, fd: i32, path: &str) -> Result<(), i64> {
        if path.is_empty() || path.len() > 107 {
            return Err(EINVAL);
        }
        let path = if path.starts_with('@') {
            path.to_string()
        } else {
            self.absolute(AT_FDCWD, path)?
        };
        for entry in self.fds.iter().filter_map(Option::as_ref) {
            if let Descriptor::UnixEndpoint(endpoint) = &entry.descriptor {
                let endpoint = endpoint.lock().map_err(|_| EIO)?;
                if endpoint.bound == path {
                    return Err(EADDRINUSE);
                }
            }
        }
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::UnixEndpoint(endpoint) = &entry.descriptor else {
            return Err(starnix_kernel::ENOTSOCK);
        };
        let mut endpoint = endpoint.lock().map_err(|_| EIO)?;
        if !endpoint.bound.is_empty() {
            return Err(EINVAL);
        }
        endpoint.bound = path;
        Ok(())
    }

    pub fn listen_unix(&self, fd: i32, backlog: u64) -> Result<(), i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::UnixEndpoint(endpoint) = &entry.descriptor else {
            return Err(starnix_kernel::ENOTSOCK);
        };
        let mut endpoint = endpoint.lock().map_err(|_| EIO)?;
        if endpoint.bound.is_empty() {
            return Err(EINVAL);
        }
        endpoint.listening = true;
        endpoint.backlog = usize::try_from(backlog).unwrap_or(128).clamp(1, 128);
        Ok(())
    }

    pub fn connect_unix(&mut self, fd: i32, path: &str) -> Result<(), i64> {
        let path = if path.starts_with('@') {
            path.to_string()
        } else {
            self.absolute(AT_FDCWD, path)?
        };
        let local_endpoint = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::UnixEndpoint(local_endpoint) = &local_endpoint.descriptor else {
            return Err(
                if matches!(local_endpoint.descriptor, Descriptor::Socket { .. }) {
                    EISCONN
                } else {
                    starnix_kernel::ENOTSOCK
                },
            );
        };
        let local_name = local_endpoint.lock().map_err(|_| EIO)?.bound.clone();
        let listener = self
            .fds
            .iter()
            .filter_map(Option::as_ref)
            .find_map(|entry| match &entry.descriptor {
                Descriptor::UnixEndpoint(endpoint) => {
                    let state = endpoint.lock().ok()?;
                    (state.bound == path && state.listening).then(|| endpoint.clone())
                }
                _ => None,
            })
            .ok_or(ECONNREFUSED)?;
        let (client, server) = Socket::pair().map_err(|_| ENOSPC)?;
        {
            let mut listener = listener.lock().map_err(|_| EIO)?;
            if listener.pending.len() >= listener.backlog {
                let _ = Memory::close(client.0);
                let _ = Memory::close(server.0);
                return Err(starnix_kernel::EAGAIN);
            }
            listener.pending.push((server.0, local_name.clone()));
        }
        let entry = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(EBADF)?;
        entry.descriptor = Descriptor::Socket {
            handle: client.0,
            readable: true,
            writable: true,
            local: local_name,
            peer: path,
        };
        Ok(())
    }

    pub fn accept_unix(&mut self, fd: i32, flags: u32) -> Result<(i32, String), i64> {
        let (endpoint, nonblocking) = {
            let entry = self
                .fds
                .get(fd as usize)
                .and_then(Option::as_ref)
                .ok_or(EBADF)?;
            let Descriptor::UnixEndpoint(endpoint) = &entry.descriptor else {
                return Err(starnix_kernel::ENOTSOCK);
            };
            (
                endpoint.clone(),
                flags & 0x800 != 0 || entry.flags & 0x800 != 0,
            )
        };
        loop {
            let accepted = {
                let mut endpoint = endpoint.lock().map_err(|_| EIO)?;
                if !endpoint.listening {
                    return Err(EINVAL);
                }
                if endpoint.pending.is_empty() {
                    None
                } else {
                    Some((endpoint.bound.clone(), endpoint.pending.remove(0)))
                }
            };
            if let Some((local, (handle, peer))) = accepted {
                let accepted_fd = self.allocate(
                    Fd {
                        descriptor: Descriptor::Socket {
                            handle,
                            readable: true,
                            writable: true,
                            local,
                            peer: peer.clone(),
                        },
                        flags,
                    },
                    0,
                )?;
                return Ok((accepted_fd, peer));
            }
            if nonblocking {
                return Err(starnix_kernel::EAGAIN);
            }
            bexos_userspace::yield_now();
        }
    }

    pub fn unix_name(&self, fd: i32, peer: bool) -> Result<String, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        match &entry.descriptor {
            Descriptor::Socket {
                local,
                peer: peer_name,
                ..
            } => Ok(if peer { peer_name } else { local }.clone()),
            Descriptor::UnixEndpoint(endpoint) if !peer => {
                Ok(endpoint.lock().map_err(|_| EIO)?.bound.clone())
            }
            Descriptor::UnixEndpoint(_) => Err(ENOTCONN),
            _ => Err(starnix_kernel::ENOTSOCK),
        }
    }

    pub fn unix_accepting(&self, fd: i32) -> Result<bool, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        match &entry.descriptor {
            Descriptor::UnixEndpoint(endpoint) => Ok(endpoint.lock().map_err(|_| EIO)?.listening),
            Descriptor::Socket { .. } => Ok(false),
            _ => Err(starnix_kernel::ENOTSOCK),
        }
    }

    pub fn shutdown(&self, fd: i32, how: u64) -> Result<(), i64> {
        if how > 2 {
            return Err(EINVAL);
        }
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let Descriptor::Socket { handle, .. } = &entry.descriptor else {
            return Err(starnix_kernel::ENOTSOCK);
        };
        Socket(*handle)
            .shutdown(how != 1, how != 0)
            .map_err(|_| EIO)
    }

    pub fn readiness(&self, fd: i32, requested: i16) -> Result<i16, i64> {
        let entry = self
            .fds
            .get(fd as usize)
            .and_then(Option::as_ref)
            .ok_or(EBADF)?;
        let mut ready = 0;
        match &entry.descriptor {
            Descriptor::Socket {
                handle,
                readable,
                writable,
                ..
            } => {
                let info = Socket(*handle).info().map_err(|_| EIO)?;
                if requested & 0x001 != 0
                    && *readable
                    && (info.readable_bytes != 0 || info.peer_write_closed)
                {
                    ready |= 0x001;
                }
                if requested & 0x004 != 0 && *writable && !info.peer_read_closed {
                    ready |= 0x004;
                }
                if info.peer_read_closed || info.peer_write_closed {
                    ready |= 0x010;
                }
            }
            Descriptor::UnixEndpoint(endpoint) => {
                let endpoint = endpoint.lock().map_err(|_| EIO)?;
                if requested & 0x001 != 0 && endpoint.listening && !endpoint.pending.is_empty() {
                    ready |= 0x001;
                }
                if requested & 0x004 != 0 && !endpoint.listening {
                    ready |= 0x004;
                }
            }
            Descriptor::EventFd(state) => {
                let state = state.lock().map_err(|_| EIO)?;
                if requested & 0x001 != 0 && state.counter != 0 {
                    ready |= 0x001;
                }
                if requested & 0x004 != 0 && state.counter < u64::MAX - 1 {
                    ready |= 0x004;
                }
            }
            Descriptor::Epoll(state) => {
                let interests: Vec<_> = state
                    .lock()
                    .map_err(|_| EIO)?
                    .interests
                    .iter()
                    .map(|(fd, interest)| (*fd, *interest))
                    .collect();
                if requested & 0x001 != 0
                    && interests.into_iter().any(|(fd, interest)| {
                        interest.enabled
                            && self
                                .readiness(fd, interest.events as i16)
                                .is_ok_and(|events| events != 0)
                    })
                {
                    ready |= 0x001;
                }
            }
            Descriptor::TimerFd(state) => {
                if requested & 0x001 != 0 && state.lock().map_err(|_| EIO)?.ready()? {
                    ready |= 0x001;
                }
            }
            Descriptor::File { .. }
            | Descriptor::Directory { .. }
            | Descriptor::Null
            | Descriptor::Zero
            | Descriptor::Synthetic { .. }
            | Descriptor::Random
            | Descriptor::Full
            | Descriptor::Stdio {
                kind: StdioKind::Console,
                ..
            } => ready = requested & (0x001 | 0x004),
            Descriptor::Stdio {
                kind: StdioKind::Socket(handle),
                ..
            } => {
                let info = Socket(*handle).info().map_err(|_| EIO)?;
                if requested & 0x001 != 0 && (info.readable_bytes != 0 || info.peer_write_closed) {
                    ready |= 0x001;
                }
                if requested & 0x004 != 0 && !info.peer_read_closed {
                    ready |= 0x004;
                }
            }
        }
        Ok(ready)
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
    let (file_type, directory_kind) = match kind {
        NodeKind::Directory => (0o040000, 4),
        NodeKind::Symlink => (0o120000, 10),
        _ => (0o100000, 8),
    };
    Stat {
        inode: path_hash(path),
        mode: attr.mode | file_type,
        uid: attr.uid,
        gid: attr.gid,
        links: 1,
        size: attr.size_bytes,
        blocks: attr.storage_allocated_bytes.div_ceil(512),
        created: attr.creation_time_nanos,
        modified: attr.modification_time_nanos,
        kind: directory_kind,
    }
}

fn device_stat(mode: u32) -> Stat {
    Stat {
        inode: 1,
        mode,
        uid: 0,
        gid: 0,
        links: 1,
        size: 0,
        blocks: 0,
        created: 0,
        modified: 0,
        kind: 2,
    }
}

fn synthetic_stat(path: &str, size: u64, directory: bool) -> Stat {
    Stat {
        inode: path_hash(path),
        mode: if directory { 0o040555 } else { 0o100444 },
        uid: 0,
        gid: 0,
        links: if directory { 2 } else { 1 },
        size,
        blocks: size.div_ceil(512),
        created: 0,
        modified: 0,
        kind: if directory { 4 } else { 8 },
    }
}

fn update_attributes(node: Channel, update: impl FnOnce(&mut FileAttributes)) -> Result<(), i64> {
    let mut attr = fs::attributes(node).map_err(errno)?;
    update(&mut attr);
    fs::set_attributes(node, attr).map_err(errno)
}

fn dot_entries(path: &str) -> Vec<DirectoryEntry> {
    vec![
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
    ]
}

pub(crate) fn synthetic_directory(path: &str) -> Option<Vec<String>> {
    let names = match path {
        "/proc" => vec![
            "1",
            "cpuinfo",
            "loadavg",
            "meminfo",
            "mounts",
            "self",
            "sys",
            "thread-self",
            "uptime",
            "version",
        ],
        "/proc/1" | "/proc/self" | "/proc/thread-self" => {
            vec![
                "cmdline", "exe", "fd", "maps", "mounts", "stat", "status", "task",
            ]
        }
        "/proc/self/fd" | "/proc/1/fd" | "/proc/thread-self/fd" => vec![],
        "/proc/self/task" | "/proc/1/task" | "/proc/thread-self/task" => vec!["1"],
        "/proc/sys" => vec!["kernel", "vm"],
        "/proc/sys/kernel" => vec!["hostname", "osrelease", "ostype"],
        "/proc/sys/vm" => vec!["overcommit_memory"],
        "/sys" => vec!["class", "devices"],
        "/sys/class" => vec![],
        "/sys/devices" => vec!["system"],
        "/sys/devices/system" => vec!["cpu"],
        "/sys/devices/system/cpu" => vec!["online", "possible", "present"],
        "/dev" => vec![
            "fd", "full", "null", "random", "stderr", "stdin", "stdout", "tty", "urandom", "zero",
        ],
        "/dev/fd" => vec!["0", "1", "2"],
        _ => return None,
    };
    Some(names.into_iter().map(str::to_string).collect())
}

pub(crate) fn synthetic_file(path: &str) -> Option<Vec<u8>> {
    let text = match path {
        "/proc/cpuinfo" => {
            if cfg!(target_arch = "aarch64") {
                "processor\t: 0\nBogoMIPS\t: 100.00\nFeatures\t: fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics\nCPU architecture: 8\n"
            } else {
                "processor\t: 0\nvendor_id\t: BexOS\nmodel name\t: BexOS Virtual CPU\ncpu MHz\t\t: 1000.000\nflags\t\t: fpu sse sse2 cx8 syscall nx lm\n"
            }
        }
        "/proc/meminfo" => {
            "MemTotal:         524288 kB\nMemFree:          262144 kB\nMemAvailable:     262144 kB\nBuffers:               0 kB\nCached:                0 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n"
        }
        "/proc/loadavg" => "0.00 0.00 0.00 1/1 1\n",
        "/proc/uptime" => "0.00 0.00\n",
        "/proc/version" => "Linux version 6.6.0-bexos (starnix@bexos) #1 SMP\n",
        "/proc/mounts" | "/proc/self/mounts" | "/proc/1/mounts" | "/proc/thread-self/mounts" => {
            "rootfs / rootfs rw 0 0\nproc /proc proc ro 0 0\nsysfs /sys sysfs ro 0 0\ntmpfs /dev tmpfs rw 0 0\n"
        }
        "/proc/self/status" | "/proc/1/status" | "/proc/thread-self/status" => {
            "Name:\tlinux\nState:\tR (running)\nTgid:\t1\nPid:\t1\nPPid:\t0\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nThreads:\t1\nSeccomp:\t0\n"
        }
        "/proc/self/stat" | "/proc/1/stat" | "/proc/thread-self/stat" => {
            "1 (linux) R 0 1 1 0 0 0 0 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"
        }
        "/proc/sys/kernel/hostname" => "bexos\n",
        "/proc/sys/kernel/osrelease" => "6.6.0-bexos\n",
        "/proc/sys/kernel/ostype" => "Linux\n",
        "/proc/sys/vm/overcommit_memory" => "0\n",
        "/sys/devices/system/cpu/online"
        | "/sys/devices/system/cpu/possible"
        | "/sys/devices/system/cpu/present" => "0\n",
        "/proc/self/cmdline" | "/proc/1/cmdline" | "/proc/thread-self/cmdline" => "linux\0",
        _ => return None,
    };
    Some(text.as_bytes().to_vec())
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

    #[test]
    fn pseudo_filesystem_has_expected_offline_linux_nodes() {
        let proc = synthetic_directory("/proc").unwrap();
        assert!(proc.iter().any(|entry| entry == "self"));
        assert!(proc.iter().any(|entry| entry == "cpuinfo"));
        assert!(synthetic_directory("/sys/devices/system/cpu").is_some());
        assert_eq!(
            synthetic_file("/sys/devices/system/cpu/online").unwrap(),
            b"0\n"
        );
        assert!(
            synthetic_file("/proc/cpuinfo")
                .unwrap()
                .starts_with(b"processor")
        );
        assert!(synthetic_directory("/does/not/exist").is_none());

        let mut vfs = Vfs {
            mounts: Vec::new(),
            fds: Vec::new(),
            cwd: "/".into(),
            executable: "/bin/test".into(),
            synthetic_overrides: BTreeMap::new(),
            synthetic_directory_overrides: BTreeMap::new(),
        };
        let maps = b"00001000-00002000 r-xp 00000000 00:00 0 [image]\n";
        vfs.set_synthetic("/proc/self/maps", maps.to_vec());
        let fd = vfs.open(AT_FDCWD, "/proc/self/maps", 0).unwrap();
        assert_eq!(vfs.read(fd, 4096), Ok(maps.to_vec()));
        vfs.set_synthetic_directory("/proc/self/task", vec!["1".into(), "7".into()]);
        let task_fd = vfs.open(AT_FDCWD, "/proc/self/task", 0x1_0000).unwrap();
        let task_entries = vfs.entries(task_fd).unwrap();
        assert!(task_entries.iter().any(|entry| entry.name == "1"));
        assert!(task_entries.iter().any(|entry| entry.name == "7"));
    }

    #[test]
    fn eventfd_drives_level_and_oneshot_epoll_readiness() {
        let mut vfs = Vfs {
            mounts: Vec::new(),
            fds: Vec::new(),
            cwd: "/".into(),
            executable: "/bin/test".into(),
            synthetic_overrides: BTreeMap::new(),
            synthetic_directory_overrides: BTreeMap::new(),
        };
        let event = vfs.eventfd(0, 0x800).unwrap();
        let epoll = vfs.epoll_create(0).unwrap();
        vfs.epoll_ctl(epoll, 1, event, 0x001 | EPOLLONESHOT, 0xfeed)
            .unwrap();
        assert!(vfs.epoll_wait(epoll, 4, Some(0)).unwrap().is_empty());
        assert_eq!(vfs.write(event, &3u64.to_ne_bytes()), Ok(8));
        assert_eq!(
            vfs.epoll_wait(epoll, 4, Some(0)).unwrap(),
            vec![(0x001, 0xfeed)]
        );
        assert!(vfs.epoll_wait(epoll, 4, Some(0)).unwrap().is_empty());
        vfs.epoll_ctl(epoll, 3, event, 0x001, 7).unwrap();
        assert_eq!(vfs.epoll_wait(epoll, 4, Some(0)).unwrap(), vec![(1, 7)]);
        assert_eq!(vfs.read(event, 8), Ok(3u64.to_ne_bytes().to_vec()));
        assert_eq!(vfs.read(event, 8), Err(starnix_kernel::EAGAIN));
    }

    #[test]
    fn descriptor_duplication_and_close_range_preserve_linux_cloexec_rules() {
        let mut vfs = Vfs {
            mounts: Vec::new(),
            fds: Vec::new(),
            cwd: "/".into(),
            executable: "/bin/test".into(),
            synthetic_overrides: BTreeMap::new(),
            synthetic_directory_overrides: BTreeMap::new(),
        };
        let original = vfs.eventfd(0, O_CLOEXEC | O_NONBLOCK).unwrap();
        assert_eq!(vfs.fd_flags(original), Ok(1));
        assert_eq!(vfs.duplicate(original, MAX_FDS, None, false), Err(EMFILE));

        let plain = vfs.duplicate(original, 0, None, false).unwrap();
        assert_eq!(vfs.fd_flags(plain), Ok(0));
        assert_eq!(vfs.flags(plain).unwrap() & O_NONBLOCK, O_NONBLOCK);

        let cloexec = vfs.duplicate(original, 0, None, true).unwrap();
        assert_eq!(vfs.fd_flags(cloexec), Ok(1));
        vfs.set_fd_flags(cloexec, 0).unwrap();
        assert_eq!(vfs.fd_flags(cloexec), Ok(0));

        vfs.close_range(plain as u32, cloexec as u32, 4).unwrap();
        assert_eq!(vfs.fd_flags(plain), Ok(1));
        assert_eq!(vfs.fd_flags(cloexec), Ok(1));
        vfs.close_range(plain as u32, cloexec as u32, 0).unwrap();
        assert_eq!(vfs.flags(plain), Err(EBADF));
        assert_eq!(vfs.flags(cloexec), Err(EBADF));
    }
}
