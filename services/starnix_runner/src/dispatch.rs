use crate::{
    abi,
    memory::{AddressSpace, MappingKind},
    polling,
    signals::{AltStack, SA_RESTORER, SignalAction, SignalState},
    vfs::{AT_FDCWD, Vfs},
};
use bexos_migration::{
    Error as MigrationError,
    codec::{Decoder, Encoder},
};
use bexos_starnix_abi::NixRunnerOptions;
use bexos_starnix_net::{InterfaceBackend, NetError, NetworkState, SocketDomain, SocketKind};
use bexos_userspace::Memory;
use bexos_zircon::{Clock, ClockId};
use starnix_kernel::{
    Architecture, EACCES, EAFNOSUPPORT, EBADF, EEXIST, EINVAL, ELOOP, ENOENT, ENOMEM, ENOSYS,
    ENOTSUP, EPERM, Syscall, error,
};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FutexAtomicOperation {
    Set,
    Add,
    Or,
    AndNot,
    Xor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FutexComparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

pub enum Outcome {
    Return(u64),
    Exit {
        status: i32,
        group: bool,
    },
    Sigreturn,
    Clone {
        flags: u64,
        stack: u64,
        parent_tid: u64,
        child_tid: u64,
        tls: u64,
    },
    Wait {
        pid: i64,
        status: u64,
        options: u32,
        rusage: u64,
    },
    Signal {
        pid: i64,
        tid: Option<u32>,
        signal: u32,
    },
    SetTidAddress(u64),
    SetRobustList(u64),
    GetRobustList {
        tid: i64,
        head: u64,
        length: u64,
    },
    Rseq {
        address: u64,
        length: u32,
        flags: u32,
        signature: u32,
    },
    FutexWait {
        address: u64,
        timeout_nanos: i64,
        private: bool,
        bitset: u32,
    },
    FutexWake {
        address: u64,
        count: u32,
        private: bool,
        bitset: u32,
    },
    FutexRequeue {
        address: u64,
        wake_count: u32,
        requeue_count: u32,
        target: u64,
        private: bool,
    },
    FutexWakeOp {
        address: u64,
        wake_count: u32,
        target: u64,
        target_wake_count: u32,
        operation: FutexAtomicOperation,
        operand: i32,
        comparison: FutexComparison,
        comparison_operand: i32,
        private: bool,
    },
    FutexPiLock {
        address: u64,
        timeout_nanos: i64,
        private: bool,
        try_only: bool,
    },
    FutexPiUnlock {
        address: u64,
        private: bool,
    },
    FutexWaitRequeuePi {
        address: u64,
        target: u64,
        timeout_nanos: i64,
        private: bool,
    },
    FutexCmpRequeuePi {
        address: u64,
        target: u64,
        requeue_count: u32,
        private: bool,
    },
    FutexWaitV {
        waiters: Vec<(u64, bool)>,
        timeout_nanos: i64,
    },
    Sleep {
        duration_nanos: u64,
    },
    SignalWait,
    SetArchBase {
        fs: bool,
        value: u64,
    },
    Exec {
        path: String,
        image: starnix_kernel::Image,
        executable: Arc<[u8]>,
        interpreter: Option<Arc<[u8]>>,
    },
}

#[derive(Clone)]
pub struct Dispatcher {
    pub architecture: Architecture,
    random: u64,
    directory_offsets: [usize; 256],
    terminal_mode: u32,
    window: [u16; 2],
    fs_base: u64,
    gs_base: u64,
    uid: u32,
    euid: u32,
    suid: u32,
    fsuid: u32,
    gid: u32,
    egid: u32,
    sgid: u32,
    fsgid: u32,
    groups: Vec<u32>,
    umask: u32,
    hostname: String,
    limits: [(u64, u64); 16],
    task_name: String,
    current_tid: u32,
    task_tids: Vec<u32>,
    pid: u32,
    ppid: u32,
    process_ids: Vec<u32>,
    nice: i8,
}

impl Dispatcher {
    pub fn new(architecture: Architecture, options: &NixRunnerOptions) -> Self {
        let mut limits = [(u64::MAX, u64::MAX); 16];
        limits[7] = (256, 256);
        for limit in &options.resource_limits {
            limits[limit.resource as usize] = (limit.soft, limit.hard);
        }
        Self {
            architecture,
            random: bexos_userspace::syscall::ticks() ^ 0x9e37_79b9_7f4a_7c15,
            directory_offsets: [0; 256],
            terminal_mode: 0x0000_0002 | 0x0000_0008 | 0x0000_0100 | 0x0000_0800,
            window: [24, 80],
            fs_base: 0,
            gs_base: 0,
            uid: options.uid,
            euid: options.uid,
            suid: options.uid,
            fsuid: options.uid,
            gid: options.gid,
            egid: options.gid,
            sgid: options.gid,
            fsgid: options.gid,
            groups: Vec::new(),
            umask: options.umask,
            hostname: options.hostname.clone(),
            limits,
            task_name: linux_name(&options.path),
            current_tid: 1,
            task_tids: vec![1],
            pid: 1,
            ppid: 0,
            process_ids: vec![1],
            nice: 0,
        }
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut out = Encoder::new();
        out.word(7);
        out.word(self.random);
        out.word(u64::from(self.terminal_mode));
        out.word(u64::from(self.window[0]));
        out.word(u64::from(self.window[1]));
        out.word(self.fs_base);
        out.word(self.gs_base);
        for offset in self.directory_offsets {
            out.word(offset as u64);
        }
        out.word(u64::from(self.uid));
        out.word(u64::from(self.gid));
        out.word(u64::from(self.umask));
        out.text(&self.hostname);
        for (soft, hard) in self.limits {
            out.word(soft);
            out.word(hard);
        }
        out.text(&self.task_name);
        out.word(u64::from(self.current_tid));
        out.word(u64::from(self.pid));
        out.word(u64::from(self.ppid));
        out.word(u64::from(self.euid));
        out.word(u64::from(self.suid));
        out.word(u64::from(self.fsuid));
        out.word(u64::from(self.egid));
        out.word(u64::from(self.sgid));
        out.word(u64::from(self.fsgid));
        out.word(self.groups.len() as u64);
        for group in &self.groups {
            out.word(u64::from(*group));
        }
        out.word(self.nice as i64 as u64);
        out.finish()
    }

    pub fn finish_exec(&mut self, path: &str) {
        self.directory_offsets = [0; 256];
        self.fs_base = 0;
        self.gs_base = 0;
        self.task_name = linux_name(path);
    }

    pub fn restore(architecture: Architecture, bytes: &[u8]) -> Result<Self, MigrationError> {
        let mut input = Decoder::new(bytes);
        let version = input.word()?;
        if !matches!(version, 1..=7) {
            return Err(MigrationError::UnsupportedVersion);
        }
        let random = input.word()?;
        let terminal_mode =
            u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        let rows = u16::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        let columns = u16::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        let fs_base = input.word()?;
        let gs_base = input.word()?;
        let mut directory_offsets = [0; 256];
        for offset in &mut directory_offsets {
            *offset = usize::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        }
        let (uid, gid, umask, hostname, limits) = if version >= 2 {
            let uid = u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
            let gid = u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
            let umask = u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
            if umask & !0o777 != 0 {
                return Err(MigrationError::InvalidData);
            }
            let hostname = input.text(64)?.to_string();
            let mut limits = [(u64::MAX, u64::MAX); 16];
            for limit in &mut limits {
                *limit = (input.word()?, input.word()?);
                if limit.0 > limit.1 {
                    return Err(MigrationError::InvalidData);
                }
            }
            (uid, gid, umask, hostname, limits)
        } else {
            let mut limits = [(u64::MAX, u64::MAX); 16];
            limits[7] = (256, 256);
            (0, 0, 0, String::new(), limits)
        };
        let task_name = if version >= 3 {
            input.text(15)?.to_string()
        } else {
            "linux".into()
        };
        let current_tid = if version >= 4 {
            u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?
        } else {
            1
        };
        let (pid, ppid) = if version >= 5 {
            (
                u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?,
                u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?,
            )
        } else {
            (1, 0)
        };
        let (euid, suid, fsuid, egid, sgid, fsgid, groups) = if version >= 6 {
            let euid = credential(&mut input)?;
            let suid = credential(&mut input)?;
            let fsuid = credential(&mut input)?;
            let egid = credential(&mut input)?;
            let sgid = credential(&mut input)?;
            let fsgid = credential(&mut input)?;
            let count = input.count(32)?;
            let mut groups = Vec::with_capacity(count);
            for _ in 0..count {
                groups.push(credential(&mut input)?);
            }
            (euid, suid, fsuid, egid, sgid, fsgid, groups)
        } else {
            (uid, uid, uid, gid, gid, gid, Vec::new())
        };
        let nice = if version >= 7 {
            let value =
                i8::try_from(input.word()? as i64).map_err(|_| MigrationError::InvalidData)?;
            if !(-20..=19).contains(&value) {
                return Err(MigrationError::InvalidData);
            }
            value
        } else {
            0
        };
        input.finish()?;
        Ok(Self {
            architecture,
            random,
            directory_offsets,
            terminal_mode,
            window: [rows, columns],
            fs_base,
            gs_base,
            uid,
            euid,
            suid,
            fsuid,
            gid,
            egid,
            sgid,
            fsgid,
            groups,
            umask,
            hostname,
            limits,
            task_name,
            current_tid,
            task_tids: vec![current_tid],
            pid,
            ppid,
            process_ids: vec![pid],
            nice,
        })
    }

    pub fn set_task_context(&mut self, tid: u32, tids: &[u32]) {
        self.current_tid = tid;
        self.task_tids.clear();
        self.task_tids.extend_from_slice(tids);
    }

    pub fn nice(&self) -> i8 {
        self.nice
    }

    pub fn set_process_context(&mut self, pid: u32, ppid: u32, process_ids: &[u32]) {
        self.pid = pid;
        self.ppid = ppid;
        self.process_ids.clear();
        self.process_ids.extend_from_slice(process_ids);
    }

    pub fn proc_status(&self, tid: u32) -> Vec<u8> {
        let groups = self
            .groups
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "Name:\t{}\nState:\tR (running)\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nTracerPid:\t0\nUid:\t{}\t{}\t{}\t{}\nGid:\t{}\t{}\t{}\t{}\nGroups:\t{}\nThreads:\t{}\nNoNewPrivs:\t0\nSeccomp:\t0\n",
            self.task_name,
            self.pid,
            tid,
            self.ppid,
            self.uid,
            self.euid,
            self.suid,
            self.fsuid,
            self.gid,
            self.egid,
            self.sgid,
            self.fsgid,
            groups,
            self.task_tids.len(),
        )
        .into_bytes()
    }

    pub fn proc_stat(&self, tid: u32) -> Vec<u8> {
        format!(
            "{} ({}) R {} {} {} 0 -1 0 0 0 0 0 0 0 0 0 0 20 {} {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
            tid,
            self.task_name,
            self.ppid,
            self.pid,
            self.pid,
            self.nice,
            self.task_tids.len(),
        )
        .into_bytes()
    }

    pub fn call_with_backends(
        &mut self,
        syscall: Syscall,
        args: [u64; 6],
        memory: &mut AddressSpace,
        vfs: &mut Vfs,
        signals: &mut SignalState,
        network: &mut NetworkState,
        l2: &mut crate::l2_backend::L2Runtime,
    ) -> Outcome {
        l2.poll(network);
        match self.call_inner(syscall, args, memory, vfs, signals, network, l2) {
            Ok(outcome) => outcome,
            Err(errno) => Outcome::Return(error(errno)),
        }
    }

    fn call_inner(
        &mut self,
        syscall: Syscall,
        a: [u64; 6],
        memory: &mut AddressSpace,
        vfs: &mut Vfs,
        signals: &mut SignalState,
        network: &mut NetworkState,
        l2: &mut crate::l2_backend::L2Runtime,
    ) -> Result<Outcome, i64> {
        use Syscall::*;
        let maps = memory.proc_maps().into_bytes();
        for path in [
            "/proc/self/maps".to_string(),
            format!("/proc/{}/maps", self.pid),
            "/proc/thread-self/maps".to_string(),
        ] {
            vfs.set_synthetic(&path, maps.clone());
        }
        vfs.set_synthetic(
            "/proc/sys/kernel/hostname",
            format!("{}\n", self.hostname).into_bytes(),
        );
        let status = self.proc_status(self.pid);
        for path in [
            "/proc/self/status".to_string(),
            format!("/proc/{}/status", self.pid),
        ] {
            vfs.set_synthetic(&path, status.clone());
        }
        vfs.set_synthetic(
            "/proc/thread-self/status",
            self.proc_status(self.current_tid),
        );
        let stat = self.proc_stat(self.pid);
        for path in [
            "/proc/self/stat".to_string(),
            format!("/proc/{}/stat", self.pid),
        ] {
            vfs.set_synthetic(&path, stat.clone());
        }
        vfs.set_synthetic("/proc/thread-self/stat", self.proc_stat(self.current_tid));
        let task_entries = self
            .task_tids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>();
        for path in [
            "/proc/self/task".to_string(),
            format!("/proc/{}/task", self.pid),
            "/proc/thread-self/task".to_string(),
        ] {
            vfs.set_synthetic_directory(&path, task_entries.clone());
        }
        refresh_network_files(vfs, network, self.current_tid, self.pid);
        let value = match syscall {
            Read => {
                let bytes = vfs.read(a[0] as i32, bounded(a[2])?)?;
                memory.write(a[1], &bytes)?;
                bytes.len() as u64
            }
            Write => {
                let bytes = memory.read(a[1], bounded(a[2])?)?;
                vfs.write(a[0] as i32, bytes)? as u64
            }
            Readv | Writev => self.vectored(
                matches!(syscall, Readv),
                a[0] as i32,
                a[1],
                a[2],
                memory,
                vfs,
            )?,
            Preadv | Pwritev | Preadv2 | Pwritev2 => {
                if matches!(syscall, Preadv2 | Pwritev2) && a[5] != 0 {
                    return Err(ENOTSUP);
                }
                let offset = (a[3] & 0xffff_ffff) | ((a[4] & 0xffff_ffff) << 32);
                if matches!(syscall, Preadv2 | Pwritev2) && offset == u64::MAX {
                    self.vectored(
                        matches!(syscall, Preadv2),
                        a[0] as i32,
                        a[1],
                        a[2],
                        memory,
                        vfs,
                    )?
                } else {
                    if offset > i64::MAX as u64 {
                        return Err(EINVAL);
                    }
                    self.positional_vectored(
                        matches!(syscall, Preadv | Preadv2),
                        a[0] as i32,
                        a[1],
                        a[2],
                        offset,
                        memory,
                        vfs,
                    )?
                }
            }
            Pread64 => {
                let bytes = vfs.read_at(a[0] as i32, a[3], bounded(a[2])?)?;
                memory.write(a[1], &bytes)?;
                bytes.len() as u64
            }
            Pwrite64 => {
                let bytes = memory.read(a[1], bounded(a[2])?)?;
                vfs.write_at(a[0] as i32, a[3], bytes)? as u64
            }
            Sendfile => {
                let explicit_offset = a[2] != 0;
                let offset = if explicit_offset {
                    let value = abi::read_u64(memory.read(a[2], 8)?, 0)? as i64;
                    if value < 0 {
                        return Err(EINVAL);
                    }
                    value as u64
                } else {
                    vfs.seek(a[1] as i32, 0, 1)?
                };
                let bytes = vfs.read_at(a[1] as i32, offset, bounded(a[3])?)?;
                let written = vfs.write(a[0] as i32, &bytes)?;
                let next = offset.checked_add(written as u64).ok_or(EINVAL)?;
                let next = i64::try_from(next).map_err(|_| EINVAL)?;
                if explicit_offset {
                    memory.write(a[2], &next.to_ne_bytes())?;
                } else {
                    vfs.seek(a[1] as i32, next, 0)?;
                }
                written as u64
            }
            Open => {
                let namespace_path = memory.cstring(a[0], 4096)?;
                let fd = self.open(AT_FDCWD, a[0], a[1], a[2], memory, vfs)?;
                bind_netns_path(network, self.current_tid, self.pid, fd, &namespace_path)?;
                fd as u64
            }
            OpenAt => {
                let namespace_path = memory.cstring(a[1], 4096)?;
                let fd = self.open(a[0] as i32, a[1], a[2], a[3], memory, vfs)?;
                bind_netns_path(network, self.current_tid, self.pid, fd, &namespace_path)?;
                fd as u64
            }
            OpenAt2 => {
                if a[3] < 24 {
                    return Err(EINVAL);
                }
                let how = memory.read(a[2], 24)?;
                let flags = abi::read_u64(how, 0)?;
                let mode = abi::read_u64(how, 8)?;
                let resolve = abi::read_u64(how, 16)?;
                if resolve & !0x3f != 0 {
                    return Err(EINVAL);
                }
                if resolve != 0 {
                    return Err(ENOTSUP);
                }
                let namespace_path = memory.cstring(a[1], 4096)?;
                let fd = self.open(a[0] as i32, a[1], flags, mode, memory, vfs)?;
                bind_netns_path(network, self.current_tid, self.pid, fd, &namespace_path)?;
                fd as u64
            }
            Close => {
                let descriptor = owned_fd(self.pid, a[0] as i32);
                let backend = closing_backend(network, descriptor);
                vfs.close(a[0] as i32)?;
                network.close_fd(descriptor);
                if network.namespace_fds.contains_key(&descriptor) {
                    network.close_namespace_fd(descriptor).map_err(net_errno)?;
                }
                close_backend(l2, backend);
                0
            }
            CloseRange => {
                vfs.close_range(a[0] as u32, a[1] as u32, a[2] as u32)?;
                if a[2] & 4 == 0 {
                    let end = a[1].min(255);
                    for fd in a[0]..=end {
                        let descriptor = owned_fd(self.pid, fd as i32);
                        let backend = closing_backend(network, descriptor);
                        network.close_fd(descriptor);
                        if network.namespace_fds.contains_key(&descriptor) {
                            network.close_namespace_fd(descriptor).map_err(net_errno)?;
                        }
                        close_backend(l2, backend);
                    }
                }
                0
            }
            Dup => {
                let fd = vfs.duplicate(a[0] as i32, 0, None, false)?;
                let source = owned_fd(self.pid, a[0] as i32);
                let target = owned_fd(self.pid, fd);
                if network.socket_by_fd(source).is_some() {
                    network.clone_fd(source, target).map_err(net_errno)?;
                }
                if network.namespace_fds.contains_key(&source) {
                    network
                        .clone_namespace_fd(source, target)
                        .map_err(net_errno)?;
                }
                fd as u64
            }
            Dup2 => {
                if a[0] == a[1] {
                    vfs.duplicate(a[0] as i32, 0, Some(a[1] as usize), false)? as u64
                } else {
                    let target = owned_fd(self.pid, a[1] as i32);
                    let replaced_backend = closing_backend(network, target);
                    let fd = vfs.duplicate(a[0] as i32, 0, Some(a[1] as usize), false)?;
                    let source = owned_fd(self.pid, a[0] as i32);
                    if network.socket_by_fd(source).is_some() {
                        network.clone_fd(source, target).map_err(net_errno)?;
                    } else {
                        network.close_fd(target);
                    }
                    clone_or_close_netns_fd(network, source, target)?;
                    close_backend(l2, replaced_backend);
                    fd as u64
                }
            }
            Dup3 => {
                if a[0] == a[1] || a[2] & !0x8_0000 != 0 {
                    return Err(EINVAL);
                }
                let target = owned_fd(self.pid, a[1] as i32);
                let replaced_backend = closing_backend(network, target);
                let fd =
                    vfs.duplicate(a[0] as i32, 0, Some(a[1] as usize), a[2] & 0x8_0000 != 0)?;
                let source = owned_fd(self.pid, a[0] as i32);
                if network.socket_by_fd(source).is_some() {
                    network.clone_fd(source, target).map_err(net_errno)?;
                } else {
                    network.close_fd(target);
                }
                clone_or_close_netns_fd(network, source, target)?;
                close_backend(l2, replaced_backend);
                fd as u64
            }
            Fcntl => match a[1] {
                0 | 1030 => {
                    let fd = vfs.duplicate(a[0] as i32, a[2] as usize, None, a[1] == 1030)?;
                    let source = owned_fd(self.pid, a[0] as i32);
                    let target = owned_fd(self.pid, fd);
                    if network.socket_by_fd(source).is_some() {
                        network.clone_fd(source, target).map_err(net_errno)?;
                    }
                    clone_or_close_netns_fd(network, source, target)?;
                    fd as u64
                }
                1 => u64::from(vfs.fd_flags(a[0] as i32)?),
                2 => {
                    vfs.set_fd_flags(a[0] as i32, a[2] as u32)?;
                    0
                }
                3 => u64::from(vfs.flags(a[0] as i32)? & !0x8_0000),
                4 => {
                    vfs.set_flags(a[0] as i32, a[2] as u32)?;
                    0
                }
                _ => return Err(ENOTSUP),
            },
            Fstat => {
                self.write_stat(vfs.stat_fd(a[0] as i32)?, a[1], memory)?;
                0
            }
            Statfs => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.stat_path(AT_FDCWD, &path)?;
                memory.write(a[1], &abi::encode_statfs())?;
                0
            }
            Fstatfs => {
                vfs.stat_fd(a[0] as i32)?;
                memory.write(a[1], &abi::encode_statfs())?;
                0
            }
            Stat => {
                let path = memory.cstring(a[0], 4096)?;
                self.write_stat(vfs.stat_path(AT_FDCWD, &path)?, a[1], memory)?;
                0
            }
            Lstat => {
                let path = memory.cstring(a[0], 4096)?;
                self.write_stat(vfs.lstat_path(AT_FDCWD, &path)?, a[1], memory)?;
                0
            }
            NewFstatAt => {
                if a[3] & !(0x100 | 0x800 | 0x1000) != 0 {
                    return Err(EINVAL);
                }
                let path = memory.cstring(a[1], 4096)?;
                let stat = if path.is_empty() && a[3] & 0x1000 != 0 {
                    vfs.stat_fd(a[0] as i32)?
                } else if a[3] & 0x100 != 0 {
                    vfs.lstat_path(a[0] as i32, &path)?
                } else {
                    vfs.stat_path(a[0] as i32, &path)?
                };
                self.write_stat(stat, a[2], memory)?;
                0
            }
            Statx => {
                if a[2] & !(0x100 | 0x800 | 0x1000 | 0x6000) != 0 {
                    return Err(EINVAL);
                }
                let path = memory.cstring(a[1], 4096)?;
                let stat = if path.is_empty() && a[2] & 0x1000 != 0 {
                    vfs.stat_fd(a[0] as i32)?
                } else if a[2] & 0x100 != 0 {
                    vfs.lstat_path(a[0] as i32, &path)?
                } else {
                    vfs.stat_path(a[0] as i32, &path)?
                };
                memory.write(a[4], &abi::encode_statx(&stat))?;
                0
            }
            Getdents64 => {
                let fd = a[0] as usize;
                if fd >= self.directory_offsets.len() {
                    return Err(EBADF);
                }
                let entries = vfs.entries(fd as i32)?;
                let (bytes, encoded) = abi::encode_dirents(
                    &entries[self.directory_offsets[fd].min(entries.len())..],
                    bounded(a[2])?,
                    self.directory_offsets[fd],
                );
                self.directory_offsets[fd] = self.directory_offsets[fd].saturating_add(encoded);
                memory.write(a[1], &bytes)?;
                bytes.len() as u64
            }
            Lseek => vfs.seek(a[0] as i32, a[1] as i64, a[2] as u8)?,
            Access | FaccessAt | FaccessAt2 => {
                let (dirfd, pointer) = if matches!(syscall, Access) {
                    (AT_FDCWD, a[0])
                } else {
                    (a[0] as i32, a[1])
                };
                let mode = if matches!(syscall, Access) {
                    a[1]
                } else {
                    a[2]
                };
                if mode & !7 != 0 {
                    return Err(EINVAL);
                }
                let flags = if matches!(syscall, FaccessAt2) {
                    a[3]
                } else {
                    0
                };
                if flags & !(0x100 | 0x200 | 0x1000) != 0 {
                    return Err(EINVAL);
                }
                let path = memory.cstring(pointer, 4096)?;
                let stat = if path.is_empty() && flags & 0x1000 != 0 {
                    vfs.stat_fd(dirfd)?
                } else if flags & 0x100 != 0 {
                    vfs.lstat_path(dirfd, &path)?
                } else {
                    vfs.stat_path(dirfd, &path)?
                };
                let effective = flags & 0x200 != 0;
                let (uid, gid) = if effective {
                    (self.euid, self.egid)
                } else {
                    (self.uid, self.gid)
                };
                self.check_access(&stat, mode as u8, uid, gid)?;
                0
            }
            Getcwd => {
                let bytes = vfs.cwd().as_bytes();
                if a[1] as usize <= bytes.len() {
                    return Err(starnix_kernel::ERANGE);
                }
                memory.write(a[0], bytes)?;
                memory.write(a[0] + bytes.len() as u64, &[0])?;
                (bytes.len() + 1) as u64
            }
            Chdir => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.chdir(&path)?;
                0
            }
            Fchdir => {
                vfs.chdir_fd(a[0] as i32)?;
                0
            }
            Mkdir => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.mkdir(AT_FDCWD, &path)?;
                if let Err(error) = self.initialize_created(vfs, AT_FDCWD, &path, a[1] as u32) {
                    let _ = vfs.unlink(AT_FDCWD, &path, true);
                    return Err(error);
                }
                0
            }
            MkdirAt => {
                let path = memory.cstring(a[1], 4096)?;
                vfs.mkdir(a[0] as i32, &path)?;
                if let Err(error) = self.initialize_created(vfs, a[0] as i32, &path, a[2] as u32) {
                    let _ = vfs.unlink(a[0] as i32, &path, true);
                    return Err(error);
                }
                0
            }
            Unlink => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.unlink(AT_FDCWD, &path, false)?;
                0
            }
            Rmdir => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.unlink(AT_FDCWD, &path, true)?;
                0
            }
            UnlinkAt => {
                let path = memory.cstring(a[1], 4096)?;
                vfs.unlink(a[0] as i32, &path, a[2] & 0x200 != 0)?;
                0
            }
            Fsync | Fdatasync => {
                vfs.sync(a[0] as i32)?;
                0
            }
            Fadvise64 => {
                vfs.stat_fd(a[0] as i32)?;
                if a[3] > 5 {
                    return Err(EINVAL);
                }
                0
            }
            Chmod => {
                let path = memory.cstring(a[0], 4096)?;
                self.authorize_chmod(&vfs.stat_path(AT_FDCWD, &path)?)?;
                vfs.chmod_path(AT_FDCWD, &path, false, a[1] as u32)?;
                0
            }
            Fchmod => {
                self.authorize_chmod(&vfs.stat_fd(a[0] as i32)?)?;
                vfs.chmod_fd(a[0] as i32, a[1] as u32)?;
                0
            }
            FchmodAt | FchmodAt2 => {
                let flags = if matches!(syscall, FchmodAt2) {
                    a[3]
                } else {
                    0
                };
                if flags & !(0x100 | 0x1000) != 0 {
                    return Err(EINVAL);
                }
                let path = memory.cstring(a[1], 4096)?;
                if path.is_empty() {
                    if flags & 0x1000 == 0 {
                        return Err(ENOENT);
                    }
                    self.authorize_chmod(&vfs.stat_fd(a[0] as i32)?)?;
                    vfs.chmod_fd(a[0] as i32, a[2] as u32)?;
                } else {
                    let stat = if flags & 0x100 != 0 {
                        vfs.lstat_path(a[0] as i32, &path)?
                    } else {
                        vfs.stat_path(a[0] as i32, &path)?
                    };
                    self.authorize_chmod(&stat)?;
                    vfs.chmod_path(a[0] as i32, &path, flags & 0x100 != 0, a[2] as u32)?;
                }
                0
            }
            Chown | Lchown => {
                let path = memory.cstring(a[0], 4096)?;
                let stat = if matches!(syscall, Lchown) {
                    vfs.lstat_path(AT_FDCWD, &path)?
                } else {
                    vfs.stat_path(AT_FDCWD, &path)?
                };
                self.authorize_chown(&stat, a[1] as u32, a[2] as u32)?;
                vfs.chown_path(
                    AT_FDCWD,
                    &path,
                    matches!(syscall, Lchown),
                    a[1] as u32,
                    a[2] as u32,
                )?;
                0
            }
            Fchown => {
                self.authorize_chown(&vfs.stat_fd(a[0] as i32)?, a[1] as u32, a[2] as u32)?;
                vfs.chown_fd(a[0] as i32, a[1] as u32, a[2] as u32)?;
                0
            }
            FchownAt => {
                if a[4] & !(0x100 | 0x1000) != 0 {
                    return Err(EINVAL);
                }
                let path = memory.cstring(a[1], 4096)?;
                if path.is_empty() {
                    if a[4] & 0x1000 == 0 {
                        return Err(ENOENT);
                    }
                    self.authorize_chown(&vfs.stat_fd(a[0] as i32)?, a[2] as u32, a[3] as u32)?;
                    vfs.chown_fd(a[0] as i32, a[2] as u32, a[3] as u32)?;
                } else {
                    let stat = if a[4] & 0x100 != 0 {
                        vfs.lstat_path(a[0] as i32, &path)?
                    } else {
                        vfs.stat_path(a[0] as i32, &path)?
                    };
                    self.authorize_chown(&stat, a[2] as u32, a[3] as u32)?;
                    vfs.chown_path(
                        a[0] as i32,
                        &path,
                        a[4] & 0x100 != 0,
                        a[2] as u32,
                        a[3] as u32,
                    )?;
                }
                0
            }
            Setxattr | Lsetxattr | Fsetxattr => {
                let (path, fd, name_pointer, value_pointer, size, flags, nofollow) =
                    if matches!(syscall, Fsetxattr) {
                        (None, Some(a[0] as i32), a[1], a[2], a[3], a[4], false)
                    } else {
                        (
                            Some(memory.cstring(a[0], 4096)?),
                            None,
                            a[1],
                            a[2],
                            a[3],
                            a[4],
                            matches!(syscall, Lsetxattr),
                        )
                    };
                if flags & !3 != 0 || flags == 3 {
                    return Err(EINVAL);
                }
                let name = memory.cstring(name_pointer, 255)?;
                if name == "security.capability" && self.euid != 0 {
                    return Err(EPERM);
                }
                if size > 65_536 {
                    return Err(starnix_kernel::ERANGE);
                }
                let value = if size == 0 {
                    &[]
                } else {
                    memory.read(value_pointer, size as usize)?
                };
                if let Some(fd) = fd {
                    vfs.set_xattr_fd(fd, &name, value, flags as u32)?;
                } else {
                    vfs.set_xattr_path(
                        AT_FDCWD,
                        path.as_deref().unwrap(),
                        nofollow,
                        &name,
                        value,
                        flags as u32,
                    )?;
                }
                0
            }
            Getxattr | Lgetxattr | Fgetxattr => {
                let (value, pointer, size) = if matches!(syscall, Fgetxattr) {
                    let name = memory.cstring(a[1], 255)?;
                    (vfs.get_xattr_fd(a[0] as i32, &name)?, a[2], a[3])
                } else {
                    let path = memory.cstring(a[0], 4096)?;
                    let name = memory.cstring(a[1], 255)?;
                    (
                        vfs.get_xattr_path(AT_FDCWD, &path, matches!(syscall, Lgetxattr), &name)?,
                        a[2],
                        a[3],
                    )
                };
                write_sized(memory, pointer, size, &value)?
            }
            Listxattr | Llistxattr | Flistxattr => {
                let (names, pointer, size) = if matches!(syscall, Flistxattr) {
                    (vfs.list_xattrs_fd(a[0] as i32)?, a[1], a[2])
                } else {
                    let path = memory.cstring(a[0], 4096)?;
                    (
                        vfs.list_xattrs_path(AT_FDCWD, &path, matches!(syscall, Llistxattr))?,
                        a[1],
                        a[2],
                    )
                };
                let mut bytes = Vec::new();
                for name in names {
                    if bytes.len().saturating_add(name.len()).saturating_add(1) > 65_536 {
                        return Err(starnix_kernel::ERANGE);
                    }
                    bytes.extend_from_slice(name.as_bytes());
                    bytes.push(0);
                }
                write_sized(memory, pointer, size, &bytes)?
            }
            Removexattr | Lremovexattr | Fremovexattr => {
                if matches!(syscall, Fremovexattr) {
                    let name = memory.cstring(a[1], 255)?;
                    vfs.remove_xattr_fd(a[0] as i32, &name)?;
                } else {
                    let path = memory.cstring(a[0], 4096)?;
                    let name = memory.cstring(a[1], 255)?;
                    vfs.remove_xattr_path(AT_FDCWD, &path, matches!(syscall, Lremovexattr), &name)?;
                }
                0
            }
            Truncate => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.truncate_path(&path, a[1])?;
                0
            }
            Ftruncate => {
                vfs.truncate_fd(a[0] as i32, a[1])?;
                0
            }
            Readlink | ReadlinkAt => {
                let (dirfd, path_address, buffer, size) = if matches!(syscall, Readlink) {
                    (AT_FDCWD, a[0], a[1], a[2])
                } else {
                    (a[0] as i32, a[1], a[2], a[3])
                };
                let path = memory.cstring(path_address, 4096)?;
                let value = vfs.readlink(dirfd, &path)?;
                let count = bounded(size)?.min(value.len());
                memory.write(buffer, &value[..count])?;
                count as u64
            }
            Rename | RenameAt | RenameAt2 => {
                if matches!(syscall, RenameAt2) && a[4] != 0 {
                    return Err(ENOTSUP);
                }
                let (old_dirfd, old_pointer, new_dirfd, new_pointer) = if matches!(syscall, Rename)
                {
                    (AT_FDCWD, a[0], AT_FDCWD, a[1])
                } else {
                    (a[0] as i32, a[1], a[2] as i32, a[3])
                };
                let old = memory.cstring(old_pointer, 4096)?;
                let new = memory.cstring(new_pointer, 4096)?;
                vfs.rename(old_dirfd, &old, new_dirfd, &new)?;
                0
            }
            Link | LinkAt => {
                let (source_dirfd, source_pointer, target_dirfd, target_pointer, flags) =
                    if matches!(syscall, Link) {
                        (AT_FDCWD, a[0], AT_FDCWD, a[1], 0)
                    } else {
                        (a[0] as i32, a[1], a[2] as i32, a[3], a[4])
                    };
                if flags & !0x400 != 0 {
                    return Err(EINVAL);
                }
                if flags & 0x400 != 0 {
                    return Err(ENOTSUP);
                }
                let source = memory.cstring(source_pointer, 4096)?;
                let target = memory.cstring(target_pointer, 4096)?;
                vfs.link(source_dirfd, &source, target_dirfd, &target)?;
                0
            }
            Symlink | SymlinkAt => {
                let (target_pointer, dirfd, link_pointer) = if matches!(syscall, Symlink) {
                    (a[0], AT_FDCWD, a[1])
                } else {
                    (a[0], a[1] as i32, a[2])
                };
                let target = memory.cstring(target_pointer, 4096)?;
                let link_path = memory.cstring(link_pointer, 4096)?;
                vfs.symlink(&target, dirfd, &link_path)?;
                0
            }
            Mmap => {
                let flags = a[3];
                if flags & 3 == 0 || flags & 3 == 3 {
                    return Err(EINVAL);
                }
                let anonymous = flags & 0x20 != 0;
                let fixed = flags & (0x10 | 0x10_0000) != 0;
                if flags & 0x10 != 0 && flags & 0x10_0000 == 0 {
                    let _ = memory.unmap(a[0], a[1]);
                }
                if anonymous {
                    memory.map_anonymous(a[0], a[1], a[2], MappingKind::Anonymous, fixed)?
                } else {
                    let (handle, size) = vfs.backing(a[4] as i32)?;
                    let result = memory.map_file_copy(
                        a[0],
                        a[1],
                        a[2],
                        a[4] as i32,
                        a[5],
                        handle,
                        size,
                        flags & 1 != 0,
                        fixed,
                    );
                    let _ = Memory::close(handle);
                    result?
                }
            }
            Munmap => {
                self.flush(a[0], a[1], memory, vfs)?;
                memory.unmap(a[0], a[1])?;
                0
            }
            Mprotect => {
                memory.protect(a[0], a[1], a[2])?;
                0
            }
            Msync => {
                self.flush(a[0], a[1], memory, vfs)?;
                0
            }
            Madvise => {
                if a[2] > 25 {
                    return Err(EINVAL);
                }
                0
            }
            Mremap => memory.remap(a[0], a[1], a[2], a[3], a[4])?,
            Brk => memory.brk(a[0]),
            Futex => {
                let (operation, private, realtime) = futex_operation(a[1])?;
                if a[0] & 3 != 0 {
                    return Err(EINVAL);
                }
                let _ = memory.read(a[0], 4)?;
                match operation {
                    0 | 9 => {
                        let bitset = if operation == 9 {
                            let bitset = a[5] as u32;
                            if bitset == 0 {
                                return Err(EINVAL);
                            }
                            bitset
                        } else {
                            u32::MAX
                        };
                        let timeout = if a[3] == 0 {
                            -1
                        } else {
                            let requested = abi::decode_timespec(memory.read(a[3], 16)?)?;
                            let duration = if operation == 9 {
                                let now = Clock::get(if realtime {
                                    ClockId::Realtime
                                } else {
                                    ClockId::Monotonic
                                })
                                .map_err(|_| EINVAL)?;
                                requested.saturating_sub(now)
                            } else {
                                requested
                            };
                            i64::try_from(duration).map_err(|_| EINVAL)?
                        };
                        let actual = u32::from_ne_bytes(memory.read(a[0], 4)?.try_into().unwrap());
                        if actual != a[2] as u32 {
                            return Err(starnix_kernel::EAGAIN);
                        }
                        return Ok(Outcome::FutexWait {
                            address: a[0],
                            timeout_nanos: timeout,
                            private,
                            bitset,
                        });
                    }
                    1 | 10 => {
                        let bitset = if operation == 10 {
                            let bitset = a[5] as u32;
                            if bitset == 0 {
                                return Err(EINVAL);
                            }
                            bitset
                        } else {
                            u32::MAX
                        };
                        return Ok(Outcome::FutexWake {
                            address: a[0],
                            count: a[2] as u32,
                            private,
                            bitset,
                        });
                    }
                    3 | 4 => {
                        if a[2] > i32::MAX as u64 || a[3] > i32::MAX as u64 || a[4] & 3 != 0 {
                            return Err(EINVAL);
                        }
                        let _ = memory.read(a[4], 4)?;
                        if operation == 4 {
                            let actual =
                                u32::from_ne_bytes(memory.read(a[0], 4)?.try_into().unwrap());
                            if actual != a[5] as u32 {
                                return Err(starnix_kernel::EAGAIN);
                            }
                        }
                        return Ok(Outcome::FutexRequeue {
                            address: a[0],
                            wake_count: a[2] as u32,
                            requeue_count: a[3] as u32,
                            target: a[4],
                            private,
                        });
                    }
                    5 => {
                        if a[2] > i32::MAX as u64 || a[3] > i32::MAX as u64 || a[4] & 3 != 0 {
                            return Err(EINVAL);
                        }
                        let _ = memory.read(a[4], 4)?;
                        let (atomic_operation, operand, comparison, comparison_operand) =
                            decode_futex_wake_operation(a[5] as u32)?;
                        return Ok(Outcome::FutexWakeOp {
                            address: a[0],
                            wake_count: a[2] as u32,
                            target: a[4],
                            target_wake_count: a[3] as u32,
                            operation: atomic_operation,
                            operand,
                            comparison,
                            comparison_operand,
                            private,
                        });
                    }
                    6 | 13 => {
                        let timeout = if a[3] == 0 {
                            -1
                        } else {
                            i64::try_from(abi::decode_timespec(memory.read(a[3], 16)?)?)
                                .map_err(|_| EINVAL)?
                        };
                        return Ok(Outcome::FutexPiLock {
                            address: a[0],
                            timeout_nanos: timeout,
                            private,
                            try_only: false,
                        });
                    }
                    8 => {
                        return Ok(Outcome::FutexPiLock {
                            address: a[0],
                            timeout_nanos: -1,
                            private,
                            try_only: true,
                        });
                    }
                    7 => {
                        return Ok(Outcome::FutexPiUnlock {
                            address: a[0],
                            private,
                        });
                    }
                    11 => {
                        if a[4] & 3 != 0 || a[4] == a[0] {
                            return Err(EINVAL);
                        }
                        let _ = memory.read(a[4], 4)?;
                        let timeout = if a[3] == 0 {
                            -1
                        } else {
                            let requested = abi::decode_timespec(memory.read(a[3], 16)?)?;
                            let now = Clock::get(if realtime {
                                ClockId::Realtime
                            } else {
                                ClockId::Monotonic
                            })
                            .map_err(|_| EINVAL)?;
                            i64::try_from(requested.saturating_sub(now)).map_err(|_| EINVAL)?
                        };
                        let actual = u32::from_ne_bytes(memory.read(a[0], 4)?.try_into().unwrap());
                        if actual != a[2] as u32 {
                            return Err(starnix_kernel::EAGAIN);
                        }
                        return Ok(Outcome::FutexWaitRequeuePi {
                            address: a[0],
                            target: a[4],
                            timeout_nanos: timeout,
                            private,
                        });
                    }
                    12 => {
                        if a[2] != 1 || a[3] > i32::MAX as u64 || a[4] & 3 != 0 || a[4] == a[0] {
                            return Err(EINVAL);
                        }
                        let _ = memory.read(a[4], 4)?;
                        let actual = u32::from_ne_bytes(memory.read(a[0], 4)?.try_into().unwrap());
                        if actual != a[5] as u32 {
                            return Err(starnix_kernel::EAGAIN);
                        }
                        return Ok(Outcome::FutexCmpRequeuePi {
                            address: a[0],
                            target: a[4],
                            requeue_count: a[3] as u32,
                            private,
                        });
                    }
                    _ => return Err(ENOTSUP),
                }
            }
            FutexWaitv => {
                const FUTEX_32: u32 = 2;
                const FUTEX_PRIVATE: u32 = 0x80;
                let count = usize::try_from(a[1]).map_err(|_| EINVAL)?;
                if count == 0 || count > 128 || a[2] != 0 || !matches!(a[4] as i32, 0 | 1) {
                    return Err(EINVAL);
                }
                let bytes = memory.read(a[0], count.checked_mul(24).ok_or(EINVAL)?)?;
                let mut waiters = Vec::with_capacity(count);
                for index in 0..count {
                    let offset = index * 24;
                    let expected = abi::read_u64(&bytes, offset)?;
                    let address = abi::read_u64(&bytes, offset + 8)?;
                    let flags = abi::read_u32(&bytes, offset + 16)?;
                    let reserved = abi::read_u32(&bytes, offset + 20)?;
                    if expected > u64::from(u32::MAX)
                        || address & 3 != 0
                        || flags & !(FUTEX_32 | FUTEX_PRIVATE) != 0
                        || flags & FUTEX_32 == 0
                        || reserved != 0
                    {
                        return Err(EINVAL);
                    }
                    let actual = u32::from_ne_bytes(memory.read(address, 4)?.try_into().unwrap());
                    if actual != expected as u32 {
                        return Err(starnix_kernel::EAGAIN);
                    }
                    waiters.push((address, flags & FUTEX_PRIVATE != 0));
                }
                let timeout = if a[3] == 0 {
                    -1
                } else {
                    let requested = abi::decode_timespec(memory.read(a[3], 16)?)?;
                    let clock = if a[4] == 0 {
                        ClockId::Realtime
                    } else {
                        ClockId::Monotonic
                    };
                    let now = Clock::get(clock).map_err(|_| EINVAL)?;
                    i64::try_from(requested.saturating_sub(now)).map_err(|_| EINVAL)?
                };
                return Ok(Outcome::FutexWaitV {
                    waiters,
                    timeout_nanos: timeout,
                });
            }
            RtSigaction => {
                let signal = a[0] as u32;
                if a[3] != 8 {
                    return Err(EINVAL);
                }
                let old = signals.action(signal)?;
                if a[2] != 0 {
                    memory.write(a[2], &encode_action(old))?;
                }
                if a[1] != 0 {
                    let action = decode_action(memory.read(a[1], 32)?)?;
                    if action.flags & SA_RESTORER != 0 && action.restorer == 0 {
                        return Err(EINVAL);
                    }
                    signals.set_action(signal, action)?;
                }
                0
            }
            RtSigprocmask => {
                if a[3] != 8 {
                    return Err(EINVAL);
                }
                let old = signals.mask();
                if a[2] != 0 {
                    memory.write(a[2], &old.to_ne_bytes())?;
                }
                if a[1] != 0 {
                    signals.update_mask(a[0] as u32, abi::read_u64(memory.read(a[1], 8)?, 0)?)?;
                }
                0
            }
            RtSigpending => {
                if a[1] != 8 {
                    return Err(EINVAL);
                }
                memory.write(a[0], &signals.pending().to_ne_bytes())?;
                0
            }
            RtSigreturn => return Ok(Outcome::Sigreturn),
            RtSigsuspend => {
                if a[1] != 8 {
                    return Err(EINVAL);
                }
                signals.suspend(abi::read_u64(memory.read(a[0], 8)?, 0)?);
                return Ok(Outcome::SignalWait);
            }
            Sigaltstack => {
                let old = signals.alt_stack();
                if a[1] != 0 {
                    memory.write(a[1], &encode_altstack(old))?;
                }
                if a[0] != 0 {
                    signals.set_alt_stack(decode_altstack(memory.read(a[0], 24)?)?)?;
                }
                0
            }
            Kill => {
                return Ok(Outcome::Signal {
                    pid: a[0] as i64,
                    tid: None,
                    signal: a[1] as u32,
                });
            }
            Tgkill => {
                return Ok(Outcome::Signal {
                    pid: a[0] as i64,
                    tid: Some(a[1] as u32),
                    signal: a[2] as u32,
                });
            }
            ClockGettime => {
                let id = match a[0] as i32 {
                    0 => ClockId::Realtime,
                    1 => ClockId::Monotonic,
                    7 => ClockId::Boot,
                    _ => return Err(EINVAL),
                };
                let nanos = Clock::get(id).map_err(|_| EINVAL)?;
                memory.write(a[1], &abi::encode_timespec(nanos))?;
                0
            }
            ClockGetres => {
                if !matches!(a[0] as i32, 0 | 1 | 7) {
                    return Err(EINVAL);
                }
                if a[1] != 0 {
                    memory.write(a[1], &abi::encode_timespec(1))?;
                }
                0
            }
            Gettimeofday => {
                let nanos = Clock::get(ClockId::Realtime).map_err(|_| EINVAL)?;
                let mut out = [0; 16];
                abi::put_u64(&mut out, 0, nanos / 1_000_000_000);
                abi::put_u64(&mut out, 8, (nanos % 1_000_000_000) / 1000);
                memory.write(a[0], &out)?;
                0
            }
            Nanosleep => {
                let duration = abi::decode_timespec(memory.read(a[0], 16)?)?;
                return Ok(Outcome::Sleep {
                    duration_nanos: duration,
                });
            }
            ClockNanosleep => {
                if !matches!(a[0] as i32, 0 | 1 | 7) || a[1] & !1 != 0 {
                    return Err(EINVAL);
                }
                let requested = abi::decode_timespec(memory.read(a[2], 16)?)?;
                let duration = if a[1] & 1 == 0 {
                    requested
                } else {
                    let now = Clock::get(match a[0] as i32 {
                        0 => ClockId::Realtime,
                        7 => ClockId::Boot,
                        _ => ClockId::Monotonic,
                    })
                    .map_err(|_| EINVAL)?;
                    requested.saturating_sub(now)
                };
                return Ok(Outcome::Sleep {
                    duration_nanos: duration,
                });
            }
            SchedYield => {
                bexos_userspace::yield_now();
                0
            }
            SchedGetparam => {
                self.validate_task_target(a[0])?;
                memory.write(a[1], &0u32.to_ne_bytes())?;
                0
            }
            SchedSetparam => {
                self.validate_task_target(a[0])?;
                if abi::read_u32(memory.read(a[1], 4)?, 0)? != 0 {
                    return Err(EINVAL);
                }
                0
            }
            SchedGetscheduler => {
                self.validate_task_target(a[0])?;
                0
            }
            SchedSetscheduler => {
                self.validate_task_target(a[0])?;
                if a[1] != 0 || abi::read_u32(memory.read(a[2], 4)?, 0)? != 0 {
                    return Err(EINVAL);
                }
                0
            }
            SchedGetPriorityMax | SchedGetPriorityMin => match a[0] {
                0 | 3 | 5 | 6 => 0,
                1 | 2 => {
                    if matches!(syscall, SchedGetPriorityMax) {
                        99
                    } else {
                        1
                    }
                }
                _ => return Err(EINVAL),
            },
            SchedRrGetInterval => {
                self.validate_task_target(a[0])?;
                memory.write(a[1], &abi::encode_timespec(100_000_000))?;
                0
            }
            Getrandom => {
                let length = bounded(a[1])?;
                let mut bytes = vec![0; length];
                for chunk in bytes.chunks_mut(8) {
                    self.random ^= self.random << 13;
                    self.random ^= self.random >> 7;
                    self.random ^= self.random << 17;
                    let value = self.random.to_ne_bytes();
                    let count = chunk.len();
                    chunk.copy_from_slice(&value[..count]);
                }
                memory.write(a[0], &bytes)?;
                length as u64
            }
            Getpid | Getpgrp => u64::from(self.pid),
            Gettid => u64::from(self.current_tid),
            Getppid => u64::from(self.ppid),
            Getpgid | Getsid => {
                if a[0] != 0 && !self.process_ids.contains(&(a[0] as u32)) {
                    return Err(starnix_kernel::ESRCH);
                }
                u64::from(self.pid)
            }
            Setsid => u64::from(self.pid),
            Getpriority => {
                self.validate_priority_target(a[0], a[1])?;
                (20 - i32::from(self.nice)) as u64
            }
            Setpriority => {
                self.validate_priority_target(a[0], a[1])?;
                let requested = (a[2] as u32 as i32).clamp(-20, 19) as i8;
                if requested < self.nice && self.euid != 0 {
                    return Err(EPERM);
                }
                self.nice = requested;
                0
            }
            Getuid => u64::from(self.uid),
            Geteuid => u64::from(self.euid),
            Getgid => u64::from(self.gid),
            Getegid => u64::from(self.egid),
            Getresuid => {
                for (pointer, value) in a[..3].iter().zip([self.uid, self.euid, self.suid]) {
                    memory.write(*pointer, &value.to_ne_bytes())?;
                }
                0
            }
            Getresgid => {
                for (pointer, value) in a[..3].iter().zip([self.gid, self.egid, self.sgid]) {
                    memory.write(*pointer, &value.to_ne_bytes())?;
                }
                0
            }
            Getgroups => {
                let capacity = usize::try_from(a[0]).map_err(|_| EINVAL)?;
                if capacity == 0 {
                    self.groups.len() as u64
                } else {
                    if capacity < self.groups.len() || !self.groups.is_empty() && a[1] == 0 {
                        return Err(EINVAL);
                    }
                    let mut bytes = Vec::with_capacity(self.groups.len() * 4);
                    for group in &self.groups {
                        bytes.extend_from_slice(&group.to_ne_bytes());
                    }
                    memory.write(a[1], &bytes)?;
                    self.groups.len() as u64
                }
            }
            Setuid => {
                self.set_uid(a[0] as u32)?;
                0
            }
            Setgid => {
                self.set_gid(a[0] as u32)?;
                0
            }
            Setreuid => {
                self.set_res_uid(a[0] as u32, a[1] as u32, u32::MAX, true)?;
                0
            }
            Setregid => {
                self.set_res_gid(a[0] as u32, a[1] as u32, u32::MAX, true)?;
                0
            }
            Setresuid => {
                self.set_res_uid(a[0] as u32, a[1] as u32, a[2] as u32, false)?;
                0
            }
            Setresgid => {
                self.set_res_gid(a[0] as u32, a[1] as u32, a[2] as u32, false)?;
                0
            }
            Setfsuid => {
                let previous = self.fsuid;
                let requested = a[0] as u32;
                if self.euid == 0
                    || [self.uid, self.euid, self.suid, self.fsuid].contains(&requested)
                {
                    self.fsuid = requested;
                }
                u64::from(previous)
            }
            Setfsgid => {
                let previous = self.fsgid;
                let requested = a[0] as u32;
                if self.euid == 0
                    || [self.gid, self.egid, self.sgid, self.fsgid].contains(&requested)
                {
                    self.fsgid = requested;
                }
                u64::from(previous)
            }
            Setgroups => {
                if self.euid != 0 {
                    return Err(EPERM);
                }
                let count = usize::try_from(a[0]).map_err(|_| EINVAL)?;
                if count > 32 || count != 0 && a[1] == 0 {
                    return Err(EINVAL);
                }
                let mut groups = Vec::with_capacity(count);
                if count != 0 {
                    let bytes = memory.read(a[1], count * 4)?;
                    for index in 0..count {
                        groups.push(abi::read_u32(bytes, index * 4)?);
                    }
                }
                self.groups = groups;
                0
            }
            Umask => {
                let previous = self.umask;
                self.umask = a[0] as u32 & 0o777;
                u64::from(previous)
            }
            Uname => {
                memory.write(a[0], &abi::encode_uts(&self.hostname))?;
                0
            }
            Getrlimit => {
                self.write_limit(a[0], a[1], memory)?;
                0
            }
            Prlimit64 => {
                if a[0] != 0 && a[0] != 1 {
                    return Err(EINVAL);
                }
                let resource = usize::try_from(a[1]).map_err(|_| EINVAL)?;
                if resource >= self.limits.len() {
                    return Err(EINVAL);
                }
                let old = self.limits[resource];
                if a[2] != 0 {
                    let bytes = memory.read(a[2], 16)?;
                    let replacement = (abi::read_u64(bytes, 0)?, abi::read_u64(bytes, 8)?);
                    if replacement.0 > replacement.1 {
                        return Err(EINVAL);
                    }
                    self.limits[resource] = replacement;
                }
                if a[3] != 0 {
                    let mut out = [0; 16];
                    abi::put_u64(&mut out, 0, old.0);
                    abi::put_u64(&mut out, 8, old.1);
                    memory.write(a[3], &out)?;
                }
                0
            }
            Getrusage => {
                if !matches!(a[0] as i32, -1 | 0 | 1) {
                    return Err(EINVAL);
                }
                memory.write(a[1], &[0; 144])?;
                0
            }
            Times => {
                if a[0] != 0 {
                    memory.write(a[0], &[0; 32])?;
                }
                bexos_userspace::syscall::ticks()
                    .saturating_mul(100)
                    .checked_div(bexos_userspace::syscall::frequency())
                    .unwrap_or(0)
            }
            Sysinfo => {
                let mut info = [0; 112];
                let uptime = Clock::get(ClockId::Boot).map_err(|_| EINVAL)? / 1_000_000_000;
                abi::put_u64(&mut info, 0, uptime);
                abi::put_u64(&mut info, 32, 512 * 1024 * 1024);
                abi::put_u64(&mut info, 40, 256 * 1024 * 1024);
                abi::put_u16(&mut info, 80, 1);
                abi::put_u32(&mut info, 104, 1);
                memory.write(a[0], &info)?;
                0
            }
            SchedGetaffinity => {
                if a[0] != 0 && a[0] != 1 || a[1] < 8 {
                    return Err(EINVAL);
                }
                memory.write(a[2], &1u64.to_ne_bytes())?;
                8
            }
            SchedSetaffinity => {
                if a[0] != 0 && a[0] != u64::from(self.pid) || a[1] < 8 {
                    return Err(EINVAL);
                }
                let mask = abi::read_u64(memory.read(a[2], 8)?, 0)?;
                if mask & 1 == 0 {
                    return Err(EINVAL);
                }
                0
            }
            Getcpu => {
                if a[0] != 0 {
                    memory.write(a[0], &0u32.to_ne_bytes())?;
                }
                if a[1] != 0 {
                    memory.write(a[1], &0u32.to_ne_bytes())?;
                }
                0
            }
            Membarrier => match a[0] {
                0 => 1 | 8 | 16 | 32 | 64 | 128 | 256,
                1 | 8 | 16 | 32 | 64 | 128 | 256 => 0,
                _ => return Err(EINVAL),
            },
            Prctl => match a[0] {
                3 => 0,
                4 => 0,
                15 => {
                    self.task_name = memory.cstring(a[1], 15)?;
                    0
                }
                16 => {
                    let mut name = [0; 16];
                    let count = self.task_name.len().min(15);
                    name[..count].copy_from_slice(&self.task_name.as_bytes()[..count]);
                    memory.write(a[1], &name)?;
                    0
                }
                _ => return Err(ENOTSUP),
            },
            SetTidAddress => return Ok(Outcome::SetTidAddress(a[0])),
            SetRobustList => {
                if a[1] != 24 {
                    return Err(EINVAL);
                }
                return Ok(Outcome::SetRobustList(a[0]));
            }
            GetRobustList => {
                if (a[0] as i64) < 0 || a[1] == 0 || a[2] == 0 {
                    return Err(if (a[0] as i64) < 0 {
                        EINVAL
                    } else {
                        starnix_kernel::EFAULT
                    });
                }
                return Ok(Outcome::GetRobustList {
                    tid: a[0] as i64,
                    head: a[1],
                    length: a[2],
                });
            }
            Rseq => {
                let length = u32::try_from(a[1]).map_err(|_| EINVAL)?;
                let flags = u32::try_from(a[2]).map_err(|_| EINVAL)?;
                let signature = u32::try_from(a[3]).map_err(|_| EINVAL)?;
                return Ok(Outcome::Rseq {
                    address: a[0],
                    length,
                    flags,
                    signature,
                });
            }
            ArchPrctl => {
                if self.architecture != Architecture::X86_64 {
                    return Err(ENOSYS);
                }
                match a[0] {
                    0x1001 => {
                        self.gs_base = a[1];
                        return Ok(Outcome::SetArchBase {
                            fs: false,
                            value: a[1],
                        });
                    }
                    0x1002 => {
                        self.fs_base = a[1];
                        return Ok(Outcome::SetArchBase {
                            fs: true,
                            value: a[1],
                        });
                    }
                    0x1003 => {
                        memory.write(a[1], &self.fs_base.to_ne_bytes())?;
                        0
                    }
                    0x1004 => {
                        memory.write(a[1], &self.gs_base.to_ne_bytes())?;
                        0
                    }
                    _ => return Err(EINVAL),
                }
            }
            Ioctl => self.ioctl(a, memory, network)?,
            Pipe | Pipe2 => {
                let flags = if matches!(syscall, Pipe2) {
                    if a[1] & !(0x800 | 0x8_0000) != 0 {
                        return Err(EINVAL);
                    }
                    a[1] as u32
                } else {
                    0
                };
                let (write_fd, read_fd) = vfs.socket_pair(flags, true)?;
                let mut descriptors = [0; 8];
                abi::put_u32(&mut descriptors, 0, read_fd as u32);
                abi::put_u32(&mut descriptors, 4, write_fd as u32);
                if let Err(error) = memory.write(a[0], &descriptors) {
                    let _ = vfs.close(read_fd);
                    let _ = vfs.close(write_fd);
                    return Err(error);
                }
                0
            }
            Poll => polling::poll(memory, vfs, a[0], a[1], polling::milliseconds(a[2])?)?,
            Ppoll => {
                if a[3] != 0 && a[4] != 8 {
                    return Err(EINVAL);
                }
                let timeout = polling::timespec(memory, a[2])?;
                polling::poll(memory, vfs, a[0], a[1], timeout)?
            }
            Select => {
                let timeout = polling::timeval(memory, a[4])?;
                polling::select(memory, vfs, a[0], a[1], a[2], a[3], timeout)?
            }
            Pselect6 => {
                let timeout = polling::timespec(memory, a[4])?;
                polling::select(memory, vfs, a[0], a[1], a[2], a[3], timeout)?
            }
            Eventfd2 => {
                if a[1] & !(1 | 0x800 | 0x8_0000) != 0 {
                    return Err(EINVAL);
                }
                vfs.eventfd(a[0], a[1] as u32)? as u64
            }
            EpollCreate1 => {
                if a[0] & !0x8_0000 != 0 {
                    return Err(EINVAL);
                }
                vfs.epoll_create(a[0] as u32)? as u64
            }
            EpollCtl => {
                let (events, data) = if a[1] == 2 {
                    (0, 0)
                } else {
                    decode_epoll_event(self.architecture, memory, a[3])?
                };
                vfs.epoll_ctl(a[0] as i32, a[1], a[2] as i32, events, data)?;
                0
            }
            EpollWait | EpollPwait => {
                if matches!(syscall, EpollPwait) && a[4] != 0 && a[5] != 8 {
                    return Err(EINVAL);
                }
                let maximum = usize::try_from(a[2]).map_err(|_| EINVAL)?;
                if maximum == 0 || maximum > 256 {
                    return Err(EINVAL);
                }
                let ready = vfs.epoll_wait(a[0] as i32, maximum, polling::milliseconds(a[3])?)?;
                write_epoll_events(self.architecture, memory, a[1], &ready)?;
                ready.len() as u64
            }
            TimerfdCreate => {
                if a[1] & !(0x800 | 0x8_0000) != 0 {
                    return Err(EINVAL);
                }
                vfs.timerfd_create(a[0] as i32, a[1] as u32)? as u64
            }
            TimerfdGettime => {
                let (value, interval) = vfs.timerfd_get(a[0] as i32)?;
                memory.write(a[1], &encode_itimerspec(value, interval))?;
                0
            }
            TimerfdSettime => {
                if a[1] & !1 != 0 {
                    return Err(EINVAL);
                }
                let (value, interval) = decode_itimerspec(memory, a[2])?;
                let old = vfs.timerfd_set(a[0] as i32, value, interval, a[1] & 1 != 0)?;
                if a[3] != 0 {
                    memory.write(a[3], &encode_itimerspec(old.0, old.1))?;
                }
                0
            }
            Socketpair => {
                if a[0] != 1 {
                    return Err(EAFNOSUPPORT);
                }
                if a[1] & 0xf != 1 || a[1] & !(0xf | 0x800 | 0x8_0000) != 0 || a[2] != 0 {
                    return Err(ENOTSUP);
                }
                let (left, right) = vfs.socket_pair(a[1] as u32 & !0xf, false)?;
                let mut descriptors = [0; 8];
                abi::put_u32(&mut descriptors, 0, left as u32);
                abi::put_u32(&mut descriptors, 4, right as u32);
                if let Err(error) = memory.write(a[3], &descriptors) {
                    let _ = vfs.close(left);
                    let _ = vfs.close(right);
                    return Err(error);
                }
                0
            }
            Shutdown => {
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, a[0] as i32)) {
                    if a[1] > 2 {
                        return Err(EINVAL);
                    }
                    let state = network.sockets.get_mut(&socket).ok_or(EBADF)?;
                    state.shutdown_read |= a[1] != 1;
                    state.shutdown_write |= a[1] != 0;
                    if state.backend_control != 0 && matches!(state.kind, SocketKind::Stream) {
                        vfs.shutdown(a[0] as i32, a[1])?;
                    }
                } else {
                    vfs.shutdown(a[0] as i32, a[1])?;
                }
                0
            }
            Socket => {
                if a[0] == 1 {
                    if a[1] & 0xf != 1 || a[1] & !(0xf | 0x800 | 0x8_0000) != 0 || a[2] != 0 {
                        return Err(ENOTSUP);
                    }
                    vfs.unix_socket(a[1] as u32 & !0xf)? as u64
                } else {
                    let domain = match (a[0], a[2]) {
                        (2, _) => SocketDomain::Inet4,
                        (10, _) => SocketDomain::Inet6,
                        (17, _) => SocketDomain::Packet,
                        (16, 0) => SocketDomain::RouteNetlink,
                        (16, 4) => SocketDomain::SockDiagNetlink,
                        (16, 12) => SocketDomain::NetfilterNetlink,
                        _ => return Err(EAFNOSUPPORT),
                    };
                    let kind = match a[1] & 0xf {
                        1 => SocketKind::Stream,
                        2 => SocketKind::Datagram,
                        3 => SocketKind::Raw,
                        _ => return Err(ENOTSUP),
                    };
                    if a[1] & !(0xf | 0x800 | 0x8_0000) != 0 {
                        return Err(EINVAL);
                    }
                    if !network.namespaces.values().any(|namespace| {
                        namespace.interfaces.values().any(|interface| {
                            !matches!(interface.backend, InterfaceBackend::Loopback)
                        })
                    }) {
                        return Err(EAFNOSUPPORT);
                    }
                    let socket = network
                        .socket(
                            self.current_tid,
                            domain,
                            kind,
                            a[2] as u16,
                            a[1] & 0x800 != 0,
                        )
                        .map_err(net_errno)?;
                    let fd = match vfs.unix_socket(a[1] as u32 & !0xf) {
                        Ok(fd) => fd,
                        Err(error) => {
                            network.sockets.remove(&socket);
                            return Err(error);
                        }
                    };
                    network
                        .bind_fd(socket, owned_fd(self.pid, fd))
                        .map_err(net_errno)?;
                    fd as u64
                }
            }
            Bind => {
                let fd = a[0] as i32;
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, fd)) {
                    let domain = network.sockets.get(&socket).ok_or(EBADF)?.domain;
                    if matches!(
                        domain,
                        SocketDomain::RouteNetlink
                            | SocketDomain::NetfilterNetlink
                            | SocketDomain::SockDiagNetlink
                    ) {
                        let bytes =
                            memory.read(a[1], usize::try_from(a[2]).map_err(|_| EINVAL)?)?;
                        if bytes.len() < 12
                            || u16::from_ne_bytes(bytes[..2].try_into().unwrap()) != 16
                        {
                            return Err(EINVAL);
                        }
                        return Ok(Outcome::Return(0));
                    }
                    let local = decode_ip_address(memory, a[1], a[2], domain)?;
                    let state = network.sockets.get_mut(&socket).ok_or(EBADF)?;
                    if state.local.is_some() {
                        return Err(EINVAL);
                    }
                    state.local = Some(local);
                    if matches!(state.kind, SocketKind::Datagram) && state.protocol != 1 {
                        ensure_udp(network, l2, socket, Some(local.0))?;
                    }
                } else {
                    let path = decode_unix_address(memory, a[1], a[2])?;
                    vfs.bind_unix(fd, &path)?;
                }
                0
            }
            Listen => {
                let fd = a[0] as i32;
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, fd)) {
                    let snapshot = network.sockets.get(&socket).cloned().ok_or(EBADF)?;
                    if !matches!(snapshot.kind, SocketKind::Stream) || snapshot.peer.is_some() {
                        return Err(EINVAL);
                    }
                    let local = snapshot.local.ok_or(EINVAL)?;
                    let listener = match backend_for_socket(network, socket, Some(local.0))? {
                        SocketBackend::Direct {
                            provider,
                            host_interface,
                        } => crate::net_backend::listen_tcp(
                            provider,
                            local,
                            snapshot.nonblocking,
                            host_interface,
                        )?,
                        SocketBackend::L2 { interface } => l2.listen_tcp(interface, local)?,
                        SocketBackend::Loopback { namespace } => {
                            l2.listen_loopback(namespace, local)?
                        }
                    };
                    let state = network.sockets.get_mut(&socket).ok_or(EBADF)?;
                    state.listening = true;
                    state.backend_control = listener;
                } else {
                    vfs.listen_unix(fd, a[1])?;
                }
                0
            }
            Connect => {
                let fd = a[0] as i32;
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, fd)) {
                    let snapshot = network.sockets.get(&socket).cloned().ok_or(EBADF)?;
                    let mut peer = decode_ip_address(memory, a[1], a[2], snapshot.domain)?;
                    if snapshot.peer.is_some() {
                        return Err(starnix_kernel::EISCONN);
                    }
                    if matches!(snapshot.domain, SocketDomain::Inet4 | SocketDomain::Inet6) {
                        let (translated, verdict) = network
                            .filter_egress(
                                socket,
                                peer,
                                if matches!(snapshot.kind, SocketKind::Stream) {
                                    6
                                } else if snapshot.protocol == 1 {
                                    1
                                } else {
                                    17
                                },
                                snapshot.local.map_or(0, |local| local.1),
                            )
                            .map_err(net_errno)?;
                        if verdict != bexos_starnix_net::Verdict::Accept {
                            return Err(EACCES);
                        }
                        peer = translated;
                    }
                    if matches!(snapshot.kind, SocketKind::Datagram) {
                        network.sockets.get_mut(&socket).unwrap().peer = Some(peer);
                        if snapshot.protocol != 1 {
                            ensure_udp(network, l2, socket, None)?;
                        }
                    } else if matches!(snapshot.kind, SocketKind::Stream) {
                        let connection = match backend_for_socket(network, socket, Some(peer.0))? {
                            SocketBackend::Direct {
                                provider,
                                host_interface,
                            } => {
                                let connection = crate::net_backend::connect_tcp(
                                    provider,
                                    peer,
                                    snapshot.nonblocking,
                                    host_interface,
                                )?;
                                crate::l2_backend::TcpConnection {
                                    stream: connection.stream,
                                    control: connection.control,
                                    local: connection.local,
                                }
                            }
                            SocketBackend::L2 { interface } => l2.connect_tcp(interface, peer)?,
                            SocketBackend::Loopback { namespace } => l2.connect_loopback(
                                namespace,
                                snapshot
                                    .local
                                    .unwrap_or_else(|| unspecified(snapshot.domain)),
                                peer,
                            )?,
                        };
                        vfs.install_native_socket(
                            fd,
                            connection.stream,
                            format_socket_address(connection.local),
                            format_socket_address(peer),
                        )?;
                        let state = network.sockets.get_mut(&socket).ok_or(EBADF)?;
                        state.local = Some(connection.local);
                        state.peer = Some(peer);
                        state.backend_control = connection.control;
                    } else {
                        return Err(ENOTSUP);
                    }
                } else {
                    let path = decode_unix_address(memory, a[1], a[2])?;
                    vfs.connect_unix(fd, &path)?;
                }
                0
            }
            Accept | Accept4 => {
                let flags = if matches!(syscall, Accept4) {
                    if a[3] & !(0x800 | 0x8_0000) != 0 {
                        return Err(EINVAL);
                    }
                    a[3] as u32
                } else {
                    0
                };
                let source_fd = a[0] as i32;
                if let Some(listener_socket) = network.socket_by_fd(owned_fd(self.pid, source_fd)) {
                    let listener = network
                        .sockets
                        .get(&listener_socket)
                        .cloned()
                        .ok_or(EBADF)?;
                    if !listener.listening || listener.backend_control == 0 {
                        return Err(EINVAL);
                    }
                    let mut accepted =
                        if crate::l2_backend::L2Runtime::is_object(listener.backend_control) {
                            l2.accept_tcp(listener.backend_control)?
                        } else {
                            let accepted =
                                crate::net_backend::accept_tcp(listener.backend_control)?;
                            crate::l2_backend::AcceptedConnection {
                                stream: accepted.stream,
                                control: accepted.control,
                                peer: accepted.peer,
                            }
                        };
                    let ingress = network.filter_ingress(
                        listener_socket,
                        accepted.peer,
                        6,
                        listener.local.map_or(0, |local| local.1),
                        0,
                    );
                    match ingress {
                        Ok((source, bexos_starnix_net::Verdict::Accept)) => accepted.peer = source,
                        Ok(_) => {
                            let _ = bexos_userspace::Memory::close(accepted.stream);
                            close_backend(l2, Some(accepted.control));
                            return Err(EACCES);
                        }
                        Err(error) => {
                            let _ = bexos_userspace::Memory::close(accepted.stream);
                            close_backend(l2, Some(accepted.control));
                            return Err(net_errno(error));
                        }
                    }
                    let fd = vfs.allocate_native_socket(
                        accepted.stream,
                        flags,
                        listener
                            .local
                            .map(format_socket_address)
                            .unwrap_or_default(),
                        format_socket_address(accepted.peer),
                    )?;
                    let socket = network
                        .socket(
                            self.current_tid,
                            listener.domain,
                            SocketKind::Stream,
                            listener.protocol,
                            flags & 0x800 != 0 || listener.nonblocking,
                        )
                        .map_err(net_errno)?;
                    {
                        let state = network.sockets.get_mut(&socket).unwrap();
                        state.local = listener.local;
                        state.peer = Some(accepted.peer);
                        state.bound_interface = listener.bound_interface;
                        state.backend_control = accepted.control;
                    }
                    if let Err(error) = network
                        .bind_fd(socket, owned_fd(self.pid, fd))
                        .map_err(net_errno)
                    {
                        let _ = vfs.close(fd);
                        network.sockets.remove(&socket);
                        return Err(error);
                    }
                    if a[1] != 0 {
                        write_ip_address(memory, a[1], a[2], accepted.peer)?;
                    }
                    fd as u64
                } else {
                    let (fd, peer) = vfs.accept_unix(source_fd, flags)?;
                    if a[1] != 0 {
                        if let Err(error) = write_unix_address(memory, a[1], a[2], &peer) {
                            let _ = vfs.close(fd);
                            return Err(error);
                        }
                    }
                    fd as u64
                }
            }
            Getsockname | Getpeername => {
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, a[0] as i32)) {
                    let socket = network.sockets.get(&socket).ok_or(EBADF)?;
                    let address = if matches!(syscall, Getpeername) {
                        socket.peer.ok_or(starnix_kernel::ENOTCONN)?
                    } else {
                        socket.local.unwrap_or_else(|| unspecified(socket.domain))
                    };
                    write_ip_address(memory, a[1], a[2], address)?;
                } else {
                    let name = vfs.unix_name(a[0] as i32, matches!(syscall, Getpeername))?;
                    write_unix_address(memory, a[1], a[2], &name)?;
                }
                0
            }
            Sendto => {
                let bytes = memory.read(a[1], bounded(a[2])?)?;
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, a[0] as i32)) {
                    let snapshot = network.sockets.get(&socket).cloned().ok_or(EBADF)?;
                    if matches!(
                        snapshot.domain,
                        SocketDomain::RouteNetlink
                            | SocketDomain::NetfilterNetlink
                            | SocketDomain::SockDiagNetlink
                    ) {
                        return Ok(Outcome::Return(
                            network.netlink_send(socket, bytes).map_err(net_errno)? as u64,
                        ));
                    }
                    if matches!(snapshot.domain, SocketDomain::Packet) {
                        let interface = match backend_for_socket(network, socket, None)? {
                            SocketBackend::L2 { interface } => interface,
                            SocketBackend::Direct { .. } | SocketBackend::Loopback { .. } => {
                                return Err(EPERM);
                            }
                        };
                        return Ok(Outcome::Return(l2.transmit_raw(interface, bytes)? as u64));
                    }
                    if !matches!(snapshot.kind, SocketKind::Datagram) && !is_icmp_socket(&snapshot)
                    {
                        return Err(ENOTSUP);
                    }
                    let mut destination = if a[4] == 0 {
                        snapshot.peer.ok_or(starnix_kernel::EDESTADDRREQ)?
                    } else {
                        decode_ip_address(memory, a[4], a[5], snapshot.domain)?
                    };
                    let (translated, verdict) = network
                        .filter_egress(
                            socket,
                            destination,
                            if is_icmp_socket(&snapshot) {
                                snapshot.protocol as u8
                            } else {
                                17
                            },
                            snapshot.local.map_or(0, |local| local.1),
                        )
                        .map_err(net_errno)?;
                    if verdict != bexos_starnix_net::Verdict::Accept {
                        return Err(EACCES);
                    }
                    destination = translated;
                    if is_icmp_socket(&snapshot) {
                        let interface =
                            match backend_for_socket(network, socket, Some(destination.0))? {
                                SocketBackend::L2 { interface } => interface,
                                SocketBackend::Direct { .. } => return Err(ENOTSUP),
                                SocketBackend::Loopback { .. } => {
                                    return Ok(Outcome::Return(l2.send_loopback_icmp(
                                        network,
                                        socket,
                                        destination.0,
                                        bytes,
                                    )?
                                        as u64));
                                }
                            };
                        return Ok(Outcome::Return(
                            l2.send_icmp(interface, destination.0, bytes)? as u64,
                        ));
                    }
                    ensure_udp(network, l2, socket, Some(destination.0))?;
                    let control = network.sockets.get(&socket).unwrap().backend_control;
                    if crate::l2_backend::L2Runtime::is_object(control) {
                        l2.send_udp(control, bytes, destination)? as u64
                    } else {
                        crate::net_backend::send_udp(control, bytes, destination)? as u64
                    }
                } else {
                    if a[4] != 0 {
                        return Err(ENOTSUP);
                    }
                    vfs.write(a[0] as i32, bytes)? as u64
                }
            }
            Recvfrom => {
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, a[0] as i32)) {
                    let domain = network.sockets.get(&socket).ok_or(EBADF)?.domain;
                    if matches!(
                        domain,
                        SocketDomain::RouteNetlink
                            | SocketDomain::NetfilterNetlink
                            | SocketDomain::SockDiagNetlink
                    ) {
                        let mut bytes = network.netlink_recv(socket).map_err(net_errno)?;
                        bytes.truncate(bounded(a[2])?);
                        memory.write(a[1], &bytes)?;
                        return Ok(Outcome::Return(bytes.len() as u64));
                    }
                    if matches!(domain, SocketDomain::Packet) {
                        let state = network.sockets.get_mut(&socket).ok_or(EBADF)?;
                        if state.queued_packets.is_empty() {
                            return Err(starnix_kernel::EAGAIN);
                        }
                        let mut bytes = state.queued_packets.remove(0);
                        bytes.truncate(bounded(a[2])?);
                        memory.write(a[1], &bytes)?;
                        return Ok(Outcome::Return(bytes.len() as u64));
                    }
                    if network.sockets.get(&socket).is_some_and(is_icmp_socket) {
                        let state = network.sockets.get_mut(&socket).ok_or(EBADF)?;
                        if state.queued_packets.is_empty() {
                            return Err(starnix_kernel::EAGAIN);
                        }
                        let frame = state.queued_packets.remove(0);
                        let (mut bytes, mut source) = decode_icmp_frame(&frame)?;
                        let protocol = if matches!(domain, SocketDomain::Inet6) {
                            58
                        } else {
                            1
                        };
                        let (translated, verdict) = network
                            .filter_ingress(socket, (source, 0), protocol, 0, bytes.len())
                            .map_err(net_errno)?;
                        if verdict != bexos_starnix_net::Verdict::Accept {
                            return Err(EACCES);
                        }
                        source = translated.0;
                        bytes.truncate(bounded(a[2])?);
                        memory.write(a[1], &bytes)?;
                        if a[4] != 0 {
                            write_ip_address(memory, a[4], a[5], (source, 0))?;
                        }
                        return Ok(Outcome::Return(bytes.len() as u64));
                    }
                    ensure_udp(network, l2, socket, None)?;
                    let control = network.sockets.get(&socket).ok_or(EBADF)?.backend_control;
                    let (mut bytes, mut source) =
                        if crate::l2_backend::L2Runtime::is_object(control) {
                            l2.recv_udp(control)?
                        } else {
                            crate::net_backend::recv_udp(control)?
                        };
                    let destination_port = network
                        .sockets
                        .get(&socket)
                        .and_then(|socket| socket.local)
                        .map_or(0, |local| local.1);
                    let (translated, verdict) = network
                        .filter_ingress(socket, source, 17, destination_port, bytes.len())
                        .map_err(net_errno)?;
                    if verdict != bexos_starnix_net::Verdict::Accept {
                        return Err(EACCES);
                    }
                    source = translated;
                    bytes.truncate(bounded(a[2])?);
                    memory.write(a[1], &bytes)?;
                    if a[4] != 0 {
                        write_ip_address(memory, a[4], a[5], source)?;
                    }
                    bytes.len() as u64
                } else {
                    let bytes = vfs.read(a[0] as i32, bounded(a[2])?)?;
                    memory.write(a[1], &bytes)?;
                    if a[4] != 0 {
                        let peer = vfs.unix_name(a[0] as i32, true)?;
                        write_unix_address(memory, a[4], a[5], &peer)?;
                    }
                    bytes.len() as u64
                }
            }
            Sendmsg | Recvmsg => {
                let header = memory.read(a[1], 56)?.to_vec();
                let fd = a[0] as i32;
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, fd)) {
                    let snapshot = network.sockets.get(&socket).cloned().ok_or(EBADF)?;
                    if matches!(
                        snapshot.domain,
                        SocketDomain::RouteNetlink
                            | SocketDomain::NetfilterNetlink
                            | SocketDomain::SockDiagNetlink
                    ) {
                        if matches!(syscall, Sendmsg) {
                            let bytes = read_iovecs(
                                memory,
                                abi::read_u64(&header, 16)?,
                                abi::read_u64(&header, 24)?,
                            )?;
                            network.netlink_send(socket, &bytes).map_err(net_errno)? as u64
                        } else {
                            let bytes = network.netlink_recv(socket).map_err(net_errno)?;
                            let written = write_iovecs(
                                memory,
                                abi::read_u64(&header, 16)?,
                                abi::read_u64(&header, 24)?,
                                &bytes,
                            )?;
                            memory.write(a[1] + 40, &0u64.to_ne_bytes())?;
                            memory.write(a[1] + 48, &0u32.to_ne_bytes())?;
                            written as u64
                        }
                    } else if matches!(snapshot.kind, SocketKind::Datagram)
                        || is_icmp_socket(&snapshot)
                    {
                        if is_icmp_socket(&snapshot) {
                            if matches!(syscall, Sendmsg) {
                                let mut destination = if abi::read_u64(&header, 0)? != 0 {
                                    decode_ip_address(
                                        memory,
                                        abi::read_u64(&header, 0)?,
                                        u64::from(abi::read_u32(&header, 8)?),
                                        snapshot.domain,
                                    )?
                                } else {
                                    snapshot.peer.ok_or(starnix_kernel::EDESTADDRREQ)?
                                };
                                let (translated, verdict) = network
                                    .filter_egress(socket, destination, snapshot.protocol as u8, 0)
                                    .map_err(net_errno)?;
                                if verdict != bexos_starnix_net::Verdict::Accept {
                                    return Err(EACCES);
                                }
                                destination = translated;
                                let bytes = read_iovecs(
                                    memory,
                                    abi::read_u64(&header, 16)?,
                                    abi::read_u64(&header, 24)?,
                                )?;
                                let interface =
                                    match backend_for_socket(network, socket, Some(destination.0))?
                                    {
                                        SocketBackend::L2 { interface } => interface,
                                        SocketBackend::Direct { .. } => return Err(ENOTSUP),
                                        SocketBackend::Loopback { .. } => {
                                            return Ok(Outcome::Return(l2.send_loopback_icmp(
                                                network,
                                                socket,
                                                destination.0,
                                                &bytes,
                                            )?
                                                as u64));
                                        }
                                    };
                                return Ok(Outcome::Return(l2.send_icmp(
                                    interface,
                                    destination.0,
                                    &bytes,
                                )?
                                    as u64));
                            }
                            let state = network.sockets.get_mut(&socket).ok_or(EBADF)?;
                            if state.queued_packets.is_empty() {
                                return Err(starnix_kernel::EAGAIN);
                            }
                            let (bytes, mut source) =
                                decode_icmp_frame(&state.queued_packets.remove(0))?;
                            let protocol = if matches!(snapshot.domain, SocketDomain::Inet6) {
                                58
                            } else {
                                1
                            };
                            let (translated, verdict) = network
                                .filter_ingress(socket, (source, 0), protocol, 0, bytes.len())
                                .map_err(net_errno)?;
                            if verdict != bexos_starnix_net::Verdict::Accept {
                                return Err(EACCES);
                            }
                            source = translated.0;
                            let written = write_iovecs(
                                memory,
                                abi::read_u64(&header, 16)?,
                                abi::read_u64(&header, 24)?,
                                &bytes,
                            )?;
                            if abi::read_u64(&header, 0)? != 0 {
                                write_ip_address(
                                    memory,
                                    abi::read_u64(&header, 0)?,
                                    a[1] + 8,
                                    (source, 0),
                                )?;
                            }
                            write_packet_info_control(
                                memory,
                                a[1],
                                network,
                                socket,
                                snapshot.receive_packet_info,
                            )?;
                            memory.write(a[1] + 48, &0u32.to_ne_bytes())?;
                            return Ok(Outcome::Return(written as u64));
                        }
                        let route_hint = if matches!(syscall, Sendmsg) {
                            if abi::read_u64(&header, 0)? != 0 {
                                Some(
                                    decode_ip_address(
                                        memory,
                                        abi::read_u64(&header, 0)?,
                                        u64::from(abi::read_u32(&header, 8)?),
                                        snapshot.domain,
                                    )?
                                    .0,
                                )
                            } else {
                                snapshot.peer.map(|peer| peer.0)
                            }
                        } else {
                            None
                        };
                        ensure_udp(network, l2, socket, route_hint)?;
                        let control = network.sockets.get(&socket).unwrap().backend_control;
                        if matches!(syscall, Sendmsg) {
                            let mut destination = if abi::read_u64(&header, 0)? != 0 {
                                decode_ip_address(
                                    memory,
                                    abi::read_u64(&header, 0)?,
                                    u64::from(abi::read_u32(&header, 8)?),
                                    snapshot.domain,
                                )?
                            } else {
                                snapshot.peer.ok_or(starnix_kernel::EDESTADDRREQ)?
                            };
                            let (translated, verdict) = network
                                .filter_egress(
                                    socket,
                                    destination,
                                    17,
                                    snapshot.local.map_or(0, |local| local.1),
                                )
                                .map_err(net_errno)?;
                            if verdict != bexos_starnix_net::Verdict::Accept {
                                return Err(EACCES);
                            }
                            destination = translated;
                            let bytes = read_iovecs(
                                memory,
                                abi::read_u64(&header, 16)?,
                                abi::read_u64(&header, 24)?,
                            )?;
                            if crate::l2_backend::L2Runtime::is_object(control) {
                                l2.send_udp(control, &bytes, destination)? as u64
                            } else {
                                crate::net_backend::send_udp(control, &bytes, destination)? as u64
                            }
                        } else {
                            let (bytes, mut source) =
                                if crate::l2_backend::L2Runtime::is_object(control) {
                                    l2.recv_udp(control)?
                                } else {
                                    crate::net_backend::recv_udp(control)?
                                };
                            let (translated, verdict) = network
                                .filter_ingress(
                                    socket,
                                    source,
                                    17,
                                    snapshot.local.map_or(0, |local| local.1),
                                    bytes.len(),
                                )
                                .map_err(net_errno)?;
                            if verdict != bexos_starnix_net::Verdict::Accept {
                                return Err(EACCES);
                            }
                            source = translated;
                            let written = write_iovecs(
                                memory,
                                abi::read_u64(&header, 16)?,
                                abi::read_u64(&header, 24)?,
                                &bytes,
                            )?;
                            if abi::read_u64(&header, 0)? != 0 {
                                write_ip_address(
                                    memory,
                                    abi::read_u64(&header, 0)?,
                                    a[1] + 8,
                                    source,
                                )?;
                            }
                            write_packet_info_control(
                                memory,
                                a[1],
                                network,
                                socket,
                                snapshot.receive_packet_info,
                            )?;
                            memory.write(a[1] + 48, &0u32.to_ne_bytes())?;
                            written as u64
                        }
                    } else {
                        if abi::read_u64(&header, 0)? != 0 {
                            return Err(starnix_kernel::EISCONN);
                        }
                        self.vectored(
                            matches!(syscall, Recvmsg),
                            fd,
                            abi::read_u64(&header, 16)?,
                            abi::read_u64(&header, 24)?,
                            memory,
                            vfs,
                        )?
                    }
                } else {
                    if abi::read_u64(&header, 32)? != 0 || abi::read_u64(&header, 40)? != 0 {
                        return Err(ENOTSUP);
                    }
                    if matches!(syscall, Sendmsg) && abi::read_u64(&header, 0)? != 0 {
                        return Err(ENOTSUP);
                    }
                    let result = self.vectored(
                        matches!(syscall, Recvmsg),
                        fd,
                        abi::read_u64(&header, 16)?,
                        abi::read_u64(&header, 24)?,
                        memory,
                        vfs,
                    )?;
                    if matches!(syscall, Recvmsg) {
                        if abi::read_u64(&header, 0)? != 0 {
                            let peer = vfs.unix_name(fd, true)?;
                            write_unix_address(
                                memory,
                                abi::read_u64(&header, 0)?,
                                a[1] + 8,
                                &peer,
                            )?;
                        }
                        memory.write(a[1] + 48, &0u32.to_ne_bytes())?;
                    }
                    result
                }
            }
            Setsockopt => {
                if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, a[0] as i32)) {
                    match (a[1], a[2]) {
                        (1, 25) => {
                            let length = usize::try_from(a[4]).map_err(|_| EINVAL)?;
                            if length > 16 {
                                return Err(EINVAL);
                            }
                            let bytes = memory.read(a[3], length)?;
                            let name =
                                core::str::from_utf8(bytes.strip_suffix(&[0]).unwrap_or(bytes))
                                    .map_err(|_| EINVAL)?;
                            if name.is_empty() {
                                network
                                    .sockets
                                    .get_mut(&socket)
                                    .ok_or(EBADF)?
                                    .bound_interface = None;
                            } else {
                                network.bind_interface(socket, name).map_err(net_errno)?;
                            }
                        }
                        (1, 2 | 7 | 8 | 9 | 20 | 21) if a[4] >= 4 => {}
                        (0, 64) => {
                            let length = bounded(a[4])?;
                            let bytes = memory.read(a[3], length)?;
                            network
                                .replace_iptables_ipv4(socket, bytes)
                                .map_err(net_errno)?;
                        }
                        (0, 8) if a[4] >= 4 => {
                            let enabled = abi::read_u32(memory.read(a[3], 4)?, 0)? != 0;
                            network
                                .set_receive_packet_info(socket, enabled)
                                .map_err(net_errno)?;
                        }
                        (41, 49) if a[4] >= 4 => {
                            let enabled = abi::read_u32(memory.read(a[3], 4)?, 0)? != 0;
                            network
                                .set_receive_packet_info(socket, enabled)
                                .map_err(net_errno)?;
                        }
                        (0, option @ (35 | 36)) if matches!(a[4], 8 | 12) => {
                            let bytes = memory.read(a[3], a[4] as usize)?;
                            let group =
                                IpAddr::V4(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]));
                            let interface = if bytes.len() == 12 {
                                let index = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
                                (index != 0).then_some(u64::from(index))
                            } else {
                                None
                            };
                            network
                                .update_multicast_membership(socket, group, interface, option == 35)
                                .map_err(net_errno)?;
                        }
                        (41, option @ (20 | 21)) if a[4] == 20 => {
                            let bytes = memory.read(a[3], 20)?;
                            let group = IpAddr::V6(Ipv6Addr::from(
                                <[u8; 16]>::try_from(&bytes[..16]).unwrap(),
                            ));
                            let index = u32::from_ne_bytes(bytes[16..20].try_into().unwrap());
                            network
                                .update_multicast_membership(
                                    socket,
                                    group,
                                    (index != 0).then_some(u64::from(index)),
                                    option == 20,
                                )
                                .map_err(net_errno)?;
                        }
                        _ => return Err(ENOTSUP),
                    }
                } else {
                    if a[1] != 1 || !matches!(a[2], 2 | 7 | 8 | 9 | 20 | 21) || a[4] < 4 {
                        return Err(ENOTSUP);
                    }
                }
                0
            }
            Getsockopt => {
                let length = abi::read_u32(memory.read(a[4], 4)?, 0)?;
                if length < 4 {
                    return Err(EINVAL);
                }
                let value =
                    if let Some(socket) = network.socket_by_fd(owned_fd(self.pid, a[0] as i32)) {
                        let socket = network.sockets.get(&socket).ok_or(EBADF)?;
                        match (a[1], a[2]) {
                            (1, 3) => match socket.kind {
                                SocketKind::Stream => 1,
                                SocketKind::Datagram => 2,
                                SocketKind::Raw => 3,
                            },
                            (1, 4) => 0,
                            (1, 30) => u32::from(socket.listening),
                            (1, 38) => u32::from(socket.protocol),
                            (1, 39) => match socket.domain {
                                SocketDomain::Inet4 => 2,
                                SocketDomain::Inet6 => 10,
                                SocketDomain::Packet => 17,
                                _ => 16,
                            },
                            (0, 8) | (41, 49) => u32::from(socket.receive_packet_info),
                            _ => return Err(ENOTSUP),
                        }
                    } else {
                        if a[1] != 1 || !matches!(a[2], 3 | 4 | 30) {
                            return Err(ENOTSUP);
                        }
                        match a[2] {
                            3 => 1,
                            4 => 0,
                            30 => u32::from(vfs.unix_accepting(a[0] as i32)?),
                            _ => unreachable!(),
                        }
                    };
                memory.write(a[3], &value.to_ne_bytes())?;
                memory.write(a[4], &4u32.to_ne_bytes())?;
                0
            }
            Execve | ExecveAt => {
                let (dirfd, path_pointer, argv, envp, flags) = if matches!(syscall, Execve) {
                    (AT_FDCWD, a[0], a[1], a[2], 0)
                } else {
                    (a[0] as i32, a[1], a[2], a[3], a[4])
                };
                if flags & !(0x100 | 0x1000) != 0 {
                    return Err(EINVAL);
                }
                let path = memory.cstring(path_pointer, 4096)?;
                if path.is_empty() || flags & 0x1000 != 0 {
                    return Err(ENOTSUP);
                }
                let arguments = string_vector(memory, argv, 256)?;
                let environment = string_vector(memory, envp, 256)?
                    .into_iter()
                    .map(|entry| {
                        entry
                            .split_once('=')
                            .map(|(name, value)| (name.into(), value.into()))
                            .ok_or(EINVAL)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let executable: Arc<[u8]> =
                    vfs.read_file_at(dirfd, &path, 256 * 1024 * 1024)?.into();
                let interpreter_path =
                    starnix_kernel::interpreter(&executable).map_err(|_| EINVAL)?;
                let interpreter: Option<Arc<[u8]>> = interpreter_path
                    .as_deref()
                    .map(|path| vfs.read_file(path, 64 * 1024 * 1024).map(Arc::from))
                    .transpose()?;
                let image = starnix_kernel::prepare_image_with_interpreter(
                    &executable,
                    interpreter.as_deref(),
                    self.architecture,
                    &arguments,
                    &environment,
                    self.euid,
                    self.egid,
                )
                .map_err(|_| EINVAL)?;
                match vfs.get_xattr_path(dirfd, &path, false, "security.capability") {
                    Ok(value) => {
                        let (permitted, inheritable, effective) = decode_file_capabilities(&value)?;
                        network
                            .task_capabilities
                            .get_mut(&self.current_tid)
                            .ok_or(EPERM)?
                            .apply_executable(permitted, inheritable, effective)
                            .map_err(net_errno)?;
                    }
                    Err(61) => {}
                    Err(error) => return Err(error),
                }
                return Ok(Outcome::Exec {
                    path,
                    image,
                    executable,
                    interpreter,
                });
            }
            Clone => {
                let (child_tid, tls) = if self.architecture == Architecture::Aarch64 {
                    (a[4], a[3])
                } else {
                    (a[3], a[4])
                };
                return Ok(Outcome::Clone {
                    flags: a[0],
                    stack: a[1],
                    parent_tid: a[2],
                    child_tid,
                    tls,
                });
            }
            Clone3 => {
                if a[1] < 64 || a[1] > 88 {
                    return Err(EINVAL);
                }
                let clone = memory.read(a[0], a[1] as usize)?;
                let stack = abi::read_u64(clone, 40)?
                    .checked_add(abi::read_u64(clone, 48)?)
                    .ok_or(EINVAL)?;
                return Ok(Outcome::Clone {
                    flags: abi::read_u64(clone, 0)?,
                    child_tid: abi::read_u64(clone, 16)?,
                    parent_tid: abi::read_u64(clone, 24)?,
                    stack,
                    tls: abi::read_u64(clone, 56)?,
                });
            }
            Unshare => {
                const CLONE_NEWNET: u64 = 0x4000_0000;
                if a[0] != CLONE_NEWNET {
                    return Err(ENOTSUP);
                }
                network
                    .create_namespace(self.current_tid)
                    .map_err(net_errno)?;
                0
            }
            Setns => {
                const CLONE_NEWNET: u64 = 0x4000_0000;
                if !matches!(a[1], 0 | CLONE_NEWNET) {
                    return Err(EINVAL);
                }
                network
                    .setns(self.current_tid, owned_fd(self.pid, a[0] as i32))
                    .map_err(net_errno)?;
                0
            }
            Capget => {
                let header = memory.read(a[0], 8)?;
                let version = abi::read_u32(header, 0)?;
                let pid = abi::read_u32(header, 4)?;
                if version != 0x2008_0522 || !matches!(pid, 0) && pid != self.current_tid {
                    return Err(EINVAL);
                }
                let capabilities = network
                    .capability_snapshot(self.current_tid)
                    .map_err(net_errno)?;
                let mut data = [0; 24];
                abi::put_u32(&mut data, 0, capabilities.effective as u32);
                abi::put_u32(&mut data, 4, capabilities.permitted as u32);
                abi::put_u32(&mut data, 8, capabilities.inheritable as u32);
                memory.write(a[1], &data)?;
                0
            }
            Capset => {
                let header = memory.read(a[0], 8)?;
                let version = abi::read_u32(header, 0)?;
                let pid = abi::read_u32(header, 4)?;
                if version != 0x2008_0522 || pid != 0 {
                    return Err(EINVAL);
                }
                let data = memory.read(a[1], 24)?;
                let effective = u64::from(abi::read_u32(data, 0)?);
                let permitted = u64::from(abi::read_u32(data, 4)?);
                let inheritable = u64::from(abi::read_u32(data, 8)?);
                network
                    .task_capabilities
                    .get_mut(&self.current_tid)
                    .ok_or(EPERM)?
                    .set(permitted, effective, inheritable)
                    .map_err(net_errno)?;
                0
            }
            Fork => {
                return Ok(Outcome::Clone {
                    flags: 17,
                    stack: 0,
                    parent_tid: 0,
                    child_tid: 0,
                    tls: 0,
                });
            }
            Vfork => {
                return Ok(Outcome::Clone {
                    flags: 0x0000_0100 | 0x0000_4000 | 17,
                    stack: 0,
                    parent_tid: 0,
                    child_tid: 0,
                    tls: 0,
                });
            }
            Wait4 => {
                return Ok(Outcome::Wait {
                    pid: a[0] as i64,
                    status: a[1],
                    options: a[2] as u32,
                    rusage: a[3],
                });
            }
            Exit => {
                return Ok(Outcome::Exit {
                    status: a[0] as u8 as i32,
                    group: false,
                });
            }
            ExitGroup => {
                return Ok(Outcome::Exit {
                    status: a[0] as u8 as i32,
                    group: true,
                });
            }
            Unsupported(_) => return Err(ENOSYS),
        };
        Ok(Outcome::Return(value))
    }

    fn open(
        &self,
        dirfd: i32,
        pointer: u64,
        flags: u64,
        mode: u64,
        memory: &AddressSpace,
        vfs: &mut Vfs,
    ) -> Result<i32, i64> {
        if flags > u64::from(u32::MAX) || mode > u64::from(u32::MAX) {
            return Err(EINVAL);
        }
        let path = memory.cstring(pointer, 4096)?;
        let status = vfs.lstat_path(dirfd, &path);
        if flags & (0x40 | 0x80) == (0x40 | 0x80) && status.is_ok() {
            return Err(EEXIST);
        }
        if flags & 0x2_0000 != 0
            && flags & 0x20_0000 == 0
            && status.as_ref().is_ok_and(|stat| stat.kind == 10)
        {
            return Err(ELOOP);
        }
        if flags & 0x20_0000 == 0 {
            if let Ok(existing) = &status {
                let existing = if existing.kind == 10 && flags & 0x2_0000 == 0 {
                    vfs.stat_path(dirfd, &path)?
                } else {
                    existing.clone()
                };
                let requested = match flags & 3 {
                    0 => 4,
                    1 => 2,
                    2 => 6,
                    _ => return Err(EINVAL),
                };
                self.check_access(&existing, requested, self.fsuid, self.fsgid)?;
            }
        }
        let created = flags & 0x40 != 0 && matches!(status, Err(ENOENT));
        let fd = vfs.open(dirfd, &path, flags as u32)?;
        if created {
            if let Err(error) = self.initialize_created(vfs, dirfd, &path, mode as u32) {
                let _ = vfs.close(fd);
                let _ = vfs.unlink(dirfd, &path, false);
                return Err(error);
            }
        }
        Ok(fd)
    }

    fn initialize_created(&self, vfs: &Vfs, dirfd: i32, path: &str, mode: u32) -> Result<(), i64> {
        vfs.chmod_path(dirfd, path, false, mode & !self.umask)?;
        vfs.chown_path(dirfd, path, false, self.fsuid, self.fsgid)
    }

    fn set_uid(&mut self, requested: u32) -> Result<(), i64> {
        if self.euid == 0 {
            self.uid = requested;
            self.euid = requested;
            self.suid = requested;
            self.fsuid = requested;
            return Ok(());
        }
        if requested == self.uid || requested == self.suid {
            self.euid = requested;
            self.fsuid = requested;
            Ok(())
        } else {
            Err(EPERM)
        }
    }

    fn set_gid(&mut self, requested: u32) -> Result<(), i64> {
        if self.euid == 0 {
            self.gid = requested;
            self.egid = requested;
            self.sgid = requested;
            self.fsgid = requested;
            return Ok(());
        }
        if requested == self.gid || requested == self.sgid {
            self.egid = requested;
            self.fsgid = requested;
            Ok(())
        } else {
            Err(EPERM)
        }
    }

    fn set_res_uid(
        &mut self,
        real: u32,
        effective: u32,
        saved: u32,
        legacy: bool,
    ) -> Result<(), i64> {
        let privileged = self.euid == 0;
        if !privileged {
            let allowed = [self.uid, self.euid, self.suid];
            let real_allowed = real == u32::MAX
                || if legacy {
                    [self.uid, self.euid].contains(&real)
                } else {
                    allowed.contains(&real)
                };
            if !real_allowed
                || effective != u32::MAX && !allowed.contains(&effective)
                || saved != u32::MAX && !allowed.contains(&saved)
            {
                return Err(EPERM);
            }
        }
        let old_real = self.uid;
        if real != u32::MAX {
            self.uid = real;
        }
        if effective != u32::MAX {
            self.euid = effective;
            self.fsuid = effective;
        }
        if saved != u32::MAX {
            self.suid = saved;
        } else if legacy && (real != u32::MAX || effective != u32::MAX && effective != old_real) {
            self.suid = self.euid;
        }
        Ok(())
    }

    fn set_res_gid(
        &mut self,
        real: u32,
        effective: u32,
        saved: u32,
        legacy: bool,
    ) -> Result<(), i64> {
        let privileged = self.euid == 0;
        if !privileged {
            let allowed = [self.gid, self.egid, self.sgid];
            let real_allowed = real == u32::MAX
                || if legacy {
                    [self.gid, self.egid].contains(&real)
                } else {
                    allowed.contains(&real)
                };
            if !real_allowed
                || effective != u32::MAX && !allowed.contains(&effective)
                || saved != u32::MAX && !allowed.contains(&saved)
            {
                return Err(EPERM);
            }
        }
        let old_real = self.gid;
        if real != u32::MAX {
            self.gid = real;
        }
        if effective != u32::MAX {
            self.egid = effective;
            self.fsgid = effective;
        }
        if saved != u32::MAX {
            self.sgid = saved;
        } else if legacy && (real != u32::MAX || effective != u32::MAX && effective != old_real) {
            self.sgid = self.egid;
        }
        Ok(())
    }

    fn write_stat(
        &self,
        stat: crate::vfs::Stat,
        address: u64,
        memory: &mut AddressSpace,
    ) -> Result<(), i64> {
        memory.write(address, &abi::encode_stat(self.architecture, &stat))
    }

    fn write_limit(
        &self,
        resource: u64,
        address: u64,
        memory: &mut AddressSpace,
    ) -> Result<(), i64> {
        let resource = usize::try_from(resource).map_err(|_| EINVAL)?;
        let limit = *self.limits.get(resource).ok_or(EINVAL)?;
        let mut out = [0; 16];
        abi::put_u64(&mut out, 0, limit.0);
        abi::put_u64(&mut out, 8, limit.1);
        memory.write(address, &out)
    }

    fn vectored(
        &self,
        read: bool,
        fd: i32,
        address: u64,
        count: u64,
        memory: &mut AddressSpace,
        vfs: &mut Vfs,
    ) -> Result<u64, i64> {
        if count > 64 {
            return Err(EINVAL);
        }
        let table = memory.read(address, count as usize * 16)?.to_vec();
        let mut total = 0u64;
        for index in 0..count as usize {
            let pointer = abi::read_u64(&table, index * 16)?;
            let length = bounded(abi::read_u64(&table, index * 16 + 8)?)?;
            let n = if read {
                let bytes = vfs.read(fd, length)?;
                memory.write(pointer, &bytes)?;
                bytes.len()
            } else {
                vfs.write(fd, memory.read(pointer, length)?)?
            };
            total = total.saturating_add(n as u64);
            if n != length {
                break;
            }
        }
        Ok(total)
    }

    fn positional_vectored(
        &self,
        read: bool,
        fd: i32,
        address: u64,
        count: u64,
        mut offset: u64,
        memory: &mut AddressSpace,
        vfs: &mut Vfs,
    ) -> Result<u64, i64> {
        if count > 64 {
            return Err(EINVAL);
        }
        let table = memory.read(address, count as usize * 16)?.to_vec();
        let mut total = 0u64;
        for index in 0..count as usize {
            let pointer = abi::read_u64(&table, index * 16)?;
            let length = bounded(abi::read_u64(&table, index * 16 + 8)?)?;
            let n = if read {
                let bytes = vfs.read_at(fd, offset, length)?;
                memory.write(pointer, &bytes)?;
                bytes.len()
            } else {
                vfs.write_at(fd, offset, memory.read(pointer, length)?)?
            };
            total = total.checked_add(n as u64).ok_or(EINVAL)?;
            offset = offset.checked_add(n as u64).ok_or(EINVAL)?;
            if n != length {
                break;
            }
        }
        Ok(total)
    }

    fn flush(
        &self,
        address: u64,
        length: u64,
        memory: &AddressSpace,
        vfs: &mut Vfs,
    ) -> Result<(), i64> {
        for (fd, offset, bytes) in memory.shared_dirty_ranges(address, length) {
            vfs.write_at(fd, offset, bytes)?;
            vfs.sync(fd)?;
        }
        Ok(())
    }

    fn validate_priority_target(&self, which: u64, who: u64) -> Result<(), i64> {
        let matches = match which {
            0 => who == 0 || who == u64::from(self.pid) || who == u64::from(self.current_tid),
            1 => who == 0 || who == u64::from(self.pid),
            2 => who == 0 || who == u64::from(self.uid) || who == u64::from(self.euid),
            _ => return Err(EINVAL),
        };
        if matches {
            Ok(())
        } else {
            Err(starnix_kernel::ESRCH)
        }
    }

    fn check_access(
        &self,
        stat: &crate::vfs::Stat,
        requested: u8,
        uid: u32,
        gid: u32,
    ) -> Result<(), i64> {
        if requested == 0 {
            return Ok(());
        }
        if uid == 0 {
            if requested & 1 != 0 && stat.mode & 0o111 == 0 && stat.mode & 0o170000 != 0o040000 {
                return Err(EACCES);
            }
            return Ok(());
        }
        let shift = if uid == stat.uid {
            6
        } else if gid == stat.gid || self.groups.contains(&stat.gid) {
            3
        } else {
            0
        };
        let permitted = ((stat.mode >> shift) & 7) as u8;
        if requested & !permitted == 0 {
            Ok(())
        } else {
            Err(EACCES)
        }
    }

    fn authorize_chmod(&self, stat: &crate::vfs::Stat) -> Result<(), i64> {
        if self.euid == 0 || self.euid == stat.uid {
            Ok(())
        } else {
            Err(EPERM)
        }
    }

    fn authorize_chown(&self, stat: &crate::vfs::Stat, uid: u32, gid: u32) -> Result<(), i64> {
        if self.euid == 0 {
            return Ok(());
        }
        let owns_file = self.euid == stat.uid;
        let preserves_owner = uid == u32::MAX || uid == stat.uid;
        let allowed_group =
            gid == u32::MAX || gid == stat.gid || gid == self.egid || self.groups.contains(&gid);
        if owns_file && preserves_owner && allowed_group {
            Ok(())
        } else {
            Err(EPERM)
        }
    }

    fn validate_task_target(&self, pid: u64) -> Result<(), i64> {
        if pid == 0 {
            return Ok(());
        }
        let pid = u32::try_from(pid).map_err(|_| starnix_kernel::ESRCH)?;
        if self.process_ids.contains(&pid) || self.task_tids.contains(&pid) {
            Ok(())
        } else {
            Err(starnix_kernel::ESRCH)
        }
    }

    fn ioctl(
        &mut self,
        a: [u64; 6],
        memory: &mut AddressSpace,
        network: &mut NetworkState,
    ) -> Result<u64, i64> {
        match a[1] {
            0x5413 => {
                let mut out = [0; 8];
                abi::put_u16(&mut out, 0, self.window[0]);
                abi::put_u16(&mut out, 2, self.window[1]);
                memory.write(a[2], &out)?;
                Ok(0)
            }
            0x5414 => {
                let bytes = memory.read(a[2], 8)?;
                self.window = [
                    u16::from_ne_bytes(bytes[0..2].try_into().unwrap()),
                    u16::from_ne_bytes(bytes[2..4].try_into().unwrap()),
                ];
                Ok(0)
            }
            0x5401 => {
                let mut out = [0; 36];
                abi::put_u32(&mut out, 12, self.terminal_mode);
                memory.write(a[2], &out)?;
                Ok(0)
            }
            0x5402 | 0x5403 | 0x5404 => {
                self.terminal_mode = abi::read_u32(memory.read(a[2], 36)?, 12)?;
                Ok(0)
            }
            0x5421 => Ok(0),
            0x8912 => {
                let namespace = network
                    .namespace_for_task(self.current_tid)
                    .map_err(net_errno)?;
                let interfaces = &network.namespaces.get(&namespace).ok_or(EINVAL)?.interfaces;
                let requested = abi::read_u32(memory.read(a[2], 16)?, 0)? as usize;
                let pointer = abi::read_u64(memory.read(a[2], 16)?, 8)?;
                let mut records = Vec::new();
                for interface in interfaces.values() {
                    let mut record = [0u8; 40];
                    copy_interface_name(&mut record[..16], &interface.name);
                    if let Some(address) = interface
                        .addresses
                        .iter()
                        .find(|address| address.address.is_ipv4())
                    {
                        record[16..18].copy_from_slice(&2u16.to_ne_bytes());
                        if let IpAddr::V4(address) = address.address {
                            record[20..24].copy_from_slice(&address.octets());
                        }
                    }
                    records.extend_from_slice(&record);
                }
                records.truncate(requested.min(records.len()));
                if pointer != 0 {
                    memory.write(pointer, &records)?;
                }
                memory.write(a[2], &(records.len() as u32).to_ne_bytes())?;
                Ok(0)
            }
            0x8910 | 0x8913 | 0x8914 | 0x8915 | 0x8916 | 0x891b | 0x891c | 0x8921 | 0x8922
            | 0x8927 | 0x8933 => {
                if matches!(a[1], 0x8914 | 0x8916 | 0x891c | 0x8922) {
                    network
                        .require(self.current_tid, bexos_starnix_net::CAP_NET_ADMIN)
                        .map_err(net_errno)?;
                }
                let mut request = memory.read(a[2], 40)?.to_vec();
                let namespace_id = network
                    .namespace_for_task(self.current_tid)
                    .map_err(net_errno)?;
                let namespace = network.namespaces.get_mut(&namespace_id).ok_or(EINVAL)?;
                let interface_id = if a[1] == 0x8910 {
                    u32::from_ne_bytes(request[16..20].try_into().unwrap()) as u64
                } else {
                    let end = request[..16]
                        .iter()
                        .position(|byte| *byte == 0)
                        .unwrap_or(16);
                    let name = core::str::from_utf8(&request[..end]).map_err(|_| EINVAL)?;
                    namespace
                        .interfaces
                        .values()
                        .find(|interface| interface.name == name)
                        .map(|interface| interface.id)
                        .ok_or(starnix_kernel::ENODEV)?
                };
                let interface = namespace
                    .interfaces
                    .get_mut(&interface_id)
                    .ok_or(starnix_kernel::ENODEV)?;
                match a[1] {
                    0x8910 => copy_interface_name(&mut request[..16], &interface.name),
                    0x8913 => {
                        let mut flags = if interface.up { 0x1 | 0x40 } else { 0 };
                        if matches!(interface.backend, InterfaceBackend::Loopback) {
                            flags |= 0x8;
                        }
                        request[16..18].copy_from_slice(&(flags as u16).to_ne_bytes());
                    }
                    0x8914 => {
                        interface.up =
                            u16::from_ne_bytes(request[16..18].try_into().unwrap()) & 1 != 0;
                    }
                    0x8915 => encode_ifreq_ipv4(
                        &mut request,
                        interface
                            .addresses
                            .iter()
                            .find(|address| address.address.is_ipv4())
                            .map(|address| address.address)
                            .ok_or(starnix_kernel::EADDRNOTAVAIL)?,
                    ),
                    0x8916 => {
                        if u16::from_ne_bytes(request[16..18].try_into().unwrap()) != 2 {
                            return Err(EINVAL);
                        }
                        let address = IpAddr::V4(Ipv4Addr::new(
                            request[20],
                            request[21],
                            request[22],
                            request[23],
                        ));
                        if let Some(existing) = interface
                            .addresses
                            .iter_mut()
                            .find(|address| address.address.is_ipv4())
                        {
                            existing.address = address;
                        } else if interface.addresses.len() < 16 {
                            interface.addresses.push(
                                bexos_starnix_net::IpPrefix::new(address, 32).map_err(net_errno)?,
                            );
                        } else {
                            return Err(starnix_kernel::ENOSPC);
                        }
                    }
                    0x891b => {
                        let prefix = interface
                            .addresses
                            .iter()
                            .find(|address| address.address.is_ipv4())
                            .ok_or(starnix_kernel::EADDRNOTAVAIL)?
                            .prefix_len;
                        let mask = if prefix == 0 {
                            0
                        } else {
                            u32::MAX << (32 - prefix)
                        };
                        encode_ifreq_ipv4(&mut request, IpAddr::V4(Ipv4Addr::from(mask)));
                    }
                    0x891c => {
                        if u16::from_ne_bytes(request[16..18].try_into().unwrap()) != 2 {
                            return Err(EINVAL);
                        }
                        let mask = u32::from_be_bytes([
                            request[20],
                            request[21],
                            request[22],
                            request[23],
                        ]);
                        let prefix = mask.leading_ones() as u8;
                        if mask
                            != if prefix == 0 {
                                0
                            } else {
                                u32::MAX << (32 - prefix)
                            }
                        {
                            return Err(EINVAL);
                        }
                        interface
                            .addresses
                            .iter_mut()
                            .find(|address| address.address.is_ipv4())
                            .ok_or(starnix_kernel::EADDRNOTAVAIL)?
                            .prefix_len = prefix;
                    }
                    0x8921 => {
                        request[16..20].copy_from_slice(&(interface.mtu as i32).to_ne_bytes())
                    }
                    0x8922 => {
                        let mtu = i32::from_ne_bytes(request[16..20].try_into().unwrap());
                        if !(68..=65_536).contains(&mtu) {
                            return Err(EINVAL);
                        }
                        interface.mtu = mtu as u32;
                    }
                    0x8927 => {
                        request[16..18].copy_from_slice(&1u16.to_ne_bytes());
                        if let InterfaceBackend::L2 { mac, .. } = &interface.backend {
                            request[18..24].copy_from_slice(mac);
                        } else {
                            request[18..24].fill(0);
                        }
                    }
                    0x8933 => request[16..20].copy_from_slice(&(interface.id as i32).to_ne_bytes()),
                    _ => unreachable!(),
                }
                memory.write(a[2], &request)?;
                Ok(0)
            }
            _ => Err(ENOTSUP),
        }
    }
}

