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
    Pread64,
    Pwrite64,
    Open,
    OpenAt,
    OpenAt2,
    Close,
    CloseRange,
    Stat,
    Lstat,
    Fstat,
    NewFstatAt,
    Statx,
    Getdents64,
    Lseek,
    Access,
    FaccessAt,
    Getcwd,
    Chdir,
    Mkdir,
    MkdirAt,
    Unlink,
    UnlinkAt,
    Rmdir,
    Rename,
    RenameAt,
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
    Mmap,
    Munmap,
    Mprotect,
    Mremap,
    Msync,
    Madvise,
    Brk,
    Futex,
    RtSigaction,
    RtSigprocmask,
    RtSigreturn,
    RtSigsuspend,
    Sigaltstack,
    Kill,
    Tgkill,
    ClockGettime,
    Gettimeofday,
    Nanosleep,
    SchedYield,
    Getrandom,
    Getpid,
    Gettid,
    Getuid,
    Geteuid,
    Getgid,
    Getegid,
    Uname,
    Prlimit64,
    SetTidAddress,
    SetRobustList,
    ArchPrctl,
    Rseq,
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
                33 => Dup2, 35 => Nanosleep, 39 => Getpid, 60 => Exit,
                62 => Kill, 63 => Uname, 72 => Fcntl, 74 => Fsync,
                75 => Fdatasync, 76 => Truncate, 77 => Ftruncate,
                79 => Getcwd, 80 => Chdir, 82 => Rename, 83 => Mkdir,
                84 => Rmdir, 86 => Link, 87 => Unlink, 88 => Symlink,
                89 => Readlink, 96 => Gettimeofday, 102 => Getuid,
                104 => Getgid, 107 => Geteuid, 108 => Getegid,
                130 => RtSigsuspend, 131 => Sigaltstack, 158 => ArchPrctl,
                186 => Gettid, 202 => Futex, 217 => Getdents64,
                218 => SetTidAddress, 228 => ClockGettime, 231 => ExitGroup,
                234 => Tgkill, 257 => OpenAt, 258 => MkdirAt,
                262 => NewFstatAt, 263 => UnlinkAt, 264 => RenameAt,
                265 => LinkAt, 266 => SymlinkAt, 267 => ReadlinkAt,
                269 => FaccessAt, 270 => Pselect6, 271 => Ppoll,
                273 => SetRobustList, 292 => Dup3, 293 => Pipe2,
                302 => Prlimit64, 318 => Getrandom, 332 => Statx,
                334 => Rseq, 436 => CloseRange, 437 => OpenAt2,
            }),
            Architecture::Aarch64 => table!(number, {
                17 => Getcwd, 23 => Dup, 24 => Dup3, 25 => Fcntl,
                29 => Ioctl, 34 => MkdirAt, 35 => UnlinkAt, 36 => SymlinkAt,
                37 => LinkAt, 38 => RenameAt, 45 => Truncate,
                46 => Ftruncate, 48 => FaccessAt, 49 => Chdir,
                56 => OpenAt, 57 => Close, 59 => Pipe2, 61 => Getdents64,
                62 => Lseek, 63 => Read, 64 => Write, 65 => Readv,
                66 => Writev, 67 => Pread64, 68 => Pwrite64,
                72 => Pselect6, 73 => Ppoll, 78 => ReadlinkAt,
                79 => NewFstatAt, 80 => Fstat, 82 => Fsync,
                83 => Fdatasync, 93 => Exit, 94 => ExitGroup,
                96 => SetTidAddress, 98 => Futex, 99 => SetRobustList,
                101 => Nanosleep, 113 => ClockGettime, 124 => SchedYield,
                129 => Kill, 131 => Tgkill, 132 => Sigaltstack,
                133 => RtSigsuspend, 134 => RtSigaction,
                135 => RtSigprocmask, 139 => RtSigreturn, 160 => Uname,
                169 => Gettimeofday, 172 => Getpid, 174 => Getuid,
                175 => Geteuid, 176 => Getgid, 177 => Getegid,
                178 => Gettid, 214 => Brk, 215 => Munmap, 216 => Mremap,
                222 => Mmap, 226 => Mprotect, 227 => Msync,
                233 => Madvise, 261 => Prlimit64, 278 => Getrandom,
                291 => Statx, 293 => Rseq, 436 => CloseRange,
                437 => OpenAt2,
            }),
        }
    }
}

pub const EPERM: i64 = 1;
pub const ENOENT: i64 = 2;
pub const EINTR: i64 = 4;
pub const EIO: i64 = 5;
pub const EBADF: i64 = 9;
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
pub const ESPIPE: i64 = 29;
pub const EROFS: i64 = 30;
pub const ENOSYS: i64 = 38;
pub const ENOTEMPTY: i64 = 39;
pub const ELOOP: i64 = 40;
pub const ERANGE: i64 = 34;
pub const ENOTSUP: i64 = 95;

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
            Syscall::decode(Architecture::Aarch64, 999),
            Syscall::Unsupported(999)
        );
    }

    #[test]
    fn task_exit_and_linux_errors_are_stable() {
        let mut task = Task::new();
        task.exit(0x123);
        assert_eq!(task.exit_code(), Some(0x23));
        assert_eq!(error(ENOSYS) as i64, -38);
    }
}
