//! BexOS platform boundary for the vendored Starnix core.
//!
//! Phase 3 keeps the upstream sources pinned while exposing the Linux ABI
//! surface used by the BexOS backends. Architecture-specific numbers live in
//! one table so raw syscall constants do not leak into subsystem code.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Architecture {
    Aarch64,
    X86_64,
}

impl Architecture {
    pub const fn current() -> Self {
        if cfg!(bexos_arch_x86_64) {
            Self::X86_64
        } else {
            Self::Aarch64
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Syscall {
    Read,
    Write,
    Readv,
    Writev,
    Preadv,
    Pwritev,
    Preadv2,
    Pwritev2,
    Pread64,
    Pwrite64,
    Sendfile,
    Open,
    OpenAt,
    OpenAt2,
    Close,
    CloseRange,
    Stat,
    Lstat,
    Fstat,
    Statfs,
    Fstatfs,
    NewFstatAt,
    Statx,
    Getdents64,
    Lseek,
    Access,
    FaccessAt,
    FaccessAt2,
    Getcwd,
    Chdir,
    Fchdir,
    Mkdir,
    MkdirAt,
    Unlink,
    UnlinkAt,
    Rmdir,
    Rename,
    RenameAt,
    RenameAt2,
    Link,
    LinkAt,
    Symlink,
    SymlinkAt,
    Readlink,
    ReadlinkAt,
    Truncate,
    Ftruncate,
    Fsync,
    Fdatasync,
    Fadvise64,
    Chmod,
    Fchmod,
    FchmodAt,
    FchmodAt2,
    Chown,
    Fchown,
    FchownAt,
    Lchown,
    Setxattr,
    Lsetxattr,
    Fsetxattr,
    Getxattr,
    Lgetxattr,
    Fgetxattr,
    Listxattr,
    Llistxattr,
    Flistxattr,
    Removexattr,
    Lremovexattr,
    Fremovexattr,
    Dup,
    Dup2,
    Dup3,
    Fcntl,
    Ioctl,
    Pipe,
    Pipe2,
    Poll,
    Ppoll,
    Select,
    Pselect6,
    Eventfd2,
    EpollCreate1,
    EpollCtl,
    EpollWait,
    EpollPwait,
    TimerfdCreate,
    TimerfdSettime,
    TimerfdGettime,
    Socket,
    Socketpair,
    Bind,
    Listen,
    Accept,
    Accept4,
    Connect,
    Getsockname,
    Getpeername,
    Sendto,
    Recvfrom,
    Sendmsg,
    Recvmsg,
    Setsockopt,
    Getsockopt,
    Shutdown,
    Mmap,
    Munmap,
    Mprotect,
    Mremap,
    Msync,
    Madvise,
    Brk,
    Futex,
    FutexWaitv,
    RtSigaction,
    RtSigprocmask,
    RtSigpending,
    RtSigreturn,
    RtSigsuspend,
    Sigaltstack,
    Kill,
    Tgkill,
    ClockGettime,
    Gettimeofday,
    Nanosleep,
    SchedSetparam,
    SchedGetparam,
    SchedSetscheduler,
    SchedGetscheduler,
    SchedGetPriorityMax,
    SchedGetPriorityMin,
    SchedRrGetInterval,
    SchedYield,
    Getrandom,
    Getpid,
    Gettid,
    Getuid,
    Geteuid,
    Getgid,
    Getegid,
    Setuid,
    Setgid,
    Setreuid,
    Setregid,
    Setresuid,
    Setresgid,
    Setfsuid,
    Setfsgid,
    Setgroups,
    Getppid,
    Getpgid,
    Getpgrp,
    Getsid,
    Setsid,
    Getresuid,
    Getresgid,
    Getgroups,
    Getrusage,
    Getpriority,
    Setpriority,
    Times,
    Sysinfo,
    Prctl,
    Getcpu,
    SchedGetaffinity,
    SchedSetaffinity,
    Membarrier,
    ClockGetres,
    ClockNanosleep,
    Umask,
    Uname,
    Getrlimit,
    Prlimit64,
    SetTidAddress,
    SetRobustList,
    GetRobustList,
    ArchPrctl,
    Rseq,
    Clone,
    Clone3,
    Fork,
    Vfork,
    Wait4,
    Execve,
    ExecveAt,
    Exit,
    ExitGroup,
    Unsupported(u64),
}

macro_rules! table {
    ($number:expr, { $($n:literal => $variant:ident,)* }) => {
        match $number { $($n => Syscall::$variant,)* number => Syscall::Unsupported(number) }
    };
}

impl Syscall {
    pub const fn decode(architecture: Architecture, number: u64) -> Self {
        match architecture {
            Architecture::X86_64 => table!(number, {
                0 => Read, 1 => Write, 2 => Open, 3 => Close, 4 => Stat,
                5 => Fstat, 6 => Lstat, 7 => Poll, 8 => Lseek, 9 => Mmap,
                10 => Mprotect, 11 => Munmap, 12 => Brk, 13 => RtSigaction,
                14 => RtSigprocmask, 15 => RtSigreturn, 16 => Ioctl,
                17 => Pread64, 18 => Pwrite64, 19 => Readv, 20 => Writev,
                21 => Access, 22 => Pipe, 23 => Select, 24 => SchedYield,
                25 => Mremap, 26 => Msync, 28 => Madvise, 32 => Dup,
                33 => Dup2, 35 => Nanosleep, 39 => Getpid, 40 => Sendfile, 41 => Socket,
                42 => Connect, 43 => Accept, 44 => Sendto, 45 => Recvfrom,
                46 => Sendmsg, 47 => Recvmsg, 48 => Shutdown, 49 => Bind,
                50 => Listen, 51 => Getsockname, 52 => Getpeername,
                53 => Socketpair, 54 => Setsockopt, 55 => Getsockopt,
                56 => Clone, 57 => Fork, 58 => Vfork, 59 => Execve, 60 => Exit, 61 => Wait4,
                62 => Kill, 63 => Uname, 72 => Fcntl, 74 => Fsync,
                75 => Fdatasync, 76 => Truncate, 77 => Ftruncate,
                79 => Getcwd, 80 => Chdir, 81 => Fchdir, 82 => Rename, 83 => Mkdir,
                84 => Rmdir, 86 => Link, 87 => Unlink, 88 => Symlink,
                89 => Readlink, 90 => Chmod, 91 => Fchmod, 92 => Chown,
                93 => Fchown, 94 => Lchown, 95 => Umask, 96 => Gettimeofday, 97 => Getrlimit,
                98 => Getrusage, 99 => Sysinfo, 100 => Times, 102 => Getuid,
                104 => Getgid, 105 => Setuid, 106 => Setgid,
                107 => Geteuid, 108 => Getegid,
                110 => Getppid, 111 => Getpgrp, 112 => Setsid, 115 => Getgroups,
                113 => Setreuid, 114 => Setregid, 116 => Setgroups,
                117 => Setresuid, 118 => Getresuid, 119 => Setresgid,
                120 => Getresgid, 121 => Getpgid, 122 => Setfsuid, 123 => Setfsgid,
                127 => RtSigpending, 130 => RtSigsuspend, 131 => Sigaltstack, 137 => Statfs,
                138 => Fstatfs, 140 => Getpriority, 141 => Setpriority,
                142 => SchedSetparam, 143 => SchedGetparam,
                144 => SchedSetscheduler, 145 => SchedGetscheduler,
                146 => SchedGetPriorityMax, 147 => SchedGetPriorityMin,
                148 => SchedRrGetInterval,
                158 => ArchPrctl,
                157 => Prctl,
                186 => Gettid, 188 => Setxattr, 189 => Lsetxattr,
                190 => Fsetxattr, 191 => Getxattr, 192 => Lgetxattr,
                193 => Fgetxattr, 194 => Listxattr, 195 => Llistxattr,
                196 => Flistxattr, 197 => Removexattr, 198 => Lremovexattr,
                199 => Fremovexattr, 202 => Futex, 217 => Getdents64,
                203 => SchedSetaffinity, 204 => SchedGetaffinity, 218 => SetTidAddress,
                221 => Fadvise64,
                228 => ClockGettime, 229 => ClockGetres, 230 => ClockNanosleep,
                231 => ExitGroup, 232 => EpollWait, 233 => EpollCtl,
                234 => Tgkill, 257 => OpenAt, 258 => MkdirAt, 260 => FchownAt,
                262 => NewFstatAt, 263 => UnlinkAt, 264 => RenameAt,
                265 => LinkAt, 266 => SymlinkAt, 267 => ReadlinkAt, 268 => FchmodAt,
                269 => FaccessAt, 270 => Pselect6, 271 => Ppoll,
                273 => SetRobustList, 274 => GetRobustList,
                281 => EpollPwait, 283 => TimerfdCreate,
                286 => TimerfdSettime, 287 => TimerfdGettime, 288 => Accept4,
                290 => Eventfd2, 291 => EpollCreate1, 292 => Dup3, 293 => Pipe2,
                295 => Preadv, 296 => Pwritev, 302 => Prlimit64, 309 => Getcpu,
                316 => RenameAt2, 318 => Getrandom,
                322 => ExecveAt, 324 => Membarrier, 332 => Statx,
                327 => Preadv2, 328 => Pwritev2, 334 => Rseq,
                435 => Clone3, 436 => CloseRange, 437 => OpenAt2,
                439 => FaccessAt2, 449 => FutexWaitv, 452 => FchmodAt2,
            }),
            Architecture::Aarch64 => table!(number, {
                5 => Setxattr, 6 => Lsetxattr, 7 => Fsetxattr,
                8 => Getxattr, 9 => Lgetxattr, 10 => Fgetxattr,
                11 => Listxattr, 12 => Llistxattr, 13 => Flistxattr,
                14 => Removexattr, 15 => Lremovexattr, 16 => Fremovexattr,
                17 => Getcwd, 19 => Eventfd2, 20 => EpollCreate1,
                21 => EpollCtl, 22 => EpollPwait, 23 => Dup, 24 => Dup3, 25 => Fcntl,
                29 => Ioctl, 34 => MkdirAt, 35 => UnlinkAt, 36 => SymlinkAt,
                37 => LinkAt, 38 => RenameAt, 43 => Statfs, 44 => Fstatfs,
                45 => Truncate,
                46 => Ftruncate, 48 => FaccessAt, 49 => Chdir, 50 => Fchdir,
                52 => Fchmod, 53 => FchmodAt, 54 => FchownAt, 55 => Fchown,
                56 => OpenAt, 57 => Close, 59 => Pipe2, 61 => Getdents64,
                62 => Lseek, 63 => Read, 64 => Write, 65 => Readv,
                66 => Writev, 67 => Pread64, 68 => Pwrite64, 69 => Preadv,
                70 => Pwritev, 71 => Sendfile,
                72 => Pselect6, 73 => Ppoll, 78 => ReadlinkAt,
                79 => NewFstatAt, 80 => Fstat, 82 => Fsync,
                83 => Fdatasync, 85 => TimerfdCreate, 86 => TimerfdSettime,
                87 => TimerfdGettime, 93 => Exit, 94 => ExitGroup,
                96 => SetTidAddress, 98 => Futex, 99 => SetRobustList,
                100 => GetRobustList,
                101 => Nanosleep, 113 => ClockGettime, 114 => ClockGetres,
                115 => ClockNanosleep, 118 => SchedSetparam,
                119 => SchedSetscheduler, 120 => SchedGetscheduler,
                121 => SchedGetparam, 122 => SchedSetaffinity,
                123 => SchedGetaffinity, 124 => SchedYield,
                125 => SchedGetPriorityMax, 126 => SchedGetPriorityMin,
                127 => SchedRrGetInterval,
                129 => Kill, 131 => Tgkill, 132 => Sigaltstack,
                133 => RtSigsuspend, 134 => RtSigaction,
                135 => RtSigprocmask, 136 => RtSigpending, 139 => RtSigreturn,
                140 => Setpriority,
                141 => Getpriority, 148 => Getresuid,
                143 => Setregid, 144 => Setgid, 145 => Setreuid, 146 => Setuid,
                147 => Setresuid, 149 => Setresgid, 150 => Getresgid,
                151 => Setfsuid, 152 => Setfsgid, 153 => Times,
                155 => Getpgid, 156 => Getsid, 157 => Setsid, 158 => Getgroups,
                159 => Setgroups, 160 => Uname, 165 => Getrusage,
                166 => Umask, 169 => Gettimeofday, 172 => Getpid, 174 => Getuid,
                167 => Prctl, 168 => Getcpu, 173 => Getppid,
                175 => Geteuid, 176 => Getgid, 177 => Getegid,
                178 => Gettid, 198 => Socket, 199 => Socketpair, 200 => Bind,
                201 => Listen, 202 => Accept, 203 => Connect, 204 => Getsockname,
                205 => Getpeername, 206 => Sendto, 207 => Recvfrom,
                208 => Setsockopt, 209 => Getsockopt, 210 => Shutdown,
                211 => Sendmsg, 212 => Recvmsg, 214 => Brk, 215 => Munmap, 216 => Mremap,
                220 => Clone, 221 => Execve,
                222 => Mmap, 223 => Fadvise64, 226 => Mprotect, 227 => Msync,
                233 => Madvise, 242 => Accept4, 261 => Prlimit64, 276 => RenameAt2,
                278 => Getrandom, 286 => Preadv2, 287 => Pwritev2,
                260 => Wait4, 281 => ExecveAt, 283 => Membarrier, 291 => Statx, 293 => Rseq,
                435 => Clone3,
                437 => OpenAt2, 439 => FaccessAt2, 449 => FutexWaitv, 452 => FchmodAt2,
            }),
        }
    }
}

pub const EPERM: i64 = 1;
pub const ENOENT: i64 = 2;
pub const EINTR: i64 = 4;
pub const ESRCH: i64 = 3;
pub const EIO: i64 = 5;
pub const EBADF: i64 = 9;
pub const ECHILD: i64 = 10;
pub const EAGAIN: i64 = 11;
pub const ENOMEM: i64 = 12;
pub const EACCES: i64 = 13;
pub const EFAULT: i64 = 14;
pub const EBUSY: i64 = 16;
pub const EEXIST: i64 = 17;
pub const EXDEV: i64 = 18;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const EMFILE: i64 = 24;
pub const ENOSPC: i64 = 28;
pub const EPIPE: i64 = 32;
pub const EDEADLK: i64 = 35;
pub const ESPIPE: i64 = 29;
pub const EROFS: i64 = 30;
pub const ENOSYS: i64 = 38;
pub const ENODATA: i64 = 61;
pub const ENOTEMPTY: i64 = 39;
pub const ELOOP: i64 = 40;
pub const ERANGE: i64 = 34;
pub const ENOTSUP: i64 = 95;
pub const EAFNOSUPPORT: i64 = 97;
pub const ETIMEDOUT: i64 = 110;
pub const EOWNERDEAD: i64 = 130;
pub const ENOTSOCK: i64 = 88;
pub const EADDRINUSE: i64 = 98;
pub const EISCONN: i64 = 106;
pub const ENOTCONN: i64 = 107;
pub const ECONNREFUSED: i64 = 111;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Task {
    exit_code: Option<i32>,
}

impl Task {
    pub const fn new() -> Self {
        Self { exit_code: None }
    }
    pub fn exit(&mut self, code: u64) {
        self.exit_code = Some(code as u8 as i32);
    }
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}

pub const fn error(errno: i64) -> u64 {
    (-errno) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_numbers_match_both_linux_abis() {
        assert_eq!(Syscall::decode(Architecture::X86_64, 0), Syscall::Read);
        assert_eq!(Syscall::decode(Architecture::Aarch64, 63), Syscall::Read);
        assert_eq!(
            Syscall::decode(Architecture::X86_64, 13),
            Syscall::RtSigaction
        );
        assert_eq!(
            Syscall::decode(Architecture::Aarch64, 134),
            Syscall::RtSigaction
        );
        assert_eq!(Syscall::decode(Architecture::X86_64, 437), Syscall::OpenAt2);
        assert_eq!(
            Syscall::decode(Architecture::X86_64, 291),
            Syscall::EpollCreate1
        );
        assert_eq!(
            Syscall::decode(Architecture::Aarch64, 20),
            Syscall::EpollCreate1
        );
        assert_eq!(Syscall::decode(Architecture::X86_64, 56), Syscall::Clone);
        assert_eq!(Syscall::decode(Architecture::Aarch64, 220), Syscall::Clone);
        assert_eq!(Syscall::decode(Architecture::X86_64, 435), Syscall::Clone3);
        assert_eq!(
            Syscall::decode(Architecture::X86_64, 449),
            Syscall::FutexWaitv
        );
        assert_eq!(
            Syscall::decode(Architecture::Aarch64, 449),
            Syscall::FutexWaitv
        );
        assert_eq!(
            Syscall::decode(Architecture::X86_64, 274),
            Syscall::GetRobustList
        );
        assert_eq!(
            Syscall::decode(Architecture::Aarch64, 100),
            Syscall::GetRobustList
        );
        assert_eq!(
            Syscall::decode(Architecture::Aarch64, 439),
            Syscall::FaccessAt2
        );
        assert_eq!(
            Syscall::decode(Architecture::X86_64, 203),
            Syscall::SchedSetaffinity
        );
        assert_eq!(Syscall::decode(Architecture::X86_64, 90), Syscall::Chmod);
        assert_eq!(Syscall::decode(Architecture::Aarch64, 52), Syscall::Fchmod);
        assert_eq!(
            Syscall::decode(Architecture::X86_64, 452),
            Syscall::FchmodAt2
        );
        assert_eq!(
            Syscall::decode(Architecture::Aarch64, 999),
            Syscall::Unsupported(999)
        );
    }

    #[test]
    fn current_architecture_follows_the_bazel_guest_configuration() {
        #[cfg(bexos_arch_x86_64)]
        assert_eq!(Architecture::current(), Architecture::X86_64);
        #[cfg(not(bexos_arch_x86_64))]
        assert_eq!(Architecture::current(), Architecture::Aarch64);
    }

    #[test]
    fn task_exit_and_linux_errors_are_stable() {
        let mut task = Task::new();
        task.exit(0x123);
        assert_eq!(task.exit_code(), Some(0x23));
        assert_eq!(error(ENOSYS) as i64, -38);
    }
}