fn string_vector(memory: &AddressSpace, address: u64, maximum: usize) -> Result<Vec<String>, i64> {
    if address == 0 {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for index in 0..=maximum {
        if index == maximum {
            return Err(EINVAL);
        }
        let pointer = abi::read_u64(memory.read(address + index as u64 * 8, 8)?, 0)?;
        if pointer == 0 {
            break;
        }
        values.push(memory.cstring(pointer, 4096)?);
    }
    Ok(values)
}

fn decode_epoll_event(
    architecture: Architecture,
    memory: &AddressSpace,
    address: u64,
) -> Result<(u32, u64), i64> {
    if address == 0 {
        return Err(EINVAL);
    }
    let data_offset = if architecture == Architecture::X86_64 {
        4
    } else {
        8
    };
    let bytes = memory.read(address, data_offset + 8)?;
    Ok((abi::read_u32(bytes, 0)?, abi::read_u64(bytes, data_offset)?))
}

fn write_epoll_events(
    architecture: Architecture,
    memory: &mut AddressSpace,
    address: u64,
    events: &[(u32, u64)],
) -> Result<(), i64> {
    let (stride, data_offset) = if architecture == Architecture::X86_64 {
        (12, 4)
    } else {
        (16, 8)
    };
    let mut bytes = vec![0; events.len() * stride];
    for (index, (flags, data)) in events.iter().enumerate() {
        let offset = index * stride;
        abi::put_u32(&mut bytes, offset, *flags);
        abi::put_u64(&mut bytes, offset + data_offset, *data);
    }
    memory.write(address, &bytes)
}

fn net_errno(error: NetError) -> i64 {
    match error {
        NetError::InvalidArgument | NetError::CorruptSnapshot => EINVAL,
        NetError::NotFound => ENOENT,
        NetError::AlreadyExists => EEXIST,
        NetError::PermissionDenied => EPERM,
        NetError::Unsupported | NetError::UnsupportedSnapshot => ENOTSUP,
        NetError::ResourceExhausted => ENOMEM,
        NetError::WouldBlock => starnix_kernel::EAGAIN,
        NetError::NetworkUnreachable => starnix_kernel::ENETUNREACH,
    }
}

fn decode_itimerspec(memory: &AddressSpace, address: u64) -> Result<(u64, u64), i64> {
    let bytes = memory.read(address, 32)?;
    let interval = abi::decode_timespec(&bytes[..16])?;
    let value = abi::decode_timespec(&bytes[16..])?;
    Ok((value, interval))
}

fn encode_itimerspec(value: u64, interval: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(&abi::encode_timespec(interval));
    bytes.extend_from_slice(&abi::encode_timespec(value));
    bytes
}

fn decode_unix_address(memory: &AddressSpace, address: u64, length: u64) -> Result<String, i64> {
    let length = usize::try_from(length).map_err(|_| EINVAL)?;
    if address == 0 || !(3..=110).contains(&length) {
        return Err(EINVAL);
    }
    let bytes = memory.read(address, length)?;
    if u16::from_ne_bytes(bytes[..2].try_into().unwrap()) != 1 {
        return Err(EAFNOSUPPORT);
    }
    let raw = &bytes[2..];
    if raw[0] == 0 {
        let name = core::str::from_utf8(&raw[1..]).map_err(|_| EINVAL)?;
        if name.is_empty() || name.contains('\0') {
            return Err(EINVAL);
        }
        return Ok(format!("@{name}"));
    }
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    let path = core::str::from_utf8(&raw[..end]).map_err(|_| EINVAL)?;
    if path.is_empty() {
        return Err(EINVAL);
    }
    Ok(path.into())
}

fn write_unix_address(
    memory: &mut AddressSpace,
    address: u64,
    length_address: u64,
    name: &str,
) -> Result<(), i64> {
    if address == 0 || length_address == 0 {
        return Err(EINVAL);
    }
    let capacity =
        usize::try_from(abi::read_u32(memory.read(length_address, 4)?, 0)?).map_err(|_| EINVAL)?;
    let mut bytes = Vec::with_capacity(110);
    bytes.extend_from_slice(&1u16.to_ne_bytes());
    if let Some(abstract_name) = name.strip_prefix('@') {
        bytes.push(0);
        bytes.extend_from_slice(abstract_name.as_bytes());
    } else if !name.is_empty() {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
    }
    if bytes.len() > 110 {
        return Err(EINVAL);
    }
    let actual = bytes.len() as u32;
    memory.write(address, &bytes[..capacity.min(bytes.len())])?;
    memory.write(length_address, &actual.to_ne_bytes())
}

fn linux_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("linux");
    let mut end = name.len().min(15);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].into()
}

