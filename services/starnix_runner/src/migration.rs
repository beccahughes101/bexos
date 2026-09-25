use crate::vfs::{
    DescriptorSnapshot, Snapshot as VfsSnapshot, synthetic_directory, synthetic_file,
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_starnix_abi::{ABI_VERSION, NixRunnerOptions, UPSTREAM_REVISION};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use std::{string::String, vec::Vec};

const RECORD_VERSION: u64 = 4;
const MAX_MAPPINGS: usize = 96;
const MAX_REGISTER_BYTES: usize = 1024;
const MAX_RUNTIME_BYTES: usize = 512 * 1024;
const MAX_FDS: usize = 256;
const MAX_PROCESSES: usize = 64;
const MAX_STARTUP_WRITES: usize = 32;
const MAX_SYNTHETIC_BYTES: usize = 256 * 1024;

pub(crate) struct RuntimeState {
    pub signals: Vec<u8>,
    pub dispatcher: Vec<u8>,
    pub vfs: VfsSnapshot,
    pub signal_frames: Vec<(u64, u64, u64)>,
    pub tasks: Vec<TaskSnapshot>,
    pub current_task: usize,
    pub next_tid: u32,
    pub pid: u32,
    pub ppid: u32,
    pub exit_signal: u32,
    pub vfork_parent: Option<u32>,
    pub startup_writes: Vec<(u64, Vec<u8>)>,
    pub processes: Vec<ProcessSnapshot>,
    pub zombies: Vec<(u32, u32, i32)>,
    pub next_pid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskSnapshot {
    pub tid: u32,
    pub registers: Vec<u8>,
    pub clear_tid: u64,
    pub robust_list: u64,
    pub rseq: Option<(u64, u32, u32)>,
    pub futex: Option<(u64, Option<u64>, bool, u32)>,
    pub pi_futex: Option<(u64, Option<u64>, bool, u32, u64)>,
    pub futex_waitv: Option<(Vec<(u64, bool)>, Option<u64>)>,
    pub pi_requeue: Option<(u64, u64, Option<u64>, bool, u64)>,
    pub sleep_deadline: Option<u64>,
    pub signal_frames: Vec<(u64, u64, u64)>,
    pub wait: Option<(i64, u64, u64)>,
    pub wait_ready: Option<(u32, u64, u64, i32)>,
    pub vfork_child: Option<u32>,
    pub signal_wait: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessMapping {
    pub handle: u64,
    pub futex_id: u64,
    pub address: u64,
    pub size: u64,
    pub offset: u64,
    pub rights: u32,
    pub kind: u64,
    pub file_fd: i32,
    pub file_offset: u64,
    pub shared: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessSnapshot {
    pub pid: u32,
    pub ppid: u32,
    pub exit_signal: u32,
    pub vfork_parent: Option<u32>,
    pub runtime: Vec<u8>,
    pub mappings: Vec<ProcessMapping>,
    pub startup_writes: Vec<(u64, Vec<u8>)>,
}

impl RuntimeState {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = Encoder::new();
        out.word(15);
        out.bytes(&self.signals);
        out.bytes(&self.dispatcher);
        out.text(&self.vfs.cwd);
        out.word(self.vfs.descriptors.len() as u64);
        for (fd, descriptor) in &self.vfs.descriptors {
            out.word(u64::from(*fd));
            match descriptor {
                DescriptorSnapshot::Stdio { source } => {
                    out.word(0);
                    out.word(u64::from(*source));
                }
                DescriptorSnapshot::File {
                    path,
                    flags,
                    offset,
                } => {
                    out.word(1);
                    out.text(path);
                    out.word(u64::from(*flags));
                    out.word(*offset);
                }
                DescriptorSnapshot::Directory { path, flags } => {
                    out.word(2);
                    out.text(path);
                    out.word(u64::from(*flags));
                }
                DescriptorSnapshot::Null { flags } => {
                    out.word(3);
                    out.word(u64::from(*flags));
                }
                DescriptorSnapshot::Zero { flags } => {
                    out.word(4);
                    out.word(u64::from(*flags));
                }
                DescriptorSnapshot::Socket {
                    handle,
                    flags,
                    readable,
                    writable,
                    local,
                    peer,
                } => {
                    out.word(5);
                    out.word(*handle);
                    out.word(u64::from(*flags));
                    out.word(u64::from(*readable));
                    out.word(u64::from(*writable));
                    out.text(local);
                    out.text(peer);
                }
                DescriptorSnapshot::Synthetic {
                    path,
                    flags,
                    data,
                    offset,
                    directory,
                } => {
                    out.word(6);
                    out.text(path);
                    out.word(*offset);
                    out.word(u64::from(*flags));
                    out.bytes(data);
                    out.word(u64::from(*directory));
                }
                DescriptorSnapshot::UnixEndpoint {
                    flags,
                    bound,
                    listening,
                    pending,
                } => {
                    out.word(7);
                    out.word(u64::from(*flags));
                    out.text(bound);
                    out.word(u64::from(*listening));
                    out.word(pending.len() as u64);
                    for (handle, peer) in pending {
                        out.word(*handle);
                        out.text(peer);
                    }
                }
                DescriptorSnapshot::EventFd {
                    flags,
                    counter,
                    semaphore,
                } => {
                    out.word(8);
                    out.word(u64::from(*flags));
                    out.word(*counter);
                    out.word(u64::from(*semaphore));
                }
                DescriptorSnapshot::Epoll { flags, interests } => {
                    out.word(9);
                    out.word(u64::from(*flags));
                    out.word(interests.len() as u64);
                    for (fd, events, data, enabled) in interests {
                        out.word(*fd as u32 as u64);
                        out.word(u64::from(*events));
                        out.word(*data);
                        out.word(u64::from(*enabled));
                    }
                }
                DescriptorSnapshot::TimerFd {
                    flags,
                    clock_id,
                    remaining,
                    interval,
                    armed,
                } => {
                    out.word(10);
                    out.word(u64::from(*flags));
                    out.word(*clock_id as u32 as u64);
                    out.word(*remaining);
                    out.word(*interval);
                    out.word(u64::from(*armed));
                }
            }
        }
        out.word(self.signal_frames.len() as u64);
        for (address, length, old_mask) in &self.signal_frames {
            out.word(*address);
            out.word(*length);
            out.word(*old_mask);
        }
        out.word(self.tasks.len() as u64);
        for task in &self.tasks {
            out.word(u64::from(task.tid));
            out.bytes(&task.registers);
            out.word(task.clear_tid);
            out.word(task.robust_list);
            if let Some((address, length, signature)) = task.rseq {
                out.word(1);
                out.word(address);
                out.word(u64::from(length));
                out.word(u64::from(signature));
            } else {
                out.word(0);
            }
            if let Some((address, deadline, private, bitset)) = task.futex {
                out.word(1);
                out.word(address);
                out.word(deadline.unwrap_or(u64::MAX));
                out.word(u64::from(private));
                out.word(u64::from(bitset));
            } else {
                out.word(0);
            }
            if let Some((address, deadline, private, owner, queued_at)) = task.pi_futex {
                out.word(1);
                out.word(address);
                out.word(deadline.unwrap_or(u64::MAX));
                out.word(u64::from(private));
                out.word(u64::from(owner));
                out.word(queued_at);
            } else {
                out.word(0);
            }
            if let Some((waiters, deadline)) = &task.futex_waitv {
                out.word(1);
                out.word(waiters.len() as u64);
                for (address, private) in waiters {
                    out.word(*address);
                    out.word(u64::from(*private));
                }
                out.word(deadline.unwrap_or(u64::MAX));
            } else {
                out.word(0);
            }
            if let Some((address, target, deadline, private, queued_at)) = task.pi_requeue {
                out.word(1);
                out.word(address);
                out.word(target);
                out.word(deadline.unwrap_or(u64::MAX));
                out.word(u64::from(private));
                out.word(queued_at);
            } else {
                out.word(0);
            }
            out.word(task.sleep_deadline.unwrap_or(u64::MAX));
            out.word(task.signal_frames.len() as u64);
            for (address, length, old_mask) in &task.signal_frames {
                out.word(*address);
                out.word(*length);
                out.word(*old_mask);
            }
            if let Some((pid, status, rusage)) = task.wait {
                out.word(1);
                out.word(pid as u64);
                out.word(status);
                out.word(rusage);
            } else {
                out.word(0);
            }
            if let Some((child, status, rusage, exit_status)) = task.wait_ready {
                out.word(1);
                out.word(u64::from(child));
                out.word(status);
                out.word(rusage);
                out.word(exit_status as u32 as u64);
            } else {
                out.word(0);
            }
            out.word(task.vfork_child.map_or(u64::MAX, u64::from));
            out.word(u64::from(task.signal_wait));
        }
        out.word(self.current_task as u64);
        out.word(u64::from(self.next_tid));
        out.word(u64::from(self.pid));
        out.word(u64::from(self.ppid));
        out.word(u64::from(self.exit_signal));
        out.word(self.vfork_parent.map_or(u64::MAX, u64::from));
        out.word(self.startup_writes.len() as u64);
        for (address, bytes) in &self.startup_writes {
            out.word(*address);
            out.bytes(bytes);
        }
        out.word(self.processes.len() as u64);
        for process in &self.processes {
            out.word(u64::from(process.pid));
            out.word(u64::from(process.ppid));
            out.word(u64::from(process.exit_signal));
            out.word(process.vfork_parent.map_or(u64::MAX, u64::from));
            out.bytes(&process.runtime);
            out.word(process.mappings.len() as u64);
            for mapping in &process.mappings {
                out.word(mapping.handle);
                out.word(mapping.futex_id);
                out.word(mapping.address);
                out.word(mapping.size);
                out.word(mapping.offset);
                out.word(u64::from(mapping.rights));
                out.word(mapping.kind);
                out.word(mapping.file_fd as u32 as u64);
                out.word(mapping.file_offset);
                out.word(u64::from(mapping.shared));
            }
            out.word(process.startup_writes.len() as u64);
            for (address, bytes) in &process.startup_writes {
                out.word(*address);
                out.bytes(bytes);
            }
        }
        out.word(self.zombies.len() as u64);
        for (pid, ppid, status) in &self.zombies {
            out.word(u64::from(*pid));
            out.word(u64::from(*ppid));
            out.word(*status as u32 as u64);
        }
        out.word(u64::from(self.next_pid));
        let bytes = out.finish();
        if bytes.len() > MAX_RUNTIME_BYTES {
            return Err(Error::Capacity);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_RUNTIME_BYTES {
            return Err(Error::Capacity);
        }
        let mut input = Decoder::new(bytes);
        let version = input.word()?;
        if !matches!(version, 1..=15) {
            return Err(Error::UnsupportedVersion);
        }
        let signals = input.bytes(16 * 1024)?.to_vec();
        let dispatcher = input.bytes(16 * 1024)?.to_vec();
        let cwd = input.text(4096)?.into();
        let mut descriptors = Vec::new();
        for _ in 0..input.count(MAX_FDS)? {
            let fd = u16::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            let descriptor = match input.word()? {
                0 => {
                    let source = u8::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                    if source > 2 {
                        return Err(Error::InvalidData);
                    }
                    DescriptorSnapshot::Stdio { source }
                }
                1 => DescriptorSnapshot::File {
                    path: input.text(4096)?.into(),
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                    offset: input.word()?,
                },
                2 => DescriptorSnapshot::Directory {
                    path: input.text(4096)?.into(),
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                },
                3 => DescriptorSnapshot::Null {
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                },
                4 => DescriptorSnapshot::Zero {
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                },
                5 if version >= 2 => DescriptorSnapshot::Socket {
                    handle: input.word()?,
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                    readable: input.flag()?,
                    writable: input.flag()?,
                    local: if version >= 4 {
                        input.text(108)?.into()
                    } else {
                        String::new()
                    },
                    peer: if version >= 4 {
                        input.text(108)?.into()
                    } else {
                        String::new()
                    },
                },
                6 if version >= 3 => {
                    let path: String = input.text(4096)?.into();
                    let offset = input.word()?;
                    let (flags, data, directory) = if version >= 15 {
                        (
                            u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                            input.bytes(MAX_SYNTHETIC_BYTES)?.to_vec(),
                            input.flag()?,
                        )
                    } else {
                        let directory = synthetic_directory(&path);
                        let data = directory.as_ref().map_or_else(
                            || synthetic_file(&path).unwrap_or_default(),
                            |entries| entries.join("\n").into_bytes(),
                        );
                        (0, data, directory.is_some())
                    };
                    if offset > data.len() as u64 && version >= 15 {
                        return Err(Error::InvalidData);
                    }
                    DescriptorSnapshot::Synthetic {
                        path,
                        flags,
                        data,
                        offset,
                        directory,
                    }
                }
                7 if version >= 4 => {
                    let flags = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                    let bound = input.text(108)?.into();
                    let listening = input.flag()?;
                    let mut pending = Vec::new();
                    for _ in 0..input.count(128)? {
                        pending.push((input.word()?, input.text(108)?.into()));
                    }
                    DescriptorSnapshot::UnixEndpoint {
                        flags,
                        bound,
                        listening,
                        pending,
                    }
                }
                8 if version >= 5 => DescriptorSnapshot::EventFd {
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                    counter: input.word()?,
                    semaphore: input.flag()?,
                },
                9 if version >= 5 => {
                    let flags = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                    let mut interests = Vec::new();
                    for _ in 0..input.count(MAX_FDS)? {
                        let fd = i32::from_ne_bytes(
                            u32::try_from(input.word()?)
                                .map_err(|_| Error::InvalidData)?
                                .to_ne_bytes(),
                        );
                        let events =
                            u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                        let data = input.word()?;
                        let enabled = input.flag()?;
                        interests.push((fd, events, data, enabled));
                    }
                    DescriptorSnapshot::Epoll { flags, interests }
                }
                10 if version >= 5 => DescriptorSnapshot::TimerFd {
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                    clock_id: i32::from_ne_bytes(
                        u32::try_from(input.word()?)
                            .map_err(|_| Error::InvalidData)?
                            .to_ne_bytes(),
                    ),
                    remaining: input.word()?,
                    interval: input.word()?,
                    armed: input.flag()?,
                },
                _ => return Err(Error::InvalidData),
            };
            if descriptors.iter().any(|(old, _)| *old == fd) {
                return Err(Error::InvalidData);
            }
            descriptors.push((fd, descriptor));
        }
        let mut signal_frames = Vec::new();
        for _ in 0..input.count(64)? {
            signal_frames.push((input.word()?, input.word()?, input.word()?));
        }
        let (tasks, current_task, next_tid) = if version >= 6 {
            let mut tasks: Vec<TaskSnapshot> = Vec::new();
            for _ in 0..input.count(256)? {
                let tid = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                let registers = input.bytes(MAX_REGISTER_BYTES)?.to_vec();
                let clear_tid = input.word()?;
                let robust_list = if version >= 9 { input.word()? } else { 0 };
                let rseq = if version >= 14 && input.flag()? {
                    let address = input.word()?;
                    let length = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                    let signature = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                    if address == 0 || address & 31 != 0 || length != 32 {
                        return Err(Error::InvalidData);
                    }
                    Some((address, length, signature))
                } else {
                    None
                };
                let futex = if input.flag()? {
                    let address = input.word()?;
                    let deadline = input.word()?;
                    let private = version >= 10 && input.flag()?;
                    let bitset = if version >= 13 {
                        u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?
                    } else {
                        u32::MAX
                    };
                    if bitset == 0 {
                        return Err(Error::InvalidData);
                    }
                    Some((
                        address,
                        (deadline != u64::MAX).then_some(deadline),
                        private,
                        bitset,
                    ))
                } else {
                    None
                };
                let pi_futex = if version >= 10 && input.flag()? {
                    let address = input.word()?;
                    let deadline = input.word()?;
                    let private = input.flag()?;
                    let owner = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                    let queued_at = input.word()?;
                    if owner == 0 {
                        return Err(Error::InvalidData);
                    }
                    Some((
                        address,
                        (deadline != u64::MAX).then_some(deadline),
                        private,
                        owner,
                        queued_at,
                    ))
                } else {
                    None
                };
                let futex_waitv = if version >= 11 && input.flag()? {
                    let mut waiters = Vec::new();
                    for _ in 0..input.count(128)? {
                        waiters.push((input.word()?, input.flag()?));
                    }
                    let deadline = input.word()?;
                    if waiters.is_empty() {
                        return Err(Error::InvalidData);
                    }
                    Some((waiters, (deadline != u64::MAX).then_some(deadline)))
                } else {
                    None
                };
                let pi_requeue = if version >= 12 && input.flag()? {
                    let address = input.word()?;
                    let target = input.word()?;
                    let deadline = input.word()?;
                    let private = input.flag()?;
                    let queued_at = input.word()?;
                    if address == target {
                        return Err(Error::InvalidData);
                    }
                    Some((
                        address,
                        target,
                        (deadline != u64::MAX).then_some(deadline),
                        private,
                        queued_at,
                    ))
                } else {
                    None
                };
                let sleep = input.word()?;
                let mut frames = Vec::new();
                for _ in 0..input.count(64)? {
                    frames.push((input.word()?, input.word()?, input.word()?));
                }
                let wait = if version >= 7 && input.flag()? {
                    Some((input.word()? as i64, input.word()?, input.word()?))
                } else {
                    None
                };
                let wait_ready = if version >= 7 && input.flag()? {
                    Some((
                        u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                        input.word()?,
                        input.word()?,
                        i32::from_ne_bytes(
                            u32::try_from(input.word()?)
                                .map_err(|_| Error::InvalidData)?
                                .to_ne_bytes(),
                        ),
                    ))
                } else {
                    None
                };
                let vfork_child = if version >= 7 {
                    let child = input.word()?;
                    (child != u64::MAX)
                        .then(|| u32::try_from(child).map_err(|_| Error::InvalidData))
                        .transpose()?
                } else {
                    None
                };
                let signal_wait = version >= 8 && input.flag()?;
                if usize::from(futex.is_some())
                    + usize::from(pi_futex.is_some())
                    + usize::from(futex_waitv.is_some())
                    + usize::from(pi_requeue.is_some())
                    + usize::from(sleep != u64::MAX)
                    + usize::from(wait.is_some())
                    + usize::from(wait_ready.is_some())
                    + usize::from(vfork_child.is_some())
                    + usize::from(signal_wait)
                    > 1
                {
                    return Err(Error::InvalidData);
                }
                if tid == 0 || tasks.iter().any(|task| task.tid == tid) {
                    return Err(Error::InvalidData);
                }
                tasks.push(TaskSnapshot {
                    tid,
                    registers,
                    clear_tid,
                    robust_list,
                    rseq,
                    futex,
                    pi_futex,
                    futex_waitv,
                    pi_requeue,
                    sleep_deadline: (sleep != u64::MAX).then_some(sleep),
                    signal_frames: frames,
                    wait,
                    wait_ready,
                    vfork_child,
                    signal_wait,
                });
            }
            let current = usize::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            let next = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            if tasks.is_empty()
                || current >= tasks.len()
                || next <= tasks.iter().map(|task| task.tid).max().unwrap_or(0)
            {
                return Err(Error::InvalidData);
            }
            (tasks, current, next)
        } else {
            (Vec::new(), 0, 2)
        };
        let (pid, ppid, exit_signal, vfork_parent, startup_writes) = if version >= 7 {
            let pid = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            let ppid = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            let exit_signal = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            let parent = input.word()?;
            let vfork_parent = (parent != u64::MAX)
                .then(|| u32::try_from(parent).map_err(|_| Error::InvalidData))
                .transpose()?;
            let mut writes = Vec::new();
            for _ in 0..input.count(MAX_STARTUP_WRITES)? {
                writes.push((input.word()?, input.bytes(64)?.to_vec()));
            }
            (pid, ppid, exit_signal, vfork_parent, writes)
        } else {
            (1, 0, 0, None, Vec::new())
        };
        let (processes, zombies, next_pid) = if version >= 7 {
            let mut processes = Vec::new();
            for _ in 0..input.count(MAX_PROCESSES)? {
                let pid = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                let ppid = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                let exit_signal = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                let parent = input.word()?;
                let vfork_parent = (parent != u64::MAX)
                    .then(|| u32::try_from(parent).map_err(|_| Error::InvalidData))
                    .transpose()?;
                let runtime = input.bytes(MAX_RUNTIME_BYTES)?.to_vec();
                let nested = RuntimeState::decode(&runtime)?;
                if !nested.processes.is_empty() || !nested.zombies.is_empty() {
                    return Err(Error::InvalidData);
                }
                let mut mappings = Vec::new();
                for _ in 0..input.count(MAX_MAPPINGS)? {
                    let handle = input.word()?;
                    mappings.push(ProcessMapping {
                        handle,
                        futex_id: if version >= 10 { input.word()? } else { handle },
                        address: input.word()?,
                        size: input.word()?,
                        offset: input.word()?,
                        rights: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                        kind: input.word()?,
                        file_fd: i32::from_ne_bytes(
                            u32::try_from(input.word()?)
                                .map_err(|_| Error::InvalidData)?
                                .to_ne_bytes(),
                        ),
                        file_offset: input.word()?,
                        shared: input.flag()?,
                    });
                }
                let mut startup_writes = Vec::new();
                for _ in 0..input.count(MAX_STARTUP_WRITES)? {
                    startup_writes.push((input.word()?, input.bytes(64)?.to_vec()));
                }
                processes.push(ProcessSnapshot {
                    pid,
                    ppid,
                    exit_signal,
                    vfork_parent,
                    runtime,
                    mappings,
                    startup_writes,
                });
            }
            let mut zombies = Vec::new();
            for _ in 0..input.count(MAX_PROCESSES)? {
                zombies.push((
                    u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                    u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                    i32::from_ne_bytes(
                        u32::try_from(input.word()?)
                            .map_err(|_| Error::InvalidData)?
                            .to_ne_bytes(),
                    ),
                ));
            }
            let next_pid = u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            (processes, zombies, next_pid)
        } else {
            (Vec::new(), Vec::new(), next_tid.max(2))
        };
        input.finish()?;
        Ok(Self {
            signals,
            dispatcher,
            vfs: VfsSnapshot { cwd, descriptors },
            signal_frames,
            tasks,
            current_task,
            next_tid,
            pid,
            ppid,
            exit_signal,
            vfork_parent,
            startup_writes,
            processes,
            zombies,
            next_pid,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Mapping {
    pub handle: u64,
    pub futex_id: u64,
    pub address: u64,
    pub size: u64,
    pub offset: u64,
    pub rights: u32,
    pub kind: u64,
    pub file_fd: i32,
    pub file_offset: u64,
    pub shared: bool,
    pub owned: bool,
}

pub(crate) struct Snapshot {
    architecture: u64,
    options: Vec<u8>,
    registers: Vec<u8>,
    runtime: Vec<u8>,
    pub mappings: Vec<Mapping>,
    owned_handles: Vec<u64>,
    migration: Option<Channel>,
    valid: bool,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        for mapping in self.mappings.drain(..).filter(|mapping| mapping.owned) {
            let _ = bexos_userspace::Memory::close(mapping.handle);
        }
        for handle in self.owned_handles.drain(..) {
            let _ = bexos_userspace::Memory::close(handle);
        }
    }
}

impl Snapshot {
    pub fn source(
        architecture: u64,
        options: &NixRunnerOptions,
        registers: &[u8],
        runtime: Vec<u8>,
        mappings: Vec<Mapping>,
        migration: Option<Channel>,
    ) -> Result<Self, Error> {
        let value = Self {
            architecture,
            options: options.encode().map_err(|_| Error::InvalidData)?,
            registers: registers.to_vec(),
            runtime,
            mappings,
            owned_handles: Vec::new(),
            migration,
            valid: true,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn candidate(architecture: u64, options: &NixRunnerOptions) -> Result<Self, Error> {
        Ok(Self {
            architecture,
            options: options.encode().map_err(|_| Error::InvalidData)?,
            registers: Vec::new(),
            runtime: Vec::new(),
            mappings: Vec::new(),
            owned_handles: Vec::new(),
            migration: None,
            valid: false,
        })
    }

    pub fn capture_registers(&mut self, registers: &[u8]) -> Result<(), Error> {
        if registers.len() > MAX_REGISTER_BYTES {
            return Err(Error::Capacity);
        }
        self.registers.clear();
        self.registers.extend_from_slice(registers);
        self.valid = true;
        Ok(())
    }

    pub fn replace_mappings(&mut self, mappings: Vec<Mapping>) {
        for mapping in self.mappings.drain(..).filter(|mapping| mapping.owned) {
            let _ = bexos_userspace::Memory::close(mapping.handle);
        }
        self.mappings = mappings;
    }

    pub fn replace_owned_handles(&mut self, handles: Vec<u64>) {
        for handle in self.owned_handles.drain(..) {
            let _ = bexos_userspace::Memory::close(handle);
        }
        self.owned_handles = handles;
    }

    pub fn capture_runtime(&mut self, runtime: Vec<u8>) -> Result<(), Error> {
        if runtime.is_empty() || runtime.len() > MAX_RUNTIME_BYTES {
            return Err(Error::Capacity);
        }
        self.runtime = runtime;
        Ok(())
    }

    pub fn runtime(&self) -> &[u8] {
        &self.runtime
    }

    pub fn registers(&self) -> &[u8] {
        &self.registers
    }

    pub fn migration(&self) -> Option<Channel> {
        self.migration
    }
}

impl State for Snapshot {
    fn empty() -> Self {
        Self {
            architecture: u64::MAX,
            options: Vec::new(),
            registers: Vec::new(),
            runtime: Vec::new(),
            mappings: Vec::new(),
            owned_handles: Vec::new(),
            migration: None,
            valid: false,
        }
    }

    fn keys(&self) -> Vec<u64> {
        vec![0]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 || !self.valid {
            return Err(Error::InvalidData);
        }
        let mut out = Encoder::new();
        out.word(RECORD_VERSION);
        out.word(u64::from(ABI_VERSION));
        out.text(UPSTREAM_REVISION);
        out.word(self.architecture);
        out.bytes(&self.options);
        out.bytes(&self.registers);
        out.bytes(&self.runtime);
        out.word(self.migration.map_or(0, |channel| channel.0));
        out.word(self.mappings.len() as u64);
        for mapping in &self.mappings {
            out.word(mapping.handle);
            out.word(mapping.futex_id);
            out.word(mapping.address);
            out.word(mapping.size);
            out.word(mapping.offset);
            out.word(u64::from(mapping.rights));
            out.word(mapping.kind);
            out.word(mapping.file_fd as u32 as u64);
            out.word(mapping.file_offset);
            out.word(u64::from(mapping.shared));
        }
        Ok(Some(out.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut input = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if input.word()? != RECORD_VERSION
            || input.word()? != u64::from(ABI_VERSION)
            || input.text(64)? != UPSTREAM_REVISION
            || input.word()? != self.architecture
        {
            return Err(Error::UnsupportedVersion);
        }
        if input.bytes(64 * 1024)? != self.options {
            return Err(Error::InvalidData);
        }
        self.registers = input.bytes(MAX_REGISTER_BYTES)?.to_vec();
        self.runtime = input.bytes(MAX_RUNTIME_BYTES)?.to_vec();
        let migration = input.word()?;
        self.migration = (migration != 0).then_some(Channel(migration));
        self.mappings.clear();
        for _ in 0..input.count(MAX_MAPPINGS)? {
            self.mappings.push(Mapping {
                handle: input.word()?,
                futex_id: input.word()?,
                address: input.word()?,
                size: input.word()?,
                offset: input.word()?,
                rights: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                kind: input.word()?,
                file_fd: i32::from_ne_bytes(
                    u32::try_from(input.word()?)
                        .map_err(|_| Error::InvalidData)?
                        .to_ne_bytes(),
                ),
                file_offset: input.word()?,
                shared: input.flag()?,
                owned: false,
            });
        }
        input.finish()?;
        self.valid = true;
        self.validate()
    }

    fn validate(&self) -> Result<(), Error> {
        let runtime = RuntimeState::decode(&self.runtime).map_err(|_| Error::InvalidData)?;
        let max_nested_tid = runtime
            .processes
            .iter()
            .filter_map(|process| RuntimeState::decode(&process.runtime).ok())
            .filter_map(|process| process.tasks.iter().map(|task| task.tid).max())
            .max()
            .unwrap_or(0);
        let max_allocated_id = runtime
            .processes
            .iter()
            .map(|process| process.pid)
            .chain(runtime.zombies.iter().map(|(pid, _, _)| *pid))
            .chain(runtime.tasks.iter().map(|task| task.tid))
            .chain(core::iter::once(max_nested_tid))
            .max()
            .unwrap_or(runtime.pid);
        if !self.valid
            || self.registers.is_empty()
            || self.registers.len() > MAX_REGISTER_BYTES
            || self.runtime.is_empty()
            || self.runtime.len() > MAX_RUNTIME_BYTES
            || runtime.pid == 0
            || runtime.next_pid <= runtime.pid
            || runtime.vfork_parent.is_some_and(|parent| {
                parent != runtime.ppid
                    || !runtime
                        .processes
                        .iter()
                        .any(|process| process.pid == parent)
            })
            || runtime
                .processes
                .iter()
                .any(|process| !valid_process(process, runtime.pid))
            || runtime
                .processes
                .iter()
                .enumerate()
                .any(|(index, process)| {
                    runtime.processes[index + 1..]
                        .iter()
                        .any(|other| other.pid == process.pid)
                })
            || runtime.processes.iter().any(|process| {
                process.ppid != runtime.pid
                    && !runtime
                        .processes
                        .iter()
                        .any(|candidate| candidate.pid == process.ppid)
            })
            || runtime.zombies.iter().any(|(pid, ppid, _)| {
                *pid == 0
                    || *pid == runtime.pid
                    || *ppid == 0
                    || runtime.processes.iter().any(|process| process.pid == *pid)
            })
            || runtime.zombies.iter().enumerate().any(|(index, zombie)| {
                runtime.zombies[index + 1..]
                    .iter()
                    .any(|other| other.0 == zombie.0)
            })
            || runtime.next_pid <= max_allocated_id
            || self.mappings.is_empty()
            || self.mappings.len() > MAX_MAPPINGS
            || self.mappings.iter().any(|mapping| {
                mapping.handle == 0
                    || mapping.futex_id == 0
                    || mapping.size == 0
                    || mapping.address & 4095 != 0
                    || mapping.size & 4095 != 0
                    || mapping.offset & 4095 != 0
                    || mapping.rights & !0xe != 0
                    || mapping.rights & (4 | 8) == (4 | 8)
                    || mapping.kind > 4
                    || (mapping.kind != 3
                        && (mapping.file_fd != -1 || mapping.file_offset != 0 || mapping.shared))
                    || mapping.address.checked_add(mapping.size).is_none_or(|end| {
                        mapping.address < bexos_boot::USER_START || end > bexos_boot::USER_END
                    })
            })
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut resources: Vec<_> = self
            .mappings
            .iter()
            .map(|mapping| Resource::Mapping {
                handle: mapping.handle,
                offset: mapping.offset,
                va: mapping.address,
                size: mapping.size,
                rights: mapping.rights,
            })
            .collect();
        if let Ok(runtime) = RuntimeState::decode(&self.runtime) {
            append_vfs_resources(&runtime.vfs, &mut resources);
            for process in &runtime.processes {
                resources.extend(
                    process
                        .mappings
                        .iter()
                        .map(|mapping| Resource::Handle(mapping.handle)),
                );
                if let Ok(nested) = RuntimeState::decode(&process.runtime) {
                    append_vfs_resources(&nested.vfs, &mut resources);
                }
            }
        }
        resources
    }

    fn activated(&mut self, _generation: u64) {}
}

fn valid_process(process: &ProcessSnapshot, current_pid: u32) -> bool {
    if process.pid == 0
        || process.pid == current_pid
        || process.ppid == 0
        || process.runtime.is_empty()
        || process.mappings.is_empty()
        || process.mappings.len() > MAX_MAPPINGS
        || process.startup_writes.len() > MAX_STARTUP_WRITES
    {
        return false;
    }
    let Ok(nested) = RuntimeState::decode(&process.runtime) else {
        return false;
    };
    nested.pid == process.pid
        && nested.ppid == process.ppid
        && nested.exit_signal == process.exit_signal
        && nested.vfork_parent == process.vfork_parent
        && process
            .vfork_parent
            .is_none_or(|parent| parent == process.ppid)
        && nested.processes.is_empty()
        && nested.zombies.is_empty()
        && process.mappings.iter().all(valid_process_mapping)
}

fn valid_process_mapping(mapping: &ProcessMapping) -> bool {
    mapping.handle != 0
        && mapping.futex_id != 0
        && mapping.size != 0
        && mapping.address & 4095 == 0
        && mapping.size & 4095 == 0
        && mapping.offset & 4095 == 0
        && mapping.rights & !0xe == 0
        && mapping.rights & (4 | 8) != (4 | 8)
        && mapping.kind <= 4
        && (mapping.kind == 3
            || (mapping.file_fd == -1 && mapping.file_offset == 0 && !mapping.shared))
        && mapping
            .address
            .checked_add(mapping.size)
            .is_some_and(|end| {
                mapping.address >= bexos_boot::USER_START && end <= bexos_boot::USER_END
            })
}

fn append_vfs_resources(vfs: &VfsSnapshot, resources: &mut Vec<Resource>) {
    for (_, descriptor) in &vfs.descriptors {
        match descriptor {
            DescriptorSnapshot::Socket { handle, .. } => {
                resources.push(Resource::Handle(*handle));
            }
            DescriptorSnapshot::UnixEndpoint { pending, .. } => {
                resources.extend(pending.iter().map(|(handle, _)| Resource::Handle(*handle)));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_rejects_wrong_architecture_and_preserves_mapping_metadata() {
        let options = NixRunnerOptions {
            path: "/pkg/bin/looping".into(),
            arguments: vec!["looping".into()],
            environment: Vec::new(),
            rootfs: Default::default(),
            ..Default::default()
        };
        let source = Snapshot::source(
            1,
            &options,
            &[7; 128],
            RuntimeState {
                signals: vec![1],
                dispatcher: vec![2],
                vfs: VfsSnapshot {
                    cwd: "/".into(),
                    descriptors: Vec::new(),
                },
                signal_frames: Vec::new(),
                tasks: vec![TaskSnapshot {
                    tid: 1,
                    registers: vec![0; 128],
                    clear_tid: 0,
                    robust_list: 0,
                    rseq: None,
                    futex: None,
                    pi_futex: None,
                    futex_waitv: None,
                    pi_requeue: None,
                    sleep_deadline: None,
                    signal_frames: Vec::new(),
                    wait: None,
                    wait_ready: None,
                    vfork_child: None,
                    signal_wait: false,
                }],
                current_task: 0,
                next_tid: 2,
                pid: 1,
                ppid: 0,
                exit_signal: 0,
                vfork_parent: None,
                startup_writes: Vec::new(),
                processes: Vec::new(),
                zombies: Vec::new(),
                next_pid: 2,
            }
            .encode()
            .unwrap(),
            vec![Mapping {
                handle: 9,
                futex_id: 90,
                address: 0x10_0000_0000,
                size: 4096,
                offset: 0,
                rights: 2 | 8,
                kind: 2,
                file_fd: -1,
                file_offset: 0,
                shared: false,
                owned: false,
            }],
            Some(Channel(11)),
        )
        .unwrap();
        let bytes = source.encode_record(0).unwrap().unwrap();
        let mut candidate = Snapshot::candidate(1, &options).unwrap();
        candidate.adopt_record(0, Some(&bytes)).unwrap();
        assert_eq!(candidate.mappings, source.mappings);
        assert_eq!(candidate.registers(), &[7; 128]);
        assert_eq!(candidate.runtime(), source.runtime());
        let mut wrong = Snapshot::candidate(0, &options).unwrap();
        assert_eq!(
            wrong.adopt_record(0, Some(&bytes)),
            Err(Error::UnsupportedVersion)
        );
        let mut wrong_options = options.clone();
        wrong_options.arguments.push("different".into());
        let mut wrong = Snapshot::candidate(1, &wrong_options).unwrap();
        assert_eq!(wrong.adopt_record(0, Some(&bytes)), Err(Error::InvalidData));
    }

    #[test]
    fn checkpoint_rejects_write_execute_mappings() {
        let options = NixRunnerOptions {
            path: "/pkg/bin/looping".into(),
            arguments: vec!["looping".into()],
            environment: Vec::new(),
            rootfs: Default::default(),
            ..Default::default()
        };
        for rights in [4 | 8, 2 | 4 | 8] {
            assert!(matches!(
                Snapshot::source(
                    1,
                    &options,
                    &[7; 128],
                    RuntimeState {
                        signals: vec![1],
                        dispatcher: vec![2],
                        vfs: VfsSnapshot {
                            cwd: "/".into(),
                            descriptors: Vec::new(),
                        },
                        signal_frames: Vec::new(),
                        tasks: vec![TaskSnapshot {
                            tid: 1,
                            registers: vec![0; 128],
                            clear_tid: 0,
                            robust_list: 0,
                            rseq: None,
                            futex: None,
                            pi_futex: None,
                            futex_waitv: None,
                            pi_requeue: None,
                            sleep_deadline: None,
                            signal_frames: Vec::new(),
                            wait: None,
                            wait_ready: None,
                            vfork_child: None,
                            signal_wait: false,
                        }],
                        current_task: 0,
                        next_tid: 2,
                        pid: 1,
                        ppid: 0,
                        exit_signal: 0,
                        vfork_parent: None,
                        startup_writes: Vec::new(),
                        processes: Vec::new(),
                        zombies: Vec::new(),
                        next_pid: 2,
                    }
                    .encode()
                    .unwrap(),
                    vec![Mapping {
                        handle: 9,
                        futex_id: 90,
                        address: 0x10_0000_0000,
                        size: 4096,
                        offset: 0,
                        rights,
                        kind: 2,
                        file_fd: -1,
                        file_offset: 0,
                        shared: false,
                        owned: false,
                    }],
                    Some(Channel(11)),
                ),
                Err(Error::InvalidData)
            ));
        }
    }

    #[test]
    fn runtime_round_trips_migratable_local_socket_handles() {
        let state = RuntimeState {
            signals: vec![1],
            dispatcher: vec![2],
            vfs: VfsSnapshot {
                cwd: "/work".into(),
                descriptors: vec![
                    (
                        4,
                        DescriptorSnapshot::Socket {
                            handle: 77,
                            flags: 0x800,
                            readable: true,
                            writable: false,
                            local: String::new(),
                            peer: String::new(),
                        },
                    ),
                    (
                        5,
                        DescriptorSnapshot::EventFd {
                            flags: 0,
                            counter: 9,
                            semaphore: true,
                        },
                    ),
                    (
                        6,
                        DescriptorSnapshot::Epoll {
                            flags: 0x8_0000,
                            interests: vec![(5, 1, 0xfeed, true)],
                        },
                    ),
                    (
                        7,
                        DescriptorSnapshot::TimerFd {
                            flags: 0x800,
                            clock_id: 1,
                            remaining: 42,
                            interval: 11,
                            armed: true,
                        },
                    ),
                    (
                        8,
                        DescriptorSnapshot::Synthetic {
                            path: "/proc/self/maps".into(),
                            flags: 0x8_0000,
                            data: b"1000-2000 r-xp 0 00:00 0 [image]\n".to_vec(),
                            offset: 5,
                            directory: false,
                        },
                    ),
                ],
            },
            signal_frames: Vec::new(),
            tasks: vec![
                TaskSnapshot {
                    tid: 1,
                    registers: vec![3; 128],
                    clear_tid: 0,
                    robust_list: 0,
                    rseq: Some((0x7000, 32, 0x5305_3053)),
                    futex: Some((0x1000, None, true, 0x55aa)),
                    pi_futex: None,
                    futex_waitv: None,
                    pi_requeue: None,
                    sleep_deadline: None,
                    signal_frames: Vec::new(),
                    wait: None,
                    wait_ready: None,
                    vfork_child: None,
                    signal_wait: false,
                },
                TaskSnapshot {
                    tid: 2,
                    registers: vec![4; 128],
                    clear_tid: 0,
                    robust_list: 0,
                    rseq: None,
                    futex: None,
                    pi_futex: Some((0x2000, Some(1234), false, 1, 99)),
                    futex_waitv: None,
                    pi_requeue: None,
                    sleep_deadline: None,
                    signal_frames: Vec::new(),
                    wait: None,
                    wait_ready: None,
                    vfork_child: None,
                    signal_wait: false,
                },
                TaskSnapshot {
                    tid: 3,
                    registers: vec![5; 128],
                    clear_tid: 0,
                    robust_list: 0,
                    rseq: None,
                    futex: None,
                    pi_futex: None,
                    futex_waitv: Some((vec![(0x3000, true), (0x4000, false)], Some(5678))),
                    pi_requeue: None,
                    sleep_deadline: None,
                    signal_frames: Vec::new(),
                    wait: None,
                    wait_ready: None,
                    vfork_child: None,
                    signal_wait: false,
                },
                TaskSnapshot {
                    tid: 4,
                    registers: vec![6; 128],
                    clear_tid: 0,
                    robust_list: 0,
                    rseq: None,
                    futex: None,
                    pi_futex: None,
                    futex_waitv: None,
                    pi_requeue: Some((0x5000, 0x6000, Some(9012), false, 101)),
                    sleep_deadline: None,
                    signal_frames: Vec::new(),
                    wait: None,
                    wait_ready: None,
                    vfork_child: None,
                    signal_wait: false,
                },
            ],
            current_task: 0,
            next_tid: 5,
            pid: 1,
            ppid: 0,
            exit_signal: 0,
            vfork_parent: None,
            startup_writes: Vec::new(),
            processes: Vec::new(),
            zombies: Vec::new(),
            next_pid: 5,
        };
        let decoded = RuntimeState::decode(&state.encode().unwrap()).unwrap();
        assert_eq!(decoded.vfs, state.vfs);
        assert_eq!(decoded.tasks, state.tasks);

        let options = NixRunnerOptions {
            path: "/bin/tool".into(),
            ..Default::default()
        };
        let snapshot = Snapshot::source(
            1,
            &options,
            &[1],
            state.encode().unwrap(),
            vec![Mapping {
                handle: 9,
                futex_id: 90,
                address: 0x10_0000_0000,
                size: 4096,
                offset: 0,
                rights: 2,
                kind: 2,
                file_fd: -1,
                file_offset: 0,
                shared: false,
                owned: false,
            }],
            None,
        )
        .unwrap();
        assert!(
            snapshot
                .resources()
                .iter()
                .any(|resource| matches!(resource, Resource::Handle(handle) if *handle == 77))
        );
    }

    #[test]
    fn runtime_round_trips_process_wait_vfork_and_zombie_state() {
        let child = RuntimeState {
            signals: vec![3],
            dispatcher: vec![4],
            vfs: VfsSnapshot {
                cwd: "/child".into(),
                descriptors: Vec::new(),
            },
            signal_frames: vec![(0x2000, 128, 7)],
            tasks: vec![TaskSnapshot {
                tid: 2,
                registers: vec![5; 128],
                clear_tid: 0x3000,
                robust_list: 0x3500,
                rseq: None,
                futex: None,
                pi_futex: None,
                futex_waitv: None,
                pi_requeue: None,
                sleep_deadline: None,
                signal_frames: Vec::new(),
                wait: None,
                wait_ready: None,
                vfork_child: None,
                signal_wait: true,
            }],
            current_task: 0,
            next_tid: 4,
            pid: 2,
            ppid: 1,
            exit_signal: 17,
            vfork_parent: Some(1),
            startup_writes: vec![(0x4000, 2u32.to_ne_bytes().to_vec())],
            processes: Vec::new(),
            zombies: Vec::new(),
            next_pid: 4,
        };
        let state = RuntimeState {
            signals: vec![1],
            dispatcher: vec![2],
            vfs: VfsSnapshot {
                cwd: "/".into(),
                descriptors: Vec::new(),
            },
            signal_frames: Vec::new(),
            tasks: vec![
                TaskSnapshot {
                    tid: 1,
                    registers: vec![6; 128],
                    clear_tid: 0,
                    robust_list: 0,
                    rseq: None,
                    futex: None,
                    pi_futex: None,
                    futex_waitv: None,
                    pi_requeue: None,
                    sleep_deadline: None,
                    signal_frames: Vec::new(),
                    wait: Some((-1, 0x5000, 0x6000)),
                    wait_ready: None,
                    vfork_child: None,
                    signal_wait: false,
                },
                TaskSnapshot {
                    tid: 4,
                    registers: vec![7; 128],
                    clear_tid: 0,
                    robust_list: 0,
                    rseq: None,
                    futex: None,
                    pi_futex: None,
                    futex_waitv: None,
                    pi_requeue: None,
                    sleep_deadline: None,
                    signal_frames: Vec::new(),
                    wait: None,
                    wait_ready: None,
                    vfork_child: Some(2),
                    signal_wait: false,
                },
            ],
            current_task: 0,
            next_tid: 5,
            pid: 1,
            ppid: 0,
            exit_signal: 0,
            vfork_parent: None,
            startup_writes: Vec::new(),
            processes: vec![ProcessSnapshot {
                pid: 2,
                ppid: 1,
                exit_signal: 17,
                vfork_parent: Some(1),
                runtime: child.encode().unwrap(),
                mappings: vec![ProcessMapping {
                    handle: 10,
                    futex_id: 90,
                    address: 0x10_0000_1000,
                    size: 4096,
                    offset: 0,
                    rights: 2 | 4,
                    kind: 2,
                    file_fd: -1,
                    file_offset: 0,
                    shared: false,
                }],
                startup_writes: vec![(0x4000, 2u32.to_ne_bytes().to_vec())],
            }],
            zombies: vec![(3, 1, 7)],
            next_pid: 5,
        };
        let decoded = RuntimeState::decode(&state.encode().unwrap()).unwrap();
        assert_eq!(decoded.tasks, state.tasks);
        assert_eq!(decoded.processes, state.processes);
        assert_eq!(decoded.zombies, state.zombies);
        assert_eq!(decoded.next_pid, 5);
    }
}