fn bounded(value: u64) -> Result<usize, i64> {
    let value = usize::try_from(value).map_err(|_| EINVAL)?;
    if value > 16 * 1024 * 1024 {
        Err(ENOMEM)
    } else {
        Ok(value)
    }
}

fn futex_operation(raw: u64) -> Result<(u64, bool, bool), i64> {
    const COMMAND_MASK: u64 = 0x7f;
    const PRIVATE: u64 = 0x80;
    const CLOCK_REALTIME: u64 = 0x100;
    if raw & !(COMMAND_MASK | PRIVATE | CLOCK_REALTIME) != 0 {
        return Err(EINVAL);
    }
    let command = raw & COMMAND_MASK;
    let realtime = raw & CLOCK_REALTIME != 0;
    if realtime && !matches!(command, 0 | 9 | 11 | 13) {
        return Err(EINVAL);
    }
    Ok((command, raw & PRIVATE != 0, realtime))
}

fn signed_12(value: u32) -> i32 {
    ((value << 20) as i32) >> 20
}

fn decode_futex_wake_operation(
    encoded: u32,
) -> Result<(FutexAtomicOperation, i32, FutexComparison, i32), i64> {
    const OPARG_SHIFT: u32 = 8;
    let encoded_operation = (encoded >> 28) & 0xf;
    let mut operand = signed_12((encoded >> 12) & 0xfff);
    let operation = match encoded_operation & !OPARG_SHIFT {
        0 => FutexAtomicOperation::Set,
        1 => FutexAtomicOperation::Add,
        2 => FutexAtomicOperation::Or,
        3 => FutexAtomicOperation::AndNot,
        4 => FutexAtomicOperation::Xor,
        _ => return Err(ENOSYS),
    };
    if encoded_operation & OPARG_SHIFT != 0 {
        if !(0..32).contains(&operand) {
            return Err(EINVAL);
        }
        operand = (1u32 << operand) as i32;
    }
    let comparison = match (encoded >> 24) & 0xf {
        0 => FutexComparison::Equal,
        1 => FutexComparison::NotEqual,
        2 => FutexComparison::Less,
        3 => FutexComparison::LessOrEqual,
        4 => FutexComparison::Greater,
        5 => FutexComparison::GreaterOrEqual,
        _ => return Err(ENOSYS),
    };
    Ok((operation, operand, comparison, signed_12(encoded & 0xfff)))
}

fn credential(input: &mut Decoder<'_>) -> Result<u32, MigrationError> {
    u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)
}

fn write_sized(
    memory: &mut AddressSpace,
    pointer: u64,
    capacity: u64,
    bytes: &[u8],
) -> Result<u64, i64> {
    if capacity == 0 {
        return Ok(bytes.len() as u64);
    }
    if capacity < bytes.len() as u64 {
        return Err(starnix_kernel::ERANGE);
    }
    memory.write(pointer, bytes)?;
    Ok(bytes.len() as u64)
}

fn encode_action(action: SignalAction) -> [u8; 32] {
    let mut out = [0; 32];
    abi::put_u64(&mut out, 0, action.handler);
    abi::put_u64(&mut out, 8, action.flags);
    abi::put_u64(&mut out, 16, action.restorer);
    abi::put_u64(&mut out, 24, action.mask);
    out
}

fn decode_action(bytes: &[u8]) -> Result<SignalAction, i64> {
    Ok(SignalAction {
        handler: abi::read_u64(bytes, 0)?,
        flags: abi::read_u64(bytes, 8)?,
        restorer: abi::read_u64(bytes, 16)?,
        mask: abi::read_u64(bytes, 24)?,
    })
}

fn encode_altstack(stack: AltStack) -> [u8; 24] {
    let mut out = [0; 24];
    abi::put_u64(&mut out, 0, stack.address);
    abi::put_u32(&mut out, 8, stack.flags);
    abi::put_u64(&mut out, 16, stack.size);
    out
}

fn decode_altstack(bytes: &[u8]) -> Result<AltStack, i64> {
    Ok(AltStack {
        address: abi::read_u64(bytes, 0)?,
        flags: abi::read_u32(bytes, 8)?,
        size: abi::read_u64(bytes, 16)?,
    })
}

fn decode_file_capabilities(bytes: &[u8]) -> Result<(u64, u64, bool), i64> {
    if bytes.len() < 12 {
        return Err(EINVAL);
    }
    let magic = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let revision = magic & 0xff00_0000;
    let effective = magic & 1 != 0;
    let permitted_low = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as u64;
    let inheritable_low = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as u64;
    match revision {
        0x0100_0000 if bytes.len() == 12 => Ok((permitted_low, inheritable_low, effective)),
        0x0200_0000 | 0x0300_0000 if bytes.len() >= 20 => {
            let permitted_high = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as u64;
            let inheritable_high = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as u64;
            Ok((
                permitted_low | permitted_high << 32,
                inheritable_low | inheritable_high << 32,
                effective,
            ))
        }
        _ => Err(EINVAL),
    }
}

#[derive(Clone, Copy)]
enum SocketBackend {
    Direct {
        provider: u64,
        host_interface: Option<u64>,
    },
    L2 {
        interface: u64,
    },
    Loopback {
        namespace: u64,
    },
}

fn is_icmp_socket(socket: &bexos_starnix_net::SocketState) -> bool {
    matches!(
        (socket.domain, socket.protocol),
        (SocketDomain::Inet4, 1) | (SocketDomain::Inet6, 58)
    ) && matches!(socket.kind, SocketKind::Datagram | SocketKind::Raw)
}

fn backend_for_socket(
    network: &NetworkState,
    socket: u64,
    destination: Option<IpAddr>,
) -> Result<SocketBackend, i64> {
    let socket = network.sockets.get(&socket).ok_or(EBADF)?;
    let namespace = network
        .namespaces
        .get(&socket.namespace)
        .ok_or(starnix_kernel::ENETUNREACH)?;
    let interface_id = if let Some(bound) = socket.bound_interface {
        bound
    } else if let Some(destination) = destination {
        namespace
            .route(destination, None)
            .map(|route| route.interface_id)
            .ok_or(starnix_kernel::ENETUNREACH)?
    } else {
        namespace
            .interfaces
            .values()
            .find(|interface| {
                interface.up && !matches!(interface.backend, InterfaceBackend::Loopback)
            })
            .map(|interface| interface.id)
            .ok_or(starnix_kernel::ENETUNREACH)?
    };
    match &namespace
        .interfaces
        .get(&interface_id)
        .ok_or(starnix_kernel::ENETUNREACH)?
        .backend
    {
        InterfaceBackend::Direct {
            provider,
            host_interface_id,
            ..
        } => Ok(SocketBackend::Direct {
            provider: *provider,
            host_interface: (*host_interface_id != 0).then_some(*host_interface_id),
        }),
        InterfaceBackend::L2 { .. } => Ok(SocketBackend::L2 {
            interface: interface_id,
        }),
        InterfaceBackend::Loopback => Ok(SocketBackend::Loopback {
            namespace: socket.namespace,
        }),
        _ => Err(ENOTSUP),
    }
}

fn ensure_udp(
    network: &mut NetworkState,
    l2: &mut crate::l2_backend::L2Runtime,
    socket: u64,
    destination: Option<IpAddr>,
) -> Result<(), i64> {
    let snapshot = network.sockets.get(&socket).cloned().ok_or(EBADF)?;
    if snapshot.backend_control != 0 {
        return Ok(());
    }
    if !matches!(snapshot.kind, SocketKind::Datagram) {
        return Err(ENOTSUP);
    }
    let local = snapshot
        .local
        .unwrap_or_else(|| unspecified(snapshot.domain));
    let route_destination = destination
        .or_else(|| snapshot.peer.map(|peer| peer.0))
        .or_else(|| (!local.0.is_unspecified()).then_some(local.0));
    let control = match backend_for_socket(network, socket, route_destination)? {
        SocketBackend::Direct {
            provider,
            host_interface,
        } => crate::net_backend::create_udp(provider, local, snapshot.nonblocking, host_interface)?,
        SocketBackend::L2 { interface } => l2.create_udp(interface, local)?,
        SocketBackend::Loopback { namespace } => l2.create_loopback_udp(namespace, local)?,
    };
    network
        .sockets
        .get_mut(&socket)
        .ok_or(EBADF)?
        .backend_control = control;
    Ok(())
}

fn owned_fd(pid: u32, fd: i32) -> u64 {
    (u64::from(pid) << 32) | u64::from(fd as u32)
}

fn closing_backend(network: &NetworkState, fd: u64) -> Option<u64> {
    let socket = network.socket_by_fd(fd)?;
    let state = network.sockets.get(&socket)?;
    (state.linux_fds.len() == 1 && state.backend_control != 0).then_some(state.backend_control)
}

fn close_backend(l2: &mut crate::l2_backend::L2Runtime, backend: Option<u64>) {
    let Some(backend) = backend else { return };
    if crate::l2_backend::L2Runtime::is_object(backend) {
        l2.close(backend);
    } else {
        let _ = bexos_userspace::Memory::close(backend);
    }
}

fn bind_netns_path(
    network: &mut NetworkState,
    tid: u32,
    pid: u32,
    fd: i32,
    path: &str,
) -> Result<(), i64> {
    if !path.ends_with("/ns/net") || !path.starts_with("/proc/") {
        return Ok(());
    }
    let owner = path
        .strip_prefix("/proc/")
        .and_then(|path| path.strip_suffix("/ns/net"))
        .ok_or(EINVAL)?;
    let target_tid = match owner {
        "self" | "thread-self" => tid,
        value => value.parse::<u32>().map_err(|_| EINVAL)?,
    };
    let namespace = network.namespace_for_task(target_tid).map_err(net_errno)?;
    network
        .bind_namespace_fd(owned_fd(pid, fd), namespace)
        .map_err(net_errno)
}

fn clone_or_close_netns_fd(
    network: &mut NetworkState,
    source: u64,
    target: u64,
) -> Result<(), i64> {
    if network.namespace_fds.contains_key(&source) {
        network
            .clone_namespace_fd(source, target)
            .map_err(net_errno)
    } else if network.namespace_fds.contains_key(&target) {
        network.close_namespace_fd(target).map_err(net_errno)
    } else {
        Ok(())
    }
}

fn refresh_network_files(vfs: &mut Vfs, network: &NetworkState, tid: u32, pid: u32) {
    let Ok(namespace_id) = network.namespace_for_task(tid) else {
        return;
    };
    let Some(namespace) = network.namespaces.get(&namespace_id) else {
        return;
    };
    vfs.set_synthetic_directory(
        "/proc/net",
        vec!["dev".into(), "if_inet6".into(), "route".into()],
    );
    let mut dev = String::from(
        "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
    );
    let mut route = String::from(
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
    );
    let mut inet6 = String::new();
    let mut interface_names = Vec::new();
    for interface in namespace.interfaces.values() {
        interface_names.push(interface.name.clone());
        dev.push_str(&format!(
            "{:>6}: {:8} {:7}    0    0    0     0          0         0 {:8} {:7}    0    0    0     0       0          0\n",
            interface.name, 0, 0, 0, 0
        ));
        for address in &interface.addresses {
            if let IpAddr::V6(ipv6) = address.address {
                let hex = ipv6
                    .octets()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                inet6.push_str(&format!(
                    "{hex} {:02x} {:02x} 00 80 {}\n",
                    interface.id, address.prefix_len, interface.name
                ));
            }
        }
        vfs.set_synthetic(
            &format!("/sys/class/net/{}/ifindex", interface.name),
            format!("{}\n", interface.id).into_bytes(),
        );
        vfs.set_synthetic(
            &format!("/sys/class/net/{}/mtu", interface.name),
            format!("{}\n", interface.mtu).into_bytes(),
        );
        vfs.set_synthetic(
            &format!("/sys/class/net/{}/operstate", interface.name),
            if interface.up {
                b"up\n".to_vec()
            } else {
                b"down\n".to_vec()
            },
        );
        let mac = match &interface.backend {
            InterfaceBackend::L2 { mac, .. } => *mac,
            _ => [0; 6],
        };
        vfs.set_synthetic(
            &format!("/sys/class/net/{}/address", interface.name),
            format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            )
            .into_bytes(),
        );
        vfs.set_synthetic_directory(
            &format!("/sys/class/net/{}", interface.name),
            vec![
                "address".into(),
                "ifindex".into(),
                "mtu".into(),
                "operstate".into(),
            ],
        );
    }
    for item in &namespace.routes {
        if let (IpAddr::V4(destination), Some(interface)) = (
            item.destination.address,
            namespace.interfaces.get(&item.interface_id),
        ) {
            let gateway = match item.gateway {
                Some(IpAddr::V4(value)) => u32::from_le_bytes(value.octets()),
                _ => 0,
            };
            let mask = if item.destination.prefix_len == 0 {
                0
            } else {
                u32::MAX << (32 - item.destination.prefix_len)
            };
            route.push_str(&format!(
                "{}\t{:08X}\t{:08X}\t0003\t0\t0\t{}\t{:08X}\t0\t{}\t0\t0\n",
                interface.name,
                u32::from_le_bytes(destination.octets()),
                gateway,
                item.metric,
                u32::from_be(mask),
                interface.mtu
            ));
        }
    }
    vfs.set_synthetic("/proc/net/dev", dev.into_bytes());
    vfs.set_synthetic("/proc/net/route", route.into_bytes());
    vfs.set_synthetic("/proc/net/if_inet6", inet6.into_bytes());
    let resolv = namespace
        .dns_servers
        .iter()
        .map(|server| format!("nameserver {server}\n"))
        .collect::<String>();
    vfs.set_synthetic("/etc/resolv.conf", resolv.into_bytes());
    vfs.set_synthetic_directory("/sys/class/net", interface_names);
    for owner in [
        "self".to_string(),
        "thread-self".to_string(),
        pid.to_string(),
    ] {
        vfs.set_synthetic_directory(&format!("/proc/{owner}/ns"), vec!["net".into()]);
        vfs.set_synthetic(
            &format!("/proc/{owner}/ns/net"),
            format!("net:[{namespace_id}]\n").into_bytes(),
        );
    }
}

fn unspecified(domain: SocketDomain) -> (IpAddr, u16) {
    match domain {
        SocketDomain::Inet6 => (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        _ => (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
    }
}

fn decode_icmp_frame(frame: &[u8]) -> Result<(Vec<u8>, IpAddr), i64> {
    if frame.len() >= 42 && frame[12..14] == 0x0800u16.to_be_bytes() && frame[14] >> 4 == 4 {
        let header = usize::from(frame[14] & 0x0f) * 4;
        let total = u16::from_be_bytes([frame[16], frame[17]]) as usize;
        if header < 20 || frame[23] != 1 || total < header + 8 || frame.len() < 14 + total {
            return Err(starnix_kernel::EIO);
        }
        return Ok((
            frame[14 + header..14 + total].to_vec(),
            IpAddr::V4(Ipv4Addr::new(frame[26], frame[27], frame[28], frame[29])),
        ));
    }
    if frame.len() >= 62 && frame[12..14] == 0x86ddu16.to_be_bytes() && frame[14] >> 4 == 6 {
        let payload = u16::from_be_bytes([frame[18], frame[19]]) as usize;
        if frame[20] != 58 || payload < 8 || frame.len() < 54 + payload {
            return Err(starnix_kernel::EIO);
        }
        return Ok((
            frame[54..54 + payload].to_vec(),
            IpAddr::V6(Ipv6Addr::from(
                <[u8; 16]>::try_from(&frame[22..38]).unwrap(),
            )),
        ));
    }
    Err(starnix_kernel::EIO)
}

fn decode_ip_address(
    memory: &AddressSpace,
    pointer: u64,
    length: u64,
    domain: SocketDomain,
) -> Result<(IpAddr, u16), i64> {
    let family = match domain {
        SocketDomain::Inet4 => 2u16,
        SocketDomain::Inet6 => 10,
        _ => return Err(EAFNOSUPPORT),
    };
    let required = if family == 2 { 16 } else { 28 };
    if pointer == 0 || length < required {
        return Err(EINVAL);
    }
    let bytes = memory.read(pointer, required as usize)?;
    if u16::from_ne_bytes(bytes[..2].try_into().unwrap()) != family {
        return Err(EAFNOSUPPORT);
    }
    let port = u16::from_be_bytes(bytes[2..4].try_into().unwrap());
    Ok(if family == 2 {
        (
            IpAddr::V4(Ipv4Addr::new(bytes[4], bytes[5], bytes[6], bytes[7])),
            port,
        )
    } else {
        (
            IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[8..24]).unwrap())),
            port,
        )
    })
}

fn write_ip_address(
    memory: &mut AddressSpace,
    pointer: u64,
    length_pointer: u64,
    address: (IpAddr, u16),
) -> Result<(), i64> {
    if pointer == 0 || length_pointer == 0 {
        return Err(EINVAL);
    }
    let mut bytes = match address.0 {
        IpAddr::V4(ip) => {
            let mut bytes = vec![0; 16];
            bytes[..2].copy_from_slice(&2u16.to_ne_bytes());
            bytes[2..4].copy_from_slice(&address.1.to_be_bytes());
            bytes[4..8].copy_from_slice(&ip.octets());
            bytes
        }
        IpAddr::V6(ip) => {
            let mut bytes = vec![0; 28];
            bytes[..2].copy_from_slice(&10u16.to_ne_bytes());
            bytes[2..4].copy_from_slice(&address.1.to_be_bytes());
            bytes[8..24].copy_from_slice(&ip.octets());
            bytes
        }
    };
    let supplied = abi::read_u32(memory.read(length_pointer, 4)?, 0)? as usize;
    let actual = bytes.len();
    bytes.truncate(supplied.min(actual));
    memory.write(pointer, &bytes)?;
    memory.write(length_pointer, &(actual as u32).to_ne_bytes())?;
    Ok(())
}

fn write_packet_info_control(
    memory: &mut AddressSpace,
    message_header: u64,
    network: &NetworkState,
    socket: u64,
    enabled: bool,
) -> Result<(), i64> {
    let header = memory.read(message_header, 56)?.to_vec();
    let pointer = abi::read_u64(&header, 32)?;
    let capacity = abi::read_u64(&header, 40)? as usize;
    if !enabled || pointer == 0 || capacity == 0 {
        memory.write(message_header + 40, &0u64.to_ne_bytes())?;
        return Ok(());
    }
    let state = network.sockets.get(&socket).ok_or(EBADF)?;
    let (interface, destination) = network.packet_info(socket).map_err(net_errno)?;
    let mut control = Vec::new();
    match destination {
        IpAddr::V4(address) => {
            // cmsghdr followed by Linux in_pktinfo. cmsg_len excludes the
            // trailing alignment but includes the header and payload.
            control.extend_from_slice(&28u64.to_ne_bytes());
            control.extend_from_slice(&0i32.to_ne_bytes()); // IPPROTO_IP
            control.extend_from_slice(&8i32.to_ne_bytes()); // IP_PKTINFO
            control.extend_from_slice(&(interface as u32).to_ne_bytes());
            control.extend_from_slice(&address.octets());
            control.extend_from_slice(&address.octets());
            control.resize(32, 0);
        }
        IpAddr::V6(address) => {
            control.extend_from_slice(&36u64.to_ne_bytes());
            control.extend_from_slice(&41i32.to_ne_bytes()); // IPPROTO_IPV6
            control.extend_from_slice(&50i32.to_ne_bytes()); // IPV6_PKTINFO
            control.extend_from_slice(&address.octets());
            control.extend_from_slice(&(interface as u32).to_ne_bytes());
            control.resize(40, 0);
        }
    }
    if (matches!(state.domain, SocketDomain::Inet4) && control.len() != 32)
        || (matches!(state.domain, SocketDomain::Inet6) && control.len() != 40)
    {
        return Err(EINVAL);
    }
    let written = capacity.min(control.len());
    memory.write(pointer, &control[..written])?;
    memory.write(message_header + 40, &(written as u64).to_ne_bytes())?;
    if written < control.len() {
        let flags = abi::read_u32(&header, 48)? | 0x8; // MSG_CTRUNC
        memory.write(message_header + 48, &flags.to_ne_bytes())?;
    }
    Ok(())
}

fn format_socket_address(address: (IpAddr, u16)) -> String {
    match address.0 {
        IpAddr::V4(ip) => format!("{ip}:{}", address.1),
        IpAddr::V6(ip) => format!("[{ip}]:{}", address.1),
    }
}

fn read_iovecs(memory: &AddressSpace, address: u64, count: u64) -> Result<Vec<u8>, i64> {
    if count > 64 {
        return Err(EINVAL);
    }
    let table = memory.read(address, count as usize * 16)?;
    let mut output = Vec::new();
    for index in 0..count as usize {
        let pointer = abi::read_u64(table, index * 16)?;
        let length = bounded(abi::read_u64(table, index * 16 + 8)?)?;
        if output.len().checked_add(length).ok_or(EINVAL)? > 65_507 {
            return Err(EINVAL);
        }
        output.extend_from_slice(memory.read(pointer, length)?);
    }
    Ok(output)
}

fn write_iovecs(
    memory: &mut AddressSpace,
    address: u64,
    count: u64,
    bytes: &[u8],
) -> Result<usize, i64> {
    if count > 64 {
        return Err(EINVAL);
    }
    let table = memory.read(address, count as usize * 16)?.to_vec();
    let mut written = 0;
    for index in 0..count as usize {
        if written == bytes.len() {
            break;
        }
        let pointer = abi::read_u64(&table, index * 16)?;
        let length = bounded(abi::read_u64(&table, index * 16 + 8)?)?;
        let take = length.min(bytes.len() - written);
        memory.write(pointer, &bytes[written..written + take])?;
        written += take;
    }
    Ok(written)
}

fn copy_interface_name(output: &mut [u8], name: &str) {
    output.fill(0);
    let bytes = name.as_bytes();
    let length = bytes.len().min(output.len().saturating_sub(1));
    output[..length].copy_from_slice(&bytes[..length]);
}

fn encode_ifreq_ipv4(output: &mut [u8], address: IpAddr) {
    output[16..32].fill(0);
    output[16..18].copy_from_slice(&2u16.to_ne_bytes());
    if let IpAddr::V4(address) = address {
        output[20..24].copy_from_slice(&address.octets());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_and_altstack_wire_layouts_round_trip() {
        let action = SignalAction {
            handler: 1,
            flags: 2,
            restorer: 3,
            mask: 4,
        };
        assert_eq!(decode_action(&encode_action(action)).unwrap(), action);
        let stack = AltStack {
            address: 5,
            size: 8192,
            flags: 0,
        };
        assert_eq!(decode_altstack(&encode_altstack(stack)).unwrap(), stack);
    }

    #[test]
    fn transfers_are_bounded() {
        assert_eq!(bounded(4096), Ok(4096));
        assert_eq!(bounded(17 * 1024 * 1024), Err(ENOMEM));
    }

    #[test]
    fn futex_operation_validates_flags_and_pi_clock_rules() {
        assert_eq!(futex_operation(6), Ok((6, false, false)));
        assert_eq!(futex_operation(6 | 0x80), Ok((6, true, false)));
        assert_eq!(futex_operation(13 | 0x100), Ok((13, false, true)));
        assert_eq!(futex_operation(11 | 0x100), Ok((11, false, true)));
        assert_eq!(futex_operation(9 | 0x180), Ok((9, true, true)));
        assert_eq!(futex_operation(6 | 0x100), Err(EINVAL));
        assert_eq!(futex_operation(12 | 0x100), Err(EINVAL));
        assert_eq!(futex_operation(1 << 20), Err(EINVAL));
    }

    #[test]
    fn futex_wake_operation_decodes_signed_and_shifted_operands() {
        assert_eq!(
            decode_futex_wake_operation((1 << 28) | (4 << 24) | (0xfff << 12) | 7),
            Ok((FutexAtomicOperation::Add, -1, FutexComparison::Greater, 7,))
        );
        assert_eq!(
            decode_futex_wake_operation((10 << 28) | (1 << 24) | (5 << 12) | 0xfff),
            Ok((FutexAtomicOperation::Or, 32, FutexComparison::NotEqual, -1,))
        );
        assert_eq!(decode_futex_wake_operation(5 << 28), Err(ENOSYS));
        assert_eq!(decode_futex_wake_operation(6 << 24), Err(ENOSYS));
        assert_eq!(
            decode_futex_wake_operation((8 << 28) | (32 << 12)),
            Err(EINVAL)
        );
    }

    #[test]
    fn credential_state_survives_checkpoint_and_enforces_unprivileged_changes() {
        let mut dispatcher = Dispatcher::new(Architecture::X86_64, &NixRunnerOptions::default());
        dispatcher.groups = vec![10, 20];
        dispatcher.set_res_gid(30, 31, 32, false).unwrap();
        dispatcher.set_res_uid(40, 41, 42, false).unwrap();
        dispatcher.fsuid = 43;
        dispatcher.fsgid = 33;
        dispatcher.nice = 7;

        assert_eq!(dispatcher.set_uid(99), Err(EPERM));
        assert_eq!(dispatcher.set_uid(40), Ok(()));
        let restored = Dispatcher::restore(Architecture::X86_64, &dispatcher.checkpoint()).unwrap();
        assert_eq!(
            (
                restored.uid,
                restored.euid,
                restored.suid,
                restored.fsuid,
                restored.gid,
                restored.egid,
                restored.sgid,
                restored.fsgid,
                restored.groups,
                restored.nice,
            ),
            (40, 40, 42, 40, 30, 31, 32, 33, vec![10, 20], 7)
        );
    }

    #[test]
    fn proc_identity_files_follow_the_active_process_and_thread() {
        let mut dispatcher = Dispatcher::new(Architecture::X86_64, &NixRunnerOptions::default());
        dispatcher.task_name = "worker".into();
        dispatcher.groups = vec![12, 34];
        dispatcher.set_process_context(7, 3, &[1, 3, 7]);
        dispatcher.set_task_context(9, &[7, 9]);
        let status = String::from_utf8(dispatcher.proc_status(9)).unwrap();
        assert!(status.contains("Name:\tworker\n"));
        assert!(status.contains("Tgid:\t7\nPid:\t9\nPPid:\t3\n"));
        assert!(status.contains("Groups:\t12 34\nThreads:\t2\n"));
        let stat = String::from_utf8(dispatcher.proc_stat(9)).unwrap();
        assert!(stat.starts_with("9 (worker) R 3 7 7 "));
    }

    #[test]
    fn access_checks_linux_owner_group_other_and_root_execute_rules() {
        let mut dispatcher = Dispatcher::new(Architecture::X86_64, &NixRunnerOptions::default());
        let mut stat = crate::vfs::Stat {
            inode: 1,
            mode: 0o100640,
            uid: 10,
            gid: 20,
            links: 1,
            size: 0,
            blocks: 0,
            created: 0,
            modified: 0,
            kind: 8,
        };
        assert_eq!(dispatcher.check_access(&stat, 6, 10, 99), Ok(()));
        assert_eq!(dispatcher.check_access(&stat, 2, 11, 20), Err(EACCES));
        dispatcher.groups.push(20);
        assert_eq!(dispatcher.check_access(&stat, 4, 11, 99), Ok(()));
        assert_eq!(dispatcher.check_access(&stat, 1, 0, 0), Err(EACCES));
        stat.mode |= 0o100;
        assert_eq!(dispatcher.check_access(&stat, 1, 0, 0), Ok(()));

        dispatcher.euid = 11;
        dispatcher.egid = 30;
        assert_eq!(dispatcher.authorize_chmod(&stat), Err(EPERM));
        assert_eq!(dispatcher.authorize_chown(&stat, u32::MAX, 20), Err(EPERM));
        dispatcher.euid = 10;
        assert_eq!(dispatcher.authorize_chmod(&stat), Ok(()));
        assert_eq!(dispatcher.authorize_chown(&stat, u32::MAX, 30), Ok(()));
        assert_eq!(dispatcher.authorize_chown(&stat, 11, u32::MAX), Err(EPERM));
    }
}
