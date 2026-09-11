#![no_std]
#![allow(non_snake_case)]

extern crate alloc;
mod command;
mod dynamic_loading;
pub use dynamic_loading::*;
mod math;
mod numbers;
mod strings;
pub use numbers::*;
pub use strings::*;
mod pthread_attr;
mod pthread_condition;
mod pthread_keys;
mod pthread_sync;
mod termination;
mod time_sleep;
pub use pthread_attr::*;
pub use pthread_condition::*;
pub use pthread_keys::*;
pub use pthread_sync::*;
pub use termination::*;
pub use time_sleep::*;

#[cfg(all(bexos_guest, target_arch = "aarch64"))]
core::arch::global_asm!(
    r#"
    .section .note.gnu.property, "a", %note
    .p2align 3
    .long 4
    .long 16
    .long 5
    .asciz "GNU"
    .p2align 3
    .long 0xc0000000
    .long 4
    .long 3
    .long 0
    .p2align 3
"#
);

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ffi::{c_char, c_int, c_long, c_short, c_void};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

const ENOENT: c_int = 2;
const ENOSYS: c_int = 38;
const EINVAL: c_int = 22;
const EINTR: c_int = 4;
const EIO: c_int = 5;
const ENOMEM: c_int = 12;
const EBADF: c_int = 9;
const EAGAIN: c_int = 11;
const EACCES: c_int = 13;
const EEXIST: c_int = 17;
const ENOTDIR: c_int = 20;
const EISDIR: c_int = 21;
const ENAMETOOLONG: c_int = 36;
const ENOTEMPTY: c_int = 39;
const ENETUNREACH: c_int = 101;
const EAFNOSUPPORT: c_int = 97;
const EOPNOTSUPP: c_int = 95;
const ECONNRESET: c_int = 104;
const ETIMEDOUT: c_int = 110;
const EROFS: c_int = 30;
const ENOPROTOOPT: c_int = 92;

const AF_INET: c_int = 2;
const AF_INET6: c_int = 10;
const SOCK_STREAM: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const SOL_SOCKET: c_int = 1;
const SO_REUSEADDR: c_int = 2;
const SO_TYPE: c_int = 3;
const SO_ERROR: c_int = 4;
const SO_KEEPALIVE: c_int = 9;
const SO_BROADCAST: c_int = 6;
const SO_REUSEPORT: c_int = 15;
const IPPROTO_TCP: c_int = 6;
const TCP_NODELAY: c_int = 1;
const O_WRONLY: c_int = 1;
const O_RDWR: c_int = 2;
const O_CREAT: c_int = 0o100;
const O_EXCL: c_int = 0o200;
const O_TRUNC: c_int = 0o1000;
const O_APPEND: c_int = 0o2000;
const F_GETFL: c_int = 3;
const F_GETFD: c_int = 1;
const F_SETFD: c_int = 2;
const F_SETFL: c_int = 4;
const F_DUPFD: c_int = 0;
const F_DUPFD_CLOEXEC: c_int = 1030;
const FD_CLOEXEC: c_int = 1;
const O_NONBLOCK: c_int = 0o4000;
const O_DIRECTORY: c_int = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    65536
} else {
    0x4000
};
const O_CLOEXEC: c_int = 0x80000;
const AT_FDCWD: c_int = -100;
const AT_REMOVEDIR: c_int = 0x200;
const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;
const SEEK_END: c_int = 2;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const POLLIN: c_short = 0x001;
const POLLOUT: c_short = 0x004;
const POLLERR: c_short = 0x008;
const POLLHUP: c_short = 0x010;
const CLOCK_REALTIME: c_int = 0;
const CLOCK_MONOTONIC: c_int = 1;
const CLOCK_BOOTTIME: c_int = 7;
const SYS_GETTID: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    186
} else {
    178
};
const SYS_FUTEX: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    202
} else {
    98
};
const SYS_EPOLL_CREATE1: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    291
} else {
    20
};
const SYS_EPOLL_CTL: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    233
} else {
    21
};
const SYS_EPOLL_PWAIT: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    281
} else {
    22
};
const SYS_EVENTFD2: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    290
} else {
    19
};
const SYS_TIMERFD_CREATE: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    283
} else {
    85
};
const SYS_TIMERFD_SETTIME: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    286
} else {
    86
};
const SYS_GETRANDOM: c_long = if cfg!(all(bexos_guest, target_arch = "x86_64")) {
    318
} else {
    278
};
const FUTEX_WAIT: c_int = 0;
const FUTEX_WAKE: c_int = 1;
const EPOLL_CTL_ADD: c_int = 1;
const EPOLL_CTL_DEL: c_int = 2;
const EPOLL_CTL_MOD: c_int = 3;
const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const GRND_NONBLOCK: c_int = 0x0001;
const _SC_NPROCESSORS_ONLN: c_int = 84;
const _SC_PAGESIZE: c_int = 30;
const DIRECT_ALLOC: usize = 1usize << (usize::BITS - 1);
const ALLOCATION_HEADER_SIZE: usize = core::mem::size_of::<usize>() * 3;

static HEAP: bexos_allocator::Heap = bexos_allocator::Heap::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static HEAP_VMAR: AtomicU64 = AtomicU64::new(0);
static ERRNO: AtomicI32 = AtomicI32::new(0);
// ID 1 belongs to the initial thread's TLS record.
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(2);
#[cfg(not(bexos_guest))]
static HOST_THREAD_LOCAL: AtomicUsize = AtomicUsize::new(0);
#[cfg(not(bexos_guest))]
static HOST_PTHREAD_VALUES: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static NETSTACK: AtomicU64 = AtomicU64::new(0);
static TIME_PAGE_VADDR: AtomicUsize = AtomicUsize::new(0);
static NAMESPACE: Locked<NamespaceTable> = Locked::new(NamespaceTable::new());
static FD_TABLE: Locked<FdTable> = Locked::new(FdTable::new());
static THREADS: Locked<ThreadTable> = Locked::new(ThreadTable::new());

pub mod allocation_stats;

pub struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { alloc_with_align(layout.size(), layout.align()).cast() }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe {
            free(ptr.cast());
        }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        unsafe { realloc_with_align(pointer.cast(), size, layout.align()).cast() }
    }
}

#[macro_export]
macro_rules! entry {
    ($entry:path) => {
        #[global_allocator]
        static BEXOS_LIBC_ALLOCATOR: bexos_libc::Allocator = bexos_libc::Allocator;

        #[cfg_attr(bexos_guest, unsafe(no_mangle))]
        pub extern "C" fn _start(channel: u64) -> ! {
            unsafe {
                bexos_libc::init();
            }
            $entry(channel)
        }
    };
}

pub unsafe fn init() {
    if INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        unsafe {
            if let Some((base, size)) = bexos_userspace::initial_heap_region() {
                HEAP_VMAR.store(bexos_userspace::syscall::heap_vmar(), Ordering::Release);
                HEAP.init(base, size);
            }
        }
    }
    let _ = ensure_initial_thread_local();
}

pub fn install_startup(startup: &bexos_userspace::Startup) {
    for grant in &startup.service_grants {
        if grant.service == "bexos.net.Netstack" {
            NETSTACK.store(grant.endpoint, Ordering::Release);
        }
    }
    NAMESPACE.with(|namespace| namespace.install_entries(&startup.namespace));
    command::install(startup);
}

fn set_errno(value: c_int) {
    unsafe {
        __errno_location().write(value);
    }
}

fn fail<T>(errno: c_int, value: T) -> T {
    set_errno(errno);
    value
}

fn errno_from_kernel(status: kernel_fidl::Status) -> c_int {
    match status {
        kernel_fidl::Status::ErrInvalidHandle => EBADF,
        kernel_fidl::Status::ErrNoMemory => ENOMEM,
        kernel_fidl::Status::ErrTimedOut => ETIMEDOUT,
        kernel_fidl::Status::ErrPeerClosed => ECONNRESET,
        kernel_fidl::Status::ErrResourceExhausted => EAGAIN,
        _ => EINVAL,
    }
}

fn errno_from_net(status: net_fidl::Status) -> c_int {
    match status {
        net_fidl::Status::ErrInvalidHandle => EBADF,
        net_fidl::Status::ErrNoMemory => ENOMEM,
        net_fidl::Status::ErrNetworkUnreachable => ENETUNREACH,
        net_fidl::Status::ErrShouldWait => EAGAIN,
        net_fidl::Status::ErrPeerClosed => ECONNRESET,
        net_fidl::Status::ErrTimedOut => ETIMEDOUT,
        _ => EINVAL,
    }
}

fn errno_from_fs(status: fs_fidl::FsStatus) -> c_int {
    match status {
        fs_fidl::FsStatus::NotFound => ENOENT,
        fs_fidl::FsStatus::NotDirectory => ENOTDIR,
        fs_fidl::FsStatus::IsDirectory => EISDIR,
        fs_fidl::FsStatus::NotEmpty => ENOTEMPTY,
        fs_fidl::FsStatus::NoSpace => ENOMEM,
        fs_fidl::FsStatus::ReadOnly => EROFS,
        fs_fidl::FsStatus::AccessDenied => EACCES,
        fs_fidl::FsStatus::InvalidArgs => EINVAL,
        fs_fidl::FsStatus::AlreadyExists => EEXIST,
        fs_fidl::FsStatus::BadState => EBADF,
        _ => EINVAL,
    }
}

struct Locked<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T> Sync for Locked<T> {}

impl<T> Locked<T> {
    const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self.locked.swap(true, Ordering::Acquire) {
            bexos_userspace::yield_now();
        }
        let result = f(unsafe { &mut *self.value.get() });
        self.locked.store(false, Ordering::Release);
        result
    }
}

#[derive(Clone, Copy)]
enum FdKind {
    Empty,
    File {
        channel: u64,
    },
    Directory {
        channel: u64,
    },
    PendingTcp,
    TcpStream {
        socket: u64,
        control: u64,
    },
    TcpListener {
        control: u64,
    },
    Udp {
        control: u64,
    },
    Epoll {
        entries: [EpollEntry; 32],
        len: usize,
    },
    Event {
        value: u64,
    },
    Timer {
        deadline_ns: u64,
    },
}

#[derive(Clone, Copy)]
struct FdEntry {
    kind: FdKind,
    flags: c_int,
    refs: usize,
}

#[derive(Clone, Copy)]
struct EpollEntry {
    fd: c_int,
    events: u32,
    data: u64,
}

struct FdTable {
    entries: [FdEntry; 128],
}

#[derive(Clone)]
struct NamespaceEntry {
    path: String,
    root: u64,
}

struct NamespaceTable {
    entries: Vec<NamespaceEntry>,
}

impl NamespaceTable {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn install_entries(&mut self, entries: &[bexos_userspace::NamespaceEntry]) {
        self.entries.clear();
        for entry in entries {
            if entry.directory != 0 {
                self.entries.push(NamespaceEntry {
                    path: normalize_mount_path(&entry.path),
                    root: entry.directory,
                });
            }
        }
    }

    fn resolve(&self, path: &str) -> Result<ResolvedPath, c_int> {
        if path.is_empty() || path.as_bytes().contains(&0) {
            return Err(EINVAL);
        }
        let absolute = path.starts_with('/');
        let lookup_path = if absolute {
            path
        } else if self.entries.iter().any(|entry| entry.path == "/cwd") {
            "/cwd"
        } else {
            "/data"
        };
        let relative_tail = if absolute { None } else { Some(path) };
        let mut best: Option<&NamespaceEntry> = None;
        for entry in &self.entries {
            if mount_matches(lookup_path, &entry.path)
                && best.is_none_or(|prior| entry.path.len() > prior.path.len())
            {
                best = Some(entry);
            }
        }
        let Some(entry) = best else {
            return Err(ENOENT);
        };
        let tail = match relative_tail {
            Some(tail) => tail,
            None => lookup_path
                .strip_prefix(&entry.path)
                .unwrap_or("")
                .strip_prefix('/')
                .unwrap_or(""),
        };
        let relative = normalize_relative_path(tail)?;
        Ok(ResolvedPath {
            root: entry.root,
            relative,
        })
    }
}

struct ResolvedPath {
    root: u64,
    relative: String,
}

#[repr(C)]
struct Iovec {
    base: *const c_void,
    len: usize,
}

#[cfg(not(all(bexos_guest, target_arch = "x86_64")))]
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxStat64 {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    __pad1: u64,
    st_size: i64,
    st_blksize: i32,
    __pad2: i32,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: c_long,
    st_mtime: i64,
    st_mtime_nsec: c_long,
    st_ctime: i64,
    st_ctime_nsec: c_long,
    __unused: [c_int; 2],
}

#[cfg(all(bexos_guest, target_arch = "x86_64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxStat64 {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_uid: u32,
    st_gid: u32,
    __pad0: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: c_long,
    st_mtime: i64,
    st_mtime_nsec: c_long,
    st_ctime: i64,
    st_ctime_nsec: c_long,
    __unused: [i64; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxDirent64 {
    d_ino: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
    d_name: [c_char; 256],
}

struct DirStream {
    fd: c_int,
    index: usize,
    entries: Vec<DirEntryCache>,
    current: LinuxDirent64,
}

struct DirEntryCache {
    name: String,
    kind: fs_fidl::NodeKind,
}

#[derive(Clone, Copy)]
struct ThreadStart {
    id: usize,
    start: extern "C" fn(*mut c_void) -> *mut c_void,
    arg: *mut c_void,
    result: *mut c_void,
    done: bool,
    started: bool,
    kernel_handle: u64,
    detached: bool,
    joining: bool,
    stack: *mut c_void,
    stack_size: usize,
    thread_pointer: *mut c_void,
}

const PTHREAD_LOCAL_RESERVED_SIZE: usize = 528;

#[repr(C, align(16))]
struct ThreadLocal {
    self_ptr: *mut ThreadLocal,
    pthread_id: usize,
    values: [usize; 64],
}

const _: () = assert!(core::mem::size_of::<ThreadLocal>() <= PTHREAD_LOCAL_RESERVED_SIZE);

struct ThreadTable {
    entries: [Option<ThreadStart>; 64],
}

impl ThreadTable {
    const fn new() -> Self {
        Self {
            entries: [None; 64],
        }
    }

    fn insert(&mut self, entry: ThreadStart) -> Result<(), c_int> {
        for slot in &mut self.entries {
            if slot.is_none() {
                *slot = Some(entry);
                return Ok(());
            }
        }
        Err(ENOMEM)
    }

    fn take_pending(&mut self, id: usize) -> Option<ThreadStart> {
        for slot in &mut self.entries {
            if slot.is_some_and(|entry| entry.id == id && !entry.done) {
                return slot.take();
            }
        }
        None
    }

    fn take_pending_on_stack(&mut self, address: usize) -> Option<ThreadStart> {
        for slot in &mut self.entries {
            if let Some(entry) = slot {
                if !entry.started
                    && !entry.done
                    && address >= entry.stack as usize
                    && address < (entry.stack as usize).saturating_add(entry.stack_size)
                {
                    entry.started = true;
                    return Some(*entry);
                }
            }
        }
        None
    }

    fn finish(&mut self, id: usize, result: *mut c_void) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.id == id)
        {
            entry.result = result;
            entry.done = true;
        }
        // The caller still uses its stack and TLS until ExitThread commits.
        // Joining/reaping must observe the kernel's TERMINATED signal first.
    }

    fn join(&mut self, id: usize) -> Option<ThreadStart> {
        for slot in &mut self.entries {
            if slot.is_some_and(|entry| entry.id == id && entry.done && entry.kernel_handle != 0) {
                return slot.take();
            }
        }
        None
    }

    fn claim_join(&mut self, id: usize) -> Result<u64, c_int> {
        let entry = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.id == id)
            .ok_or(3)?; // ESRCH
        if entry.detached || entry.joining {
            return Err(EINVAL);
        }
        if entry.kernel_handle == 0 {
            return Err(3);
        }
        entry.joining = true;
        Ok(entry.kernel_handle)
    }

    fn cancel_join(&mut self, id: usize) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.id == id)
        {
            entry.joining = false;
        }
    }
}

unsafe fn initialize_thread_local(local: *mut ThreadLocal, pthread_id: usize) {
    unsafe {
        local.write(ThreadLocal {
            self_ptr: local,
            pthread_id,
            values: [0; 64],
        });
    }
}

mod arch;
#[cfg(bexos_guest)]
use arch::*;

#[cfg(not(bexos_guest))]
unsafe fn alloc_thread_state(pthread_id: usize) -> *mut c_void {
    let local = unsafe { alloc_with_align(core::mem::size_of::<ThreadLocal>(), 16) };
    if local.is_null() {
        return ptr::null_mut();
    }
    unsafe { initialize_thread_local(local.cast(), pthread_id) };
    local
}
#[cfg(not(bexos_guest))]
fn set_thread_pointer(thread_pointer: *mut c_void) {
    HOST_THREAD_LOCAL.store(thread_pointer as usize, Ordering::Release);
}
#[cfg(not(bexos_guest))]
fn current_thread_local() -> *mut ThreadLocal {
    HOST_THREAD_LOCAL.load(Ordering::Acquire) as *mut ThreadLocal
}

fn ensure_initial_thread_local() -> *mut ThreadLocal {
    let existing = current_thread_local();
    if !existing.is_null() {
        if unsafe { (*existing).self_ptr != existing } {
            unsafe { initialize_thread_local(existing, 1) };
        }
        return existing;
    }
    let thread_pointer = unsafe { alloc_thread_state(1) };
    if !thread_pointer.is_null() {
        set_thread_pointer(thread_pointer);
    }
    current_thread_local()
}

impl FdTable {
    const fn new() -> Self {
        const EMPTY: FdEntry = FdEntry {
            kind: FdKind::Empty,
            flags: 0,
            refs: 0,
        };
        Self {
            entries: [EMPTY; 128],
        }
    }

    fn alloc(&mut self, kind: FdKind) -> Result<c_int, c_int> {
        for index in 3..self.entries.len() {
            if matches!(self.entries[index].kind, FdKind::Empty) {
                self.entries[index] = FdEntry {
                    kind,
                    flags: 0,
                    refs: 1,
                };
                return Ok(index as c_int);
            }
        }
        Err(ENOMEM)
    }

    fn get(&self, fd: c_int) -> Result<FdEntry, c_int> {
        let entry = self.entries.get(fd as usize).ok_or(EBADF)?;
        if matches!(entry.kind, FdKind::Empty) {
            Err(EBADF)
        } else {
            Ok(*entry)
        }
    }

    fn set_kind(&mut self, fd: c_int, kind: FdKind) -> Result<(), c_int> {
        let entry = self.entries.get_mut(fd as usize).ok_or(EBADF)?;
        if matches!(entry.kind, FdKind::Empty) {
            return Err(EBADF);
        }
        entry.kind = kind;
        Ok(())
    }

    fn close(&mut self, fd: c_int) -> Result<(), c_int> {
        let entry = self.entries.get_mut(fd as usize).ok_or(EBADF)?;
        if matches!(entry.kind, FdKind::Empty) {
            return Err(EBADF);
        }
        if entry.refs > 1 {
            entry.refs -= 1;
            return Ok(());
        }
        match entry.kind {
            FdKind::File { channel } | FdKind::Directory { channel } => {
                let _ = bexos_userspace::fs::close(bexos_userspace::Channel(channel));
            }
            FdKind::TcpStream { socket, control: 0 } => {
                let _ = bexos_userspace::Memory::close(socket);
            }
            _ => {}
        }
        *entry = FdEntry {
            kind: FdKind::Empty,
            flags: 0,
            refs: 0,
        };
        Ok(())
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

#[repr(C)]
struct SockAddrIn6 {
    sin6_family: u16,
    sin6_port: u16,
    sin6_flowinfo: u32,
    sin6_addr: [u8; 16],
    sin6_scope_id: u32,
}

#[repr(C)]
pub struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

#[cfg_attr(all(bexos_guest, target_arch = "x86_64"), repr(C, packed))]
#[cfg_attr(not(all(bexos_guest, target_arch = "x86_64")), repr(C))]
struct EpollEvent {
    events: u32,
    data: u64,
}

fn now_ns() -> u64 {
    let ticks = bexos_userspace::syscall::ticks();
    let freq = bexos_userspace::syscall::frequency().max(1);
    (u128::from(ticks) * 1_000_000_000u128 / u128::from(freq)) as u64
}

fn vdso_time_page() -> Option<&'static bexos_time_abi::TimePageV1> {
    let mapped = TIME_PAGE_VADDR.load(Ordering::Acquire);
    if mapped == usize::MAX {
        return None;
    }
    if mapped != 0 {
        return Some(unsafe { &*(mapped as *const bexos_time_abi::TimePageV1) });
    }
    let response: kernel_fidl::ClockGetVdsoTimePageResponse = bexos_userspace::ipc::kernel_call(
        8,
        "GetVdsoTimePage",
        kernel_fidl::CLOCK_PUBLIC_METHODS,
        &kernel_fidl::ClockGetVdsoTimePageRequest {},
    )
    .ok()?;
    if response.status != kernel_fidl::Status::Ok || response.vmo.raw == 0 {
        TIME_PAGE_VADDR.store(usize::MAX, Ordering::Release);
        return None;
    }
    let vaddr = match bexos_userspace::memory::Memory::map(response.vmo.raw, 4096, 2) {
        Ok(vaddr) => vaddr,
        Err(_) => {
            TIME_PAGE_VADDR.store(usize::MAX, Ordering::Release);
            return None;
        }
    };
    let _ = bexos_userspace::memory::Memory::close(response.vmo.raw);
    TIME_PAGE_VADDR.store(vaddr as usize, Ordering::Release);
    Some(unsafe { &*(vaddr as *const bexos_time_abi::TimePageV1) })
}

fn clock_fast(clock_type: kernel_fidl::ClockType) -> Option<u64> {
    let page = vdso_time_page()?;
    let snapshot = page.read_snapshot()?;
    let ticks = bexos_userspace::syscall::ticks();
    match clock_type {
        kernel_fidl::ClockType::Monotonic | kernel_fidl::ClockType::BootTime => {
            Some(snapshot.monotonic_from_ticks(ticks))
        }
        kernel_fidl::ClockType::Realtime => snapshot.realtime_from_ticks(ticks),
    }
}

fn clock_fidl(clock_type: kernel_fidl::ClockType) -> Result<u64, c_int> {
    let r: kernel_fidl::ClockGetTimeResponse = bexos_userspace::ipc::kernel_call(
        8,
        "GetTime",
        kernel_fidl::CLOCK_PUBLIC_METHODS,
        &kernel_fidl::ClockGetTimeRequest { clock_type },
    )
    .map_err(errno_from_kernel)?;
    if r.status == kernel_fidl::Status::Ok {
        Ok(r.nanos)
    } else {
        Err(errno_from_kernel(r.status))
    }
}

fn parse_sockaddr(addr: *const c_void, len: u32) -> Result<net_fidl::SocketAddress, c_int> {
    if addr.is_null() || len < 2 {
        return Err(EINVAL);
    }
    let family = unsafe { *(addr as *const u16) } as c_int;
    match family {
        AF_INET if len as usize >= core::mem::size_of::<SockAddrIn>() => {
            let a = unsafe { &*(addr as *const SockAddrIn) };
            Ok(net_fidl::SocketAddress {
                addr: net_fidl::IpAddress::Ipv4(net_fidl::Ipv4Address { octets: a.sin_addr }),
                port: u16::from_be(a.sin_port),
            })
        }
        AF_INET6 if len as usize >= core::mem::size_of::<SockAddrIn6>() => {
            let a = unsafe { &*(addr as *const SockAddrIn6) };
            Ok(net_fidl::SocketAddress {
                addr: net_fidl::IpAddress::Ipv6(net_fidl::Ipv6Address {
                    octets: a.sin6_addr,
                }),
                port: u16::from_be(a.sin6_port),
            })
        }
        _ => Err(EAFNOSUPPORT),
    }
}

fn write_sockaddr(
    value: net_fidl::SocketAddress,
    addr: *mut c_void,
    len: *mut u32,
) -> Result<(), c_int> {
    if addr.is_null() || len.is_null() {
        return Ok(());
    }
    match value.addr {
        net_fidl::IpAddress::Ipv4(ip) => {
            let need = core::mem::size_of::<SockAddrIn>() as u32;
            unsafe {
                if *len < need {
                    *len = need;
                    return Err(EINVAL);
                }
                *(addr as *mut SockAddrIn) = SockAddrIn {
                    sin_family: AF_INET as u16,
                    sin_port: value.port.to_be(),
                    sin_addr: ip.octets,
                    sin_zero: [0; 8],
                };
                *len = need;
            }
        }
        net_fidl::IpAddress::Ipv6(ip) => {
            let need = core::mem::size_of::<SockAddrIn6>() as u32;
            unsafe {
                if *len < need {
                    *len = need;
                    return Err(EINVAL);
                }
                *(addr as *mut SockAddrIn6) = SockAddrIn6 {
                    sin6_family: AF_INET6 as u16,
                    sin6_port: value.port.to_be(),
                    sin6_flowinfo: 0,
                    sin6_addr: ip.octets,
                    sin6_scope_id: 0,
                };
                *len = need;
            }
        }
    }
    Ok(())
}

fn netstack_client() -> Result<net_fidl::NetstackPublicClient<bexos_userspace::Rpc>, c_int> {
    let raw = NETSTACK.load(Ordering::Acquire);
    if raw == 0 {
        return Err(ENETUNREACH);
    }
    Ok(net_fidl::NetstackPublicClient::new(bexos_userspace::Rpc(
        bexos_userspace::Channel(raw),
    )))
}

fn channel_pair() -> Result<(u64, u64), c_int> {
    bexos_userspace::Channel::pair()
        .map(|(a, b)| (a.0, b.0))
        .map_err(errno_from_kernel)
}

fn default_options() -> net_fidl::SocketOptions {
    net_fidl::SocketOptions {
        non_blocking: None,
        keep_alive_ms: None,
        rx_buffer_size: None,
        tx_buffer_size: None,
    }
}

fn tcp_get_stream(control: u64) -> Result<u64, c_int> {
    let mut client = net_fidl::TcpSocketPublicClient::new(bexos_userspace::Rpc(
        bexos_userspace::Channel(control),
    ));
    let mut rb = [0; 64];
    let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
    let mut ob = [0; 64];
    let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
    let r = client
        .get_stream(
            &net_fidl::TcpSocketGetStreamRequest {},
            &mut rb,
            &mut rh,
            &mut ob,
            &mut oh,
        )
        .map_err(|_| EINVAL)?;
    if r.status == net_fidl::Status::Ok {
        Ok(r.socket.raw)
    } else {
        Err(errno_from_net(r.status))
    }
}

fn tcp_addr(control: u64, local: bool) -> Result<net_fidl::SocketAddress, c_int> {
    let mut client = net_fidl::TcpSocketPublicClient::new(bexos_userspace::Rpc(
        bexos_userspace::Channel(control),
    ));
    let mut rb = [0; 64];
    let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
    let mut ob = [0; 128];
    let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
    if local {
        let r = client
            .get_local_address(
                &net_fidl::TcpSocketGetLocalAddressRequest {},
                &mut rb,
                &mut rh,
                &mut ob,
                &mut oh,
            )
            .map_err(|_| EINVAL)?;
        if r.status == net_fidl::Status::Ok {
            Ok(r.addr)
        } else {
            Err(errno_from_net(r.status))
        }
    } else {
        let r = client
            .get_peer_address(
                &net_fidl::TcpSocketGetPeerAddressRequest {},
                &mut rb,
                &mut rh,
                &mut ob,
                &mut oh,
            )
            .map_err(|_| EINVAL)?;
        if r.status == net_fidl::Status::Ok {
            Ok(r.addr)
        } else {
            Err(errno_from_net(r.status))
        }
    }
}

fn cstr_len(s: *const c_char) -> usize {
    let mut len = 0;
    unsafe {
        while *s.add(len) != 0 {
            len += 1;
        }
    }
    len
}

fn cstr(path: *const c_char) -> Result<&'static str, c_int> {
    if path.is_null() {
        return Err(EINVAL);
    }
    let bytes = unsafe { core::slice::from_raw_parts(path.cast::<u8>(), cstr_len(path)) };
    core::str::from_utf8(bytes).map_err(|_| EINVAL)
}

fn normalize_mount_path(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    let mut out = String::new();
    if !path.starts_with('/') {
        out.push('/');
    }
    out.push_str(path.trim_end_matches('/'));
    out
}

fn mount_matches(path: &str, mount: &str) -> bool {
    mount == "/"
        || path == mount
        || path
            .strip_prefix(mount)
            .is_some_and(|tail| tail.starts_with('/'))
}

fn normalize_relative_path(path: &str) -> Result<String, c_int> {
    let mut out = String::new();
    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." || component.len() > 255 {
            return Err(EINVAL);
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(component);
    }
    if out.len() > 255 {
        Err(ENAMETOOLONG)
    } else {
        Ok(out)
    }
}

fn split_parent_leaf(path: &str) -> Result<(String, String), c_int> {
    let normalized = normalize_relative_path(path)?;
    let Some((parent, leaf)) = normalized.rsplit_once('/') else {
        if normalized.is_empty() {
            return Err(EINVAL);
        }
        return Ok((String::new(), normalized));
    };
    if leaf.is_empty() {
        return Err(EINVAL);
    }
    Ok((parent.to_string(), leaf.to_string()))
}

fn resolve_path(path: *const c_char) -> Result<ResolvedPath, c_int> {
    let path = cstr(path)?;
    NAMESPACE.with(|namespace| namespace.resolve(path))
}

fn fs_flags(flags: c_int) -> u32 {
    let mut out = 0;
    match flags & 3 {
        O_WRONLY => out |= 2,
        O_RDWR => out |= 1 | 2,
        _ => out |= 1,
    }
    if flags & O_CREAT != 0 {
        out |= 8;
    }
    if flags & O_TRUNC != 0 {
        out |= 16;
    }
    if flags & O_DIRECTORY != 0 {
        out |= 32;
    }
    out
}

fn open_node(root: u64, relative: &str, flags: u32) -> Result<bexos_userspace::Channel, c_int> {
    if relative.is_empty() {
        bexos_userspace::Memory::duplicate(root, 1 | 2 | 4 | 32)
            .map(bexos_userspace::Channel)
            .map_err(errno_from_kernel)
    } else {
        bexos_userspace::fs::open(bexos_userspace::Channel(root), relative, flags)
            .map_err(errno_from_fs)
    }
}

fn open_resolved(path: ResolvedPath, flags: c_int) -> Result<c_int, c_int> {
    if path.relative.is_empty() && flags & O_CREAT != 0 {
        return Err(EISDIR);
    }
    if flags & O_EXCL != 0 && flags & O_CREAT != 0 {
        let probe = open_node(path.root, &path.relative, 1);
        if let Ok(channel) = probe {
            let _ = bexos_userspace::fs::close(channel);
            return Err(EEXIST);
        }
    }
    let channel = open_node(path.root, &path.relative, fs_flags(flags))?;
    let attr = bexos_userspace::fs::attributes(channel).map_err(|e| {
        let _ = bexos_userspace::fs::close(channel);
        errno_from_fs(e)
    })?;
    if flags & O_APPEND != 0 {
        let _ = bexos_userspace::fs::seek(channel, attr.size_bytes as i64);
    }
    let kind = if flags & O_DIRECTORY != 0 {
        FdKind::Directory { channel: channel.0 }
    } else {
        FdKind::File { channel: channel.0 }
    };
    FD_TABLE.with(|t| t.alloc(kind)).inspect_err(|_| {
        let _ = bexos_userspace::fs::close(channel);
    })
}

fn file_attr_to_stat(attr: fs_fidl::FileAttributes, kind: fs_fidl::NodeKind) -> LinuxStat64 {
    let mode_type = match kind {
        fs_fidl::NodeKind::Directory => S_IFDIR,
        fs_fidl::NodeKind::File => S_IFREG,
    };
    LinuxStat64 {
        st_dev: 1,
        st_ino: 1,
        st_mode: mode_type | attr.mode,
        st_nlink: 1,
        st_uid: 0,
        st_gid: 0,
        st_rdev: 0,
        #[cfg(not(all(bexos_guest, target_arch = "x86_64")))]
        __pad1: 0,
        #[cfg(all(bexos_guest, target_arch = "x86_64"))]
        __pad0: 0,
        st_size: attr.size_bytes as i64,
        st_blksize: 4096,
        #[cfg(not(all(bexos_guest, target_arch = "x86_64")))]
        __pad2: 0,
        st_blocks: attr.storage_allocated_bytes.div_ceil(512) as i64,
        st_atime: (attr.modification_time_nanos / 1_000_000_000) as i64,
        st_atime_nsec: (attr.modification_time_nanos % 1_000_000_000) as c_long,
        st_mtime: (attr.modification_time_nanos / 1_000_000_000) as i64,
        st_mtime_nsec: (attr.modification_time_nanos % 1_000_000_000) as c_long,
        st_ctime: (attr.creation_time_nanos / 1_000_000_000) as i64,
        st_ctime_nsec: (attr.creation_time_nanos % 1_000_000_000) as c_long,
        #[cfg(not(all(bexos_guest, target_arch = "x86_64")))]
        __unused: [0; 2],
        #[cfg(all(bexos_guest, target_arch = "x86_64"))]
        __unused: [0; 3],
    }
}

fn fd_stat(entry: FdEntry) -> Result<LinuxStat64, c_int> {
    match entry.kind {
        FdKind::File { channel } => {
            bexos_userspace::fs::attributes(bexos_userspace::Channel(channel))
                .map(|attr| file_attr_to_stat(attr, fs_fidl::NodeKind::File))
                .map_err(errno_from_fs)
        }
        FdKind::Directory { channel } => {
            bexos_userspace::fs::attributes(bexos_userspace::Channel(channel))
                .map(|attr| file_attr_to_stat(attr, fs_fidl::NodeKind::Directory))
                .map_err(errno_from_fs)
        }
        _ => Err(EBADF),
    }
}

fn path_stat(path: *const c_char) -> Result<LinuxStat64, c_int> {
    let resolved = resolve_path(path)?;
    let channel = open_node(resolved.root, &resolved.relative, 1)?;
    let attr = bexos_userspace::fs::attributes(channel).map_err(errno_from_fs)?;
    let entries = bexos_userspace::fs::read_entries(channel);
    let kind = if entries.is_ok() {
        fs_fidl::NodeKind::Directory
    } else {
        fs_fidl::NodeKind::File
    };
    let _ = bexos_userspace::fs::close(channel);
    Ok(file_attr_to_stat(attr, kind))
}

fn empty_dirent64() -> LinuxDirent64 {
    LinuxDirent64 {
        d_ino: 0,
        d_off: 0,
        d_reclen: core::mem::size_of::<LinuxDirent64>() as u16,
        d_type: 0,
        d_name: [0; 256],
    }
}

fn dirent64(entry: &DirEntryCache, index: usize) -> LinuxDirent64 {
    let mut out = empty_dirent64();
    out.d_ino = (index + 1) as u64;
    out.d_off = (index + 1) as i64;
    out.d_type = match entry.kind {
        fs_fidl::NodeKind::Directory => DT_DIR,
        fs_fidl::NodeKind::File => DT_REG,
    };
    for (index, byte) in entry.name.as_bytes().iter().take(255).enumerate() {
        out.d_name[index] = *byte as c_char;
    }
    out
}

fn fd_read(entry: FdEntry, buf: *mut c_void, count: usize) -> Result<usize, c_int> {
    if buf.is_null() {
        return Err(EINVAL);
    }
    match entry.kind {
        FdKind::File { channel } => {
            let bytes = bexos_userspace::fs::read(bexos_userspace::Channel(channel), count as u64)
                .map_err(errno_from_fs)?;
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast(), bytes.len()) };
            Ok(bytes.len())
        }
        FdKind::TcpStream { socket, control: 0 } if entry.flags & O_NONBLOCK == 0 => loop {
            match bexos_userspace::Socket(socket).read(count.min(32768) as u32) {
                Ok(bytes) => {
                    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast(), bytes.len()) };
                    break Ok(bytes.len());
                }
                Err(kernel_fidl::Status::ErrTimedOut) => {
                    bexos_userspace::Socket(socket)
                        .wait_io(false, -1)
                        .map_err(errno_from_kernel)?;
                }
                Err(e) => break Err(errno_from_kernel(e)),
            }
        },
        FdKind::TcpStream { socket, .. } => {
            match bexos_userspace::Socket(socket).read(count as u32) {
                Ok(bytes) => {
                    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast(), bytes.len()) };
                    Ok(bytes.len())
                }
                Err(kernel_fidl::Status::ErrTimedOut) if entry.flags & O_NONBLOCK != 0 => {
                    Err(EAGAIN)
                }
                Err(e) => Err(errno_from_kernel(e)),
            }
        }
        FdKind::Event { value } => {
            if count < 8 {
                return Err(EINVAL);
            }
            unsafe { (buf as *mut u64).write(value) };
            Ok(8)
        }
        FdKind::Timer { deadline_ns, .. } => {
            if count < 8 {
                return Err(EINVAL);
            }
            let expirations = if now_ns() >= deadline_ns { 1 } else { 0 };
            if expirations == 0 && entry.flags & O_NONBLOCK != 0 {
                return Err(EAGAIN);
            }
            unsafe { (buf as *mut u64).write(expirations) };
            Ok(8)
        }
        _ => Err(EBADF),
    }
}

fn fd_write(entry: FdEntry, buf: *const c_void, count: usize) -> Result<usize, c_int> {
    if buf.is_null() {
        return Err(EINVAL);
    }
    let bytes = unsafe { core::slice::from_raw_parts(buf.cast::<u8>(), count) };
    match entry.kind {
        FdKind::File { channel } => {
            bexos_userspace::fs::write(bexos_userspace::Channel(channel), bytes)
                .map(|()| bytes.len())
                .map_err(errno_from_fs)
        }
        FdKind::TcpStream { socket, control: 0 } if entry.flags & O_NONBLOCK == 0 => loop {
            match bexos_userspace::Socket(socket).write(&bytes[..bytes.len().min(32768)]) {
                Ok(0) if !bytes.is_empty() => {
                    bexos_userspace::Socket(socket)
                        .wait_io(true, -1)
                        .map_err(errno_from_kernel)?;
                }
                Ok(n) => break Ok(n as usize),
                Err(e) => break Err(errno_from_kernel(e)),
            }
        },
        FdKind::TcpStream { socket, .. } => bexos_userspace::Socket(socket)
            .write(bytes)
            .map(|n| n as usize)
            .map_err(errno_from_kernel),
        FdKind::Event { .. } => {
            if count < 8 {
                Err(EINVAL)
            } else {
                Ok(8)
            }
        }
        _ => Err(EBADF),
    }
}

fn readiness(entry: FdEntry, wanted: c_short) -> c_short {
    let mut ready = 0;
    match entry.kind {
        FdKind::TcpStream { socket, .. } => match bexos_userspace::Socket(socket).info() {
            Ok(info) => {
                if wanted & POLLIN != 0 && (info.readable_bytes > 0 || info.peer_write_closed) {
                    ready |= POLLIN;
                }
                if wanted & POLLOUT != 0 && !info.local_write_closed && !info.peer_read_closed {
                    ready |= POLLOUT;
                }
                if info.peer_read_closed || info.peer_write_closed {
                    ready |= POLLHUP;
                }
            }
            Err(_) => ready |= POLLERR,
        },
        FdKind::TcpListener { control: _ } => {
            if wanted & POLLIN != 0 {
                ready |= POLLIN;
            }
        }
        FdKind::Udp { .. } => {
            if wanted & POLLIN != 0 {
                ready |= POLLIN;
            }
            if wanted & POLLOUT != 0 {
                ready |= POLLOUT;
            }
        }
        FdKind::Event { value } => {
            if wanted & POLLIN != 0 && value != 0 {
                ready |= POLLIN;
            }
            if wanted & POLLOUT != 0 {
                ready |= POLLOUT;
            }
        }
        FdKind::Timer { deadline_ns, .. } => {
            if wanted & POLLIN != 0 && now_ns() >= deadline_ns {
                ready |= POLLIN;
            }
        }
        _ => {}
    }
    ready
}

unsafe fn allocation_layout(size: usize, align: usize) -> Option<(Layout, usize)> {
    let align = align
        .max(core::mem::align_of::<usize>())
        .next_power_of_two();
    let total = size
        .checked_add(align.checked_sub(1)?)?
        .checked_add(ALLOCATION_HEADER_SIZE)?;
    Some((Layout::from_size_align(total, align).ok()?, align))
}

unsafe fn alloc_with_align(size: usize, align: usize) -> *mut c_void {
    let result = unsafe { alloc_untracked(size, align) };
    allocation_stats::allocated(size, result.is_null());
    result
}
unsafe fn alloc_untracked(size: usize, align: usize) -> *mut c_void {
    unsafe {
        init();
        let size = size.max(1);
        let Some((layout, align)) = allocation_layout(size, align) else {
            set_errno(ENOMEM);
            return ptr::null_mut();
        };
        if layout.size() as u64 > bexos_boot::USER_HEAP_CHUNK_SIZE {
            return alloc_direct(size, align);
        }
        let mut base = HEAP.alloc(layout);
        if base.is_null() {
            if let Some((region, region_size)) = bexos_userspace::initial_heap_region() {
                HEAP.add_region(region, region_size);
                base = HEAP.alloc(layout);
            }
        }
        if base.is_null() {
            bexos_userspace::log("bexos-libc: heap allocation failed after growth\n");
            set_errno(ENOMEM);
            return ptr::null_mut();
        }
        let user = base.add(ALLOCATION_HEADER_SIZE).add(align - 1).addr() & !(align - 1);
        let user = user as *mut u8;
        (user.sub(core::mem::size_of::<usize>()) as *mut usize).write(0);
        (user.sub(core::mem::size_of::<usize>() * 2) as *mut usize).write(size);
        (user.sub(core::mem::size_of::<usize>() * 3) as *mut usize).write(base.addr());
        user.cast()
    }
}

unsafe fn alloc_direct(size: usize, align: usize) -> *mut c_void {
    let heap = HEAP_VMAR.load(Ordering::Acquire);
    if heap == 0 {
        set_errno(ENOMEM);
        return ptr::null_mut();
    }
    let Some(total) = size
        .checked_add(align.checked_sub(1).unwrap_or(0))
        .and_then(|n| n.checked_add(ALLOCATION_HEADER_SIZE))
    else {
        set_errno(ENOMEM);
        return ptr::null_mut();
    };
    let Some(mapped_size) = bexos_boot::page_round(total as u64) else {
        set_errno(ENOMEM);
        return ptr::null_mut();
    };
    let vmo = match bexos_userspace::Memory::create(mapped_size, 0) {
        Ok(vmo) => vmo,
        Err(status) => {
            log_direct_allocation_failure(mapped_size, status as i32);
            set_errno(ENOMEM);
            return ptr::null_mut();
        }
    };
    let mapped = match bexos_userspace::Memory::map_vmo(heap, vmo, 0, 0, mapped_size, 0x1 | 0x2) {
        Ok(mapped) => mapped as usize,
        Err(_) => {
            bexos_userspace::log("bexos-libc: direct allocation VMO map failed\n");
            let _ = bexos_userspace::Memory::close(vmo);
            set_errno(ENOMEM);
            return ptr::null_mut();
        }
    };
    let _ = bexos_userspace::Memory::close(vmo);
    let user = (mapped + ALLOCATION_HEADER_SIZE + align - 1) & !(align - 1);
    unsafe {
        ((user - core::mem::size_of::<usize>() * 3) as *mut usize).write(mapped);
        ((user - core::mem::size_of::<usize>() * 2) as *mut usize).write(size);
        ((user - core::mem::size_of::<usize>()) as *mut usize)
            .write(mapped_size as usize | DIRECT_ALLOC);
    }
    user as *mut c_void
}

fn log_direct_allocation_failure(mapped_size: u64, status: i32) {
    const PREFIX: &[u8] = b"bexos-libc: direct allocation VMO create failed bytes=";
    const STATUS: &[u8] = b" status=";
    let mut output = [0u8; 96];
    let mut len = PREFIX.len();
    output[..len].copy_from_slice(PREFIX);
    len += write_decimal(&mut output[len..], mapped_size);
    output[len..len + STATUS.len()].copy_from_slice(STATUS);
    len += STATUS.len();
    if status < 0 {
        output[len] = b'-';
        len += 1;
    }
    len += write_decimal(&mut output[len..], u64::from(status.unsigned_abs()));
    output[len] = b'\n';
    len += 1;
    bexos_userspace::log(unsafe { core::str::from_utf8_unchecked(&output[..len]) });
}

fn write_decimal(output: &mut [u8], mut value: u64) -> usize {
    if value == 0 {
        output[0] = b'0';
        return 1;
    }
    let digits = value.ilog10() as usize + 1;
    for index in (0..digits).rev() {
        output[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    digits
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    // GNU max_align_t is 16 bytes on both supported 64-bit C ABIs.
    unsafe { alloc_with_align(size, 16) }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    let Some(total) = count.checked_mul(size) else {
        allocation_stats::failed();
        set_errno(ENOMEM);
        return ptr::null_mut();
    };
    unsafe {
        let p = malloc(total);
        if !p.is_null() {
            ptr::write_bytes(p, 0, total);
        }
        p
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    if !ptr.is_null() {
        allocation_stats::freed();
    }
    unsafe { free_untracked(ptr) }
}
unsafe fn free_untracked(ptr: *mut c_void) {
    unsafe {
        if ptr.is_null() {
            return;
        }
        let user = ptr.cast::<u8>();
        let base = (user.sub(core::mem::size_of::<usize>() * 3) as *const usize).read();
        let metadata = (user.sub(core::mem::size_of::<usize>()) as *const usize).read();
        if metadata & DIRECT_ALLOC != 0 {
            let _ = bexos_userspace::Memory::unmap_vmar(
                HEAP_VMAR.load(Ordering::Acquire),
                base as u64,
                (metadata & !DIRECT_ALLOC) as u64,
            );
            return;
        }
        HEAP.dealloc(
            base as *mut u8,
            Layout::from_size_align_unchecked(1, core::mem::align_of::<usize>()),
        );
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    unsafe { realloc_with_align(ptr, size, core::mem::align_of::<usize>()) }
}

unsafe fn realloc_with_align(ptr: *mut c_void, size: usize, align: usize) -> *mut c_void {
    if ptr.is_null() {
        return unsafe { alloc_with_align(size, align) };
    }
    if size == 0 {
        unsafe { free(ptr) };
        return ptr::null_mut();
    }
    let result = unsafe { realloc_untracked(ptr, size, align) };
    allocation_stats::reallocated(size, result.is_null());
    result
}
unsafe fn realloc_untracked(ptr: *mut c_void, size: usize, align: usize) -> *mut c_void {
    unsafe {
        if ptr.is_null() {
            return alloc_untracked(size, align);
        }
        if size == 0 {
            free_untracked(ptr);
            return ptr::null_mut();
        }
        let user = ptr.cast::<u8>();
        let old_size = (user.sub(core::mem::size_of::<usize>() * 2) as *const usize).read();
        let base = (user.sub(core::mem::size_of::<usize>() * 3) as *const usize).read();
        let metadata = (user.sub(core::mem::size_of::<usize>()) as *const usize).read();
        if metadata & DIRECT_ALLOC == 0 {
            if let Some(total) = size.checked_add(user.addr() - base) {
                if HEAP.resize_in_place(base as *mut u8, total) {
                    (user.sub(core::mem::size_of::<usize>() * 2) as *mut usize).write(size);
                    return ptr;
                }
            }
        }
        let next = alloc_untracked(size, align);
        if !next.is_null() {
            ptr::copy_nonoverlapping(ptr.cast::<u8>(), next.cast::<u8>(), old_size.min(size));
            free_untracked(ptr);
        }
        next
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int {
    if out.is_null() || !align.is_power_of_two() || align < core::mem::size_of::<usize>() {
        allocation_stats::failed();
        return EINVAL;
    }
    unsafe {
        let ptr = alloc_with_align(size, align);
        if ptr.is_null() {
            ENOMEM
        } else {
            out.write(ptr);
            0
        }
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    unsafe { arch::memcpy(dst, src, n) }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    unsafe { arch::memmove(dst, src, n) }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn memset(dst: *mut c_void, value: c_int, n: usize) -> *mut c_void {
    unsafe { arch::memset(dst, value, n) }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int {
    unsafe {
        for i in 0..n {
            let av = *a.cast::<u8>().add(i);
            let bv = *b.cast::<u8>().add(i);
            if av != bv {
                return av as c_int - bv as c_int;
            }
        }
        0
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn bcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int {
    unsafe { memcmp(a, b, n) }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn strlen(s: *const c_char) -> usize {
    unsafe {
        let mut n = 0;
        while *s.add(n) != 0 {
            n += 1;
        }
        n
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn __errno_location() -> *mut c_int {
    #[cfg(bexos_guest)]
    {
        // Slot zero is reserved; key tokens only use slots 1 through 63.
        // Avoid allocating here: allocation failures themselves need errno.
        let local = current_thread_local();
        if !local.is_null() {
            if unsafe { (*local).self_ptr != local } {
                unsafe {
                    initialize_thread_local(local, 1);
                }
            }
            return unsafe { core::ptr::addr_of_mut!((*local).values[0]).cast() };
        }
    }
    &ERRNO as *const AtomicI32 as *mut c_int
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn getauxval(_kind: c_long) -> c_long {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn dl_iterate_phdr(
    _callback: extern "C" fn(*mut c_void, usize, *mut c_void) -> c_int,
    _data: *mut c_void,
) -> c_int {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_Resume() -> ! {
    bexos_userspace::exit()
}

// Native stack unwinding is unavailable on BexOS. Backtrace traversal above
// returns end-of-stack; these complete the accessors referenced by Rust std.
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_GetCFA(_ctx: *mut c_void) -> usize {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_FindEnclosingFunction(_pc: *mut c_void) -> *mut c_void {
    ptr::null_mut()
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_GetIP(_ctx: *mut c_void) -> usize {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_GetIPInfo(_ctx: *mut c_void, ip_before_insn: *mut c_int) -> usize {
    unsafe {
        if !ip_before_insn.is_null() {
            ip_before_insn.write(0);
        }
    }
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_GetLanguageSpecificData(_ctx: *mut c_void) -> usize {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_GetRegionStart(_ctx: *mut c_void) -> usize {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_GetTextRelBase(_ctx: *mut c_void) -> usize {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_GetDataRelBase(_ctx: *mut c_void) -> usize {
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_SetGR(_ctx: *mut c_void, _index: c_int, _value: usize) {}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_SetIP(_ctx: *mut c_void, _value: usize) {}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn _Unwind_Backtrace(
    _callback: extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    _data: *mut c_void,
) -> c_int {
    5
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn abort() -> ! {
    bexos_userspace::log("libc: abort\n");
    bexos_userspace::exit()
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn getenv(name: *const c_char) -> *mut c_char {
    unsafe { command::getenv(name) }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn secure_getenv(name: *const c_char) -> *mut c_char {
    // BexOS has no setuid execution; appd installs the authorized environment
    // for each launched process instead of inheriting ambient shell state.
    unsafe { command::getenv(name) }
}

fn errno_message(errno: c_int) -> &'static [u8] {
    match errno {
        ENOENT => b"not found\0",
        EBADF => b"bad file descriptor\0",
        EAGAIN => b"try again\0",
        ENOMEM => b"out of memory\0",
        EACCES => b"access denied\0",
        EEXIST => b"already exists\0",
        EINVAL => b"invalid argument\0",
        ENOTDIR => b"not a directory\0",
        EISDIR => b"is a directory\0",
        EROFS => b"read-only filesystem\0",
        ENOSYS => b"not implemented\0",
        ENOPROTOOPT => b"protocol option not available\0",
        ENOTEMPTY => b"directory not empty\0",
        EAFNOSUPPORT => b"address family not supported\0",
        EOPNOTSUPP => b"operation not supported\0",
        ENETUNREACH => b"network unreachable\0",
        ECONNRESET => b"connection reset\0",
        ETIMEDOUT => b"timed out\0",
        _ => b"unknown error\0",
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn __xpg_strerror_r(errno: c_int, buf: *mut c_char, buflen: usize) -> c_int {
    if buf.is_null() || buflen == 0 {
        return EINVAL;
    }
    let message = errno_message(errno);
    let n = message.len().min(buflen);
    unsafe {
        ptr::copy_nonoverlapping(message.as_ptr(), buf.cast::<u8>(), n);
        if n == buflen {
            buf.add(buflen - 1).write(0);
        }
    }
    0
}

macro_rules! enosys {
    ($name:ident ( $($arg:ident : $ty:ty),* ) -> $ret:ty, $fail:expr) => {
        #[cfg_attr(bexos_guest, unsafe(no_mangle))]
        pub extern "C" fn $name($($arg: $ty),*) -> $ret {
            let _ = ($($arg),*);
            set_errno(ENOSYS);
            $fail
        }
    };
}

enosys!(munmap(addr: *mut c_void, len: usize) -> c_int, -1);
enosys!(mmap64(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, offset: i64) -> *mut c_void, !0usize as *mut c_void);
enosys!(getcwd(buf: *mut c_char, size: usize) -> *mut c_char, ptr::null_mut());
enosys!(readlink(path: *const c_char, buf: *mut c_char, size: usize) -> isize, -1);
enosys!(getpriority(which: c_int, who: u32) -> c_int, -1);
enosys!(chdir(path: *const c_char) -> c_int, -1);
enosys!(pipe2(fds: *mut c_int, flags: c_int) -> c_int, -1);
enosys!(rename(old: *const c_char, new: *const c_char) -> c_int, -1);
enosys!(chmod(path: *const c_char, mode: u32) -> c_int, -1);

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn sysconf(name: c_int) -> c_long {
    match name {
        _SC_PAGESIZE => 4096,
        // The kernel currently exposes no online-CPU query to this ABI.
        // Consumers must use their documented fallback, never a guessed mask.
        _SC_NPROCESSORS_ONLN => fail(ENOSYS, -1),
        _ => fail(EINVAL, -1),
    }
}

// No memory-protection or POSIX signal-stack syscall is exposed by BexOS.
// In particular, never claim that a requested guard page has been installed.
enosys!(mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int, -1);
enosys!(sigaltstack(ss: *const c_void, old_ss: *mut c_void) -> c_int, -1);

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pause() -> c_int {
    bexos_userspace::yield_now();
    fail(EINTR, -1)
}

enosys!(sched_getaffinity(pid: c_int, cpusetsize: usize, mask: *mut c_void) -> c_int, -1);

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn ioctl(fd: c_int, request: c_long, arg: c_long) -> c_int {
    const FIONBIO: c_long = 0x5421;
    if request != FIONBIO {
        return fail(ENOSYS, -1);
    }
    if arg == 0 {
        return fail(EINVAL, -1);
    }
    FD_TABLE.with(|t| {
        let Some(entry) = t.entries.get_mut(fd as usize) else {
            return fail(EBADF, -1);
        };
        if matches!(entry.kind, FdKind::Empty) {
            return fail(EBADF, -1);
        }
        let enabled = unsafe { *(arg as *const c_int) } != 0;
        if enabled {
            entry.flags |= O_NONBLOCK;
        } else {
            entry.flags &= !O_NONBLOCK;
        }
        0
    })
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn open64(path: *const c_char, flags: c_int, _mode: c_int) -> c_int {
    match resolve_path(path).and_then(|resolved| open_resolved(resolved, flags)) {
        Ok(fd) => fd,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn open(path: *const c_char, flags: c_int, mode: c_int) -> c_int {
    open64(path, flags, mode)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn openat(dirfd: c_int, path: *const c_char, flags: c_int, mode: c_int) -> c_int {
    if path.is_null() {
        return fail(EINVAL, -1);
    }
    let Ok(path_str) = cstr(path) else {
        return fail(EINVAL, -1);
    };
    if path_str.starts_with('/') || dirfd == AT_FDCWD {
        return open64(path, flags, mode);
    }
    let entry = match FD_TABLE.with(|t| t.get(dirfd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::Directory { channel } = entry.kind else {
        return fail(ENOTDIR, -1);
    };
    let relative = match normalize_relative_path(path_str) {
        Ok(relative) => relative,
        Err(e) => return fail(e, -1),
    };
    let opened = bexos_userspace::fs::open(
        bexos_userspace::Channel(channel),
        &relative,
        fs_flags(flags),
    )
    .map_err(errno_from_fs)
    .and_then(|channel| {
        let kind = if flags & O_DIRECTORY != 0 {
            FdKind::Directory { channel: channel.0 }
        } else {
            FdKind::File { channel: channel.0 }
        };
        FD_TABLE.with(|t| t.alloc(kind)).inspect_err(|_| {
            let _ = bexos_userspace::fs::close(channel);
        })
    });
    match opened {
        Ok(fd) => fd,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn writev(fd: c_int, iov: *const c_void, iovcnt: c_int) -> isize {
    if iovcnt < 0 || (iov.is_null() && iovcnt != 0) {
        return fail(EINVAL, -1);
    }
    let mut written = 0isize;
    for index in 0..iovcnt as usize {
        let item = unsafe { &*(iov as *const Iovec).add(index) };
        let n = write(fd, item.base, item.len);
        if n < 0 {
            return if written == 0 { -1 } else { written };
        }
        written += n;
    }
    written
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn lseek64(fd: c_int, offset: i64, whence: c_int) -> i64 {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::File { channel } = entry.kind else {
        return fail(EBADF, -1);
    };
    let whence = match whence {
        SEEK_SET => 0,
        SEEK_CUR => 1,
        SEEK_END => 2,
        _ => return fail(EINVAL, -1),
    };
    let result =
        bexos_userspace::fs::seek_with_whence(bexos_userspace::Channel(channel), offset, whence)
            .map_err(errno_from_fs);
    match result {
        Ok(pos) => pos as i64,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn fstat(fd: c_int, stat: *mut c_void) -> c_int {
    // Both supported GNU 64-bit ABIs use the same stat and stat64 layout.
    fstat64(fd, stat)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn fstat64(fd: c_int, stat: *mut c_void) -> c_int {
    if stat.is_null() {
        return fail(EINVAL, -1);
    }
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    match fd_stat(entry) {
        Ok(value) => {
            unsafe { (stat as *mut LinuxStat64).write(value) };
            0
        }
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn stat64(path: *const c_char, stat: *mut c_void) -> c_int {
    if stat.is_null() {
        return fail(EINVAL, -1);
    }
    match path_stat(path) {
        Ok(value) => {
            unsafe { (stat as *mut LinuxStat64).write(value) };
            0
        }
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn lstat64(path: *const c_char, stat: *mut c_void) -> c_int {
    stat64(path, stat)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn fstatat64(
    dirfd: c_int,
    path: *const c_char,
    stat: *mut c_void,
    _flags: c_int,
) -> c_int {
    if path.is_null() || stat.is_null() {
        return fail(EINVAL, -1);
    }
    let Ok(path_str) = cstr(path) else {
        return fail(EINVAL, -1);
    };
    if path_str.starts_with('/') || dirfd == AT_FDCWD {
        return stat64(path, stat);
    }
    let entry = match FD_TABLE.with(|t| t.get(dirfd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::Directory { channel } = entry.kind else {
        return fail(ENOTDIR, -1);
    };
    let relative = match normalize_relative_path(path_str) {
        Ok(relative) => relative,
        Err(e) => return fail(e, -1),
    };
    let result = open_node(channel, &relative, 1).and_then(|child| {
        let attr = bexos_userspace::fs::attributes(child).map_err(errno_from_fs)?;
        let kind = if bexos_userspace::fs::read_entries(child).is_ok() {
            fs_fidl::NodeKind::Directory
        } else {
            fs_fidl::NodeKind::File
        };
        let _ = bexos_userspace::fs::close(child);
        Ok(file_attr_to_stat(attr, kind))
    });
    match result {
        Ok(value) => {
            unsafe { (stat as *mut LinuxStat64).write(value) };
            0
        }
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn ftruncate64(fd: c_int, length: i64) -> c_int {
    if length < 0 {
        return fail(EINVAL, -1);
    }
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::File { channel } = entry.kind else {
        return fail(EBADF, -1);
    };
    match bexos_userspace::fs::set_len(bexos_userspace::Channel(channel), length as u64) {
        Ok(()) => 0,
        Err(e) => fail(errno_from_fs(e), -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn fsync(fd: c_int) -> c_int {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::File { channel } = entry.kind else {
        return fail(EBADF, -1);
    };
    match bexos_userspace::fs::sync_file(bexos_userspace::Channel(channel)) {
        Ok(()) => 0,
        Err(e) => fail(errno_from_fs(e), -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn fdatasync(fd: c_int) -> c_int {
    fsync(fd)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn mkdir(path: *const c_char, _mode: u32) -> c_int {
    let resolved = match resolve_path(path) {
        Ok(resolved) => resolved,
        Err(e) => return fail(e, -1),
    };
    if resolved.relative.is_empty() {
        return fail(EEXIST, -1);
    }
    if let Ok(channel) = open_node(resolved.root, &resolved.relative, 1 | 32) {
        let _ = bexos_userspace::fs::close(channel);
        return fail(EEXIST, -1);
    }
    match open_node(resolved.root, &resolved.relative, 1 | 2 | 8 | 32) {
        Ok(channel) => {
            let _ = bexos_userspace::fs::close(channel);
            0
        }
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn unlink(path: *const c_char) -> c_int {
    let resolved = match resolve_path(path) {
        Ok(resolved) => resolved,
        Err(e) => return fail(e, -1),
    };
    let (parent, leaf) = match split_parent_leaf(&resolved.relative) {
        Ok(parts) => parts,
        Err(e) => return fail(e, -1),
    };
    let parent = match open_node(resolved.root, &parent, 1 | 2 | 32) {
        Ok(parent) => parent,
        Err(e) => return fail(e, -1),
    };
    let result = bexos_userspace::fs::unlink(parent, &leaf).map_err(errno_from_fs);
    let _ = bexos_userspace::fs::close(parent);
    match result {
        Ok(()) => 0,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn rmdir(path: *const c_char) -> c_int {
    unlink(path)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn unlinkat(dirfd: c_int, path: *const c_char, flags: c_int) -> c_int {
    if path.is_null() {
        return fail(EINVAL, -1);
    }
    let Ok(path_str) = cstr(path) else {
        return fail(EINVAL, -1);
    };
    if path_str.starts_with('/') || dirfd == AT_FDCWD {
        return if flags & AT_REMOVEDIR != 0 {
            rmdir(path)
        } else {
            unlink(path)
        };
    }
    let entry = match FD_TABLE.with(|t| t.get(dirfd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::Directory { channel } = entry.kind else {
        return fail(ENOTDIR, -1);
    };
    let (parent, leaf) = match split_parent_leaf(path_str) {
        Ok(parts) => parts,
        Err(e) => return fail(e, -1),
    };
    let parent = match open_node(channel, &parent, 1 | 2 | 32) {
        Ok(parent) => parent,
        Err(e) => return fail(e, -1),
    };
    let result = bexos_userspace::fs::unlink(parent, &leaf).map_err(errno_from_fs);
    let _ = bexos_userspace::fs::close(parent);
    match result {
        Ok(()) => 0,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn opendir(path: *const c_char) -> *mut c_void {
    let fd = open64(path, O_DIRECTORY | O_CLOEXEC, 0);
    if fd < 0 {
        return ptr::null_mut();
    }
    fdopendir(fd)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn fdopendir(fd: c_int) -> *mut c_void {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, ptr::null_mut()),
    };
    let FdKind::Directory { channel } = entry.kind else {
        return fail(ENOTDIR, ptr::null_mut());
    };
    let entries = match bexos_userspace::fs::read_entries(bexos_userspace::Channel(channel)) {
        Ok(entries) => entries
            .into_iter()
            .map(|entry| DirEntryCache {
                name: entry.name,
                kind: entry.kind,
            })
            .collect(),
        Err(e) => return fail(errno_from_fs(e), ptr::null_mut()),
    };
    Box::into_raw(Box::new(DirStream {
        fd,
        index: 0,
        entries,
        current: empty_dirent64(),
    }))
    .cast()
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn readdir64(dirp: *mut c_void) -> *mut c_void {
    if dirp.is_null() {
        return fail(EINVAL, ptr::null_mut());
    }
    let stream = unsafe { &mut *(dirp as *mut DirStream) };
    let Some(entry) = stream.entries.get(stream.index) else {
        return ptr::null_mut();
    };
    stream.current = dirent64(entry, stream.index);
    stream.index += 1;
    (&mut stream.current as *mut LinuxDirent64).cast()
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn readdir(dirp: *mut c_void) -> *mut c_void {
    readdir64(dirp)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn closedir(dirp: *mut c_void) -> c_int {
    if dirp.is_null() {
        return fail(EINVAL, -1);
    }
    let stream = unsafe { Box::from_raw(dirp as *mut DirStream) };
    close(stream.fd)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn dirfd(dirp: *mut c_void) -> c_int {
    if dirp.is_null() {
        return fail(EINVAL, -1);
    }
    unsafe { (*(dirp as *mut DirStream)).fd }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn rewinddir(dirp: *mut c_void) {
    if !dirp.is_null() {
        unsafe { (*(dirp as *mut DirStream)).index = 0 };
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn realpath(path: *const c_char, resolved: *mut c_char) -> *mut c_char {
    let Ok(path_str) = cstr(path) else {
        return fail(EINVAL, ptr::null_mut());
    };
    if let Err(e) = path_stat(path) {
        return fail(e, ptr::null_mut());
    }
    if resolved.is_null() {
        return fail(ENOSYS, ptr::null_mut());
    }
    let bytes = path_str.as_bytes();
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), resolved.cast::<u8>(), bytes.len());
        resolved.add(bytes.len()).write(0);
    }
    resolved
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn statx(
    _dirfd: c_int,
    _pathname: *const c_char,
    _flags: c_int,
    _mask: u32,
    _statxbuf: *mut c_void,
) -> c_int {
    fail(ENOSYS, -1)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn clock_gettime(clock_id: c_int, tp: *mut c_void) -> c_int {
    if tp.is_null() {
        return fail(EINVAL, -1);
    }
    let nanos = match clock_id {
        CLOCK_MONOTONIC => clock_fast(kernel_fidl::ClockType::Monotonic).unwrap_or_else(now_ns),
        CLOCK_BOOTTIME => clock_fast(kernel_fidl::ClockType::BootTime).unwrap_or_else(now_ns),
        CLOCK_REALTIME => clock_fast(kernel_fidl::ClockType::Realtime)
            .or_else(|| clock_fidl(kernel_fidl::ClockType::Realtime).ok())
            .unwrap_or_else(now_ns),
        _ => return fail(EINVAL, -1),
    };
    unsafe {
        (tp as *mut Timespec).write(Timespec {
            tv_sec: (nanos / 1_000_000_000) as i64,
            tv_nsec: (nanos % 1_000_000_000) as i64,
        });
    }
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn nanosleep(req: *const c_void, rem: *mut c_void) -> c_int {
    if req.is_null() {
        return fail(EINVAL, -1);
    }
    let req = unsafe { &*(req as *const Timespec) };
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 {
        return fail(EINVAL, -1);
    }
    let Some(delta) = (req.tv_sec as u64)
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(req.tv_nsec as u64))
    else {
        return fail(EINVAL, -1);
    };
    let deadline = now_ns().saturating_add(delta);
    let sleeper = AtomicU32::new(0);
    loop {
        let remaining = deadline.saturating_sub(now_ns());
        if remaining == 0 {
            break;
        }
        match pthread_sync::Kernel.wait_for(&sleeper, 0, remaining.min(i64::MAX as u64) as i64) {
            Ok(()) | Err(ETIMEDOUT) => {}
            Err(error) => return fail(error, -1),
        }
    }
    if !rem.is_null() {
        unsafe {
            (rem as *mut Timespec).write(Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            });
        }
    }
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn close(fd: c_int) -> c_int {
    match FD_TABLE.with(|t| t.close(fd)) {
        Ok(()) => 0,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize {
    if !buf.is_null() && count >= 8 {
        let event = FD_TABLE.with(|t| {
            let Some(entry) = t.entries.get_mut(fd as usize) else {
                return None;
            };
            let FdKind::Event { value } = &mut entry.kind else {
                return None;
            };
            if *value == 0 {
                return Some(Err(EAGAIN));
            }
            let out = *value;
            *value = 0;
            Some(Ok(out))
        });
        if let Some(result) = event {
            return match result {
                Ok(value) => {
                    unsafe { (buf as *mut u64).write(value) };
                    8
                }
                Err(e) => fail(e, -1),
            };
        }
    }
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    match fd_read(entry, buf, count) {
        Ok(n) => n as isize,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn write(fd: c_int, buf: *const c_void, count: usize) -> isize {
    if !buf.is_null() && count >= 8 {
        let event = FD_TABLE.with(|t| {
            let Some(entry) = t.entries.get_mut(fd as usize) else {
                return None;
            };
            let FdKind::Event { value } = &mut entry.kind else {
                return None;
            };
            let delta = unsafe { *(buf as *const u64) };
            if delta == u64::MAX {
                return Some(Err(EINVAL));
            }
            *value = value.saturating_add(delta);
            Some(Ok(()))
        });
        if let Some(result) = event {
            return match result {
                Ok(()) => 8,
                Err(e) => fail(e, -1),
            };
        }
    }
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    match fd_write(entry, buf, count) {
        Ok(n) => n as isize,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn fcntl(fd: c_int, cmd: c_int, arg: c_long) -> c_int {
    FD_TABLE.with(|t| {
        let Some(entry) = t.entries.get_mut(fd as usize) else {
            return fail(EBADF, -1);
        };
        if matches!(entry.kind, FdKind::Empty) {
            return fail(EBADF, -1);
        }
        match cmd {
            F_DUPFD | F_DUPFD_CLOEXEC => {
                if arg < 0 || entry.refs == usize::MAX {
                    return fail(EINVAL, -1);
                }
                entry.refs += 1;
                fd
            }
            F_GETFD => 0,
            F_SETFD => {
                let _ = arg & FD_CLOEXEC as c_long;
                0
            }
            F_GETFL => entry.flags,
            F_SETFL => {
                entry.flags = (entry.flags & !O_NONBLOCK) | ((arg as c_int) & O_NONBLOCK);
                0
            }
            _ => fail(EINVAL, -1),
        }
    })
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn socket(domain: c_int, kind: c_int, _protocol: c_int) -> c_int {
    if domain != AF_INET && domain != AF_INET6 {
        return fail(EAFNOSUPPORT, -1);
    }
    match kind & 0xf {
        SOCK_STREAM => FD_TABLE
            .with(|t| t.alloc(FdKind::PendingTcp))
            .unwrap_or_else(|e| fail(e, -1)),
        SOCK_DGRAM => {
            let (client_end, server_end) = match channel_pair() {
                Ok(pair) => pair,
                Err(e) => return fail(e, -1),
            };
            let mut client = match netstack_client() {
                Ok(client) => client,
                Err(e) => return fail(e, -1),
            };
            let request = net_fidl::NetstackCreateUdpSocketRequest {
                options: default_options(),
                socket: net_fidl::HandleRef { raw: server_end },
            };
            let mut rb = [0; 128];
            let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
            let mut ob = [0; 128];
            let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
            let response =
                match client.create_udp_socket(&request, &mut rb, &mut rh, &mut ob, &mut oh) {
                    Ok(response) => response,
                    Err(_) => return fail(EINVAL, -1),
                };
            if response.status != net_fidl::Status::Ok {
                return fail(errno_from_net(response.status), -1);
            }
            FD_TABLE
                .with(|t| {
                    t.alloc(FdKind::Udp {
                        control: client_end,
                    })
                })
                .unwrap_or_else(|e| fail(e, -1))
        }
        _ => fail(EOPNOTSUPP, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn connect(fd: c_int, addr: *const c_void, len: u32) -> c_int {
    let remote = match parse_sockaddr(addr, len) {
        Ok(addr) => addr,
        Err(e) => return fail(e, -1),
    };
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    if !matches!(entry.kind, FdKind::PendingTcp { .. }) {
        return fail(EINVAL, -1);
    }
    let (client_end, server_end) = match channel_pair() {
        Ok(pair) => pair,
        Err(e) => return fail(e, -1),
    };
    let mut client = match netstack_client() {
        Ok(client) => client,
        Err(e) => return fail(e, -1),
    };
    let request = net_fidl::NetstackConnectTcpRequest {
        remote_addr: remote,
        options: default_options(),
        socket: net_fidl::HandleRef { raw: server_end },
    };
    let mut rb = [0; 256];
    let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
    let mut ob = [0; 128];
    let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
    let response = match client.connect_tcp(&request, &mut rb, &mut rh, &mut ob, &mut oh) {
        Ok(response) => response,
        Err(_) => return fail(EINVAL, -1),
    };
    if response.status != net_fidl::Status::Ok {
        return fail(errno_from_net(response.status), -1);
    }
    let socket = match tcp_get_stream(client_end) {
        Ok(socket) => socket,
        Err(e) => return fail(e, -1),
    };
    match FD_TABLE.with(|t| {
        t.set_kind(
            fd,
            FdKind::TcpStream {
                socket,
                control: client_end,
            },
        )
    }) {
        Ok(()) => 0,
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn bind(fd: c_int, addr: *const c_void, len: u32) -> c_int {
    let local = match parse_sockaddr(addr, len) {
        Ok(addr) => addr,
        Err(e) => return fail(e, -1),
    };
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    match entry.kind {
        FdKind::Udp { control } => {
            let mut client = net_fidl::UdpSocketPublicClient::new(bexos_userspace::Rpc(
                bexos_userspace::Channel(control),
            ));
            let mut rb = [0; 128];
            let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
            let mut ob = [0; 128];
            let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
            match client.bind(
                &net_fidl::UdpSocketBindRequest { local_addr: local },
                &mut rb,
                &mut rh,
                &mut ob,
                &mut oh,
            ) {
                Ok(r) if r.status == net_fidl::Status::Ok => 0,
                Ok(r) => fail(errno_from_net(r.status), -1),
                Err(_) => fail(EINVAL, -1),
            }
        }
        FdKind::PendingTcp => {
            let (client_end, server_end) = match channel_pair() {
                Ok(pair) => pair,
                Err(e) => return fail(e, -1),
            };
            let mut client = match netstack_client() {
                Ok(client) => client,
                Err(e) => return fail(e, -1),
            };
            let request = net_fidl::NetstackListenTcpRequest {
                local_addr: local,
                options: default_options(),
                listener: net_fidl::HandleRef { raw: server_end },
            };
            let mut rb = [0; 256];
            let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
            let mut ob = [0; 128];
            let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
            let response = match client.listen_tcp(&request, &mut rb, &mut rh, &mut ob, &mut oh) {
                Ok(response) => response,
                Err(_) => return fail(EINVAL, -1),
            };
            if response.status != net_fidl::Status::Ok {
                return fail(errno_from_net(response.status), -1);
            }
            FD_TABLE
                .with(|t| {
                    t.set_kind(
                        fd,
                        FdKind::TcpListener {
                            control: client_end,
                        },
                    )
                })
                .map_or_else(|e| fail(e, -1), |_| 0)
        }
        _ => fail(EINVAL, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn listen(fd: c_int, _backlog: c_int) -> c_int {
    match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) if matches!(entry.kind, FdKind::TcpListener { .. }) => 0,
        Ok(_) => fail(EINVAL, -1),
        Err(e) => fail(e, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn accept(fd: c_int, addr: *mut c_void, len: *mut u32) -> c_int {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::TcpListener { control } = entry.kind else {
        return fail(EINVAL, -1);
    };
    let mut client = net_fidl::TcpListenerPublicClient::new(bexos_userspace::Rpc(
        bexos_userspace::Channel(control),
    ));
    let mut rb = [0; 64];
    let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
    let mut ob = [0; 128];
    let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
    let response = match client.accept(
        &net_fidl::TcpListenerAcceptRequest {},
        &mut rb,
        &mut rh,
        &mut ob,
        &mut oh,
    ) {
        Ok(response) => response,
        Err(_) => return fail(EINVAL, -1),
    };
    if response.status != net_fidl::Status::Ok {
        return fail(errno_from_net(response.status), -1);
    }
    if let Err(e) = write_sockaddr(response.peer_addr, addr, len) {
        return fail(e, -1);
    }
    let socket = match tcp_get_stream(response.client.raw) {
        Ok(socket) => socket,
        Err(e) => return fail(e, -1),
    };
    FD_TABLE
        .with(|t| {
            t.alloc(FdKind::TcpStream {
                socket,
                control: response.client.raw,
            })
        })
        .unwrap_or_else(|e| fail(e, -1))
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn recv(fd: c_int, buf: *mut c_void, count: usize, _flags: c_int) -> isize {
    read(fd, buf, count)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn send(fd: c_int, buf: *const c_void, count: usize, _flags: c_int) -> isize {
    write(fd, buf, count)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn shutdown(fd: c_int, how: c_int) -> c_int {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::TcpStream { socket, .. } = entry.kind else {
        return fail(EINVAL, -1);
    };
    let read_side = how == 0 || how == 2;
    let write_side = how == 1 || how == 2;
    match bexos_userspace::Socket(socket).shutdown(read_side, write_side) {
        Ok(()) => 0,
        Err(e) => fail(errno_from_kernel(e), -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn setsockopt(
    fd: c_int,
    level: c_int,
    optname: c_int,
    _optval: *const c_void,
    _optlen: u32,
) -> c_int {
    if FD_TABLE.with(|t| t.get(fd)).is_err() {
        return fail(EBADF, -1);
    }
    match (level, optname) {
        (SOL_SOCKET, SO_REUSEADDR | SO_REUSEPORT | SO_KEEPALIVE | SO_BROADCAST)
        | (IPPROTO_TCP, TCP_NODELAY) => 0,
        _ => fail(ENOPROTOOPT, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn getsockopt(
    fd: c_int,
    level: c_int,
    optname: c_int,
    optval: *mut c_void,
    optlen: *mut u32,
) -> c_int {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    if optval.is_null() || optlen.is_null() {
        return fail(EINVAL, -1);
    }
    let value = match (level, optname) {
        (SOL_SOCKET, SO_ERROR) => 0,
        (SOL_SOCKET, SO_TYPE) => match entry.kind {
            FdKind::TcpStream { .. } | FdKind::TcpListener { .. } | FdKind::PendingTcp => {
                SOCK_STREAM
            }
            FdKind::Udp { .. } => SOCK_DGRAM,
            _ => return fail(ENOPROTOOPT, -1),
        },
        (SOL_SOCKET, SO_REUSEADDR | SO_REUSEPORT | SO_KEEPALIVE | SO_BROADCAST)
        | (IPPROTO_TCP, TCP_NODELAY) => 0,
        _ => return fail(ENOPROTOOPT, -1),
    };
    unsafe {
        if *optlen < core::mem::size_of::<c_int>() as u32 {
            return fail(EINVAL, -1);
        }
        (optval as *mut c_int).write(value);
        *optlen = core::mem::size_of::<c_int>() as u32;
    }
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn getsockname(fd: c_int, addr: *mut c_void, len: *mut u32) -> c_int {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    match entry.kind {
        FdKind::TcpStream { control, .. } => match tcp_addr(control, true) {
            Ok(value) => write_sockaddr(value, addr, len).map_or_else(|e| fail(e, -1), |_| 0),
            Err(e) => fail(e, -1),
        },
        _ => fail(EOPNOTSUPP, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn getpeername(fd: c_int, addr: *mut c_void, len: *mut u32) -> c_int {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    match entry.kind {
        FdKind::TcpStream { control, .. } => match tcp_addr(control, false) {
            Ok(value) => write_sockaddr(value, addr, len).map_or_else(|e| fail(e, -1), |_| 0),
            Err(e) => fail(e, -1),
        },
        _ => fail(EOPNOTSUPP, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn sendto(
    fd: c_int,
    buf: *const c_void,
    count: usize,
    _flags: c_int,
    addr: *const c_void,
    len: u32,
) -> isize {
    let destination = match parse_sockaddr(addr, len) {
        Ok(addr) => addr,
        Err(e) => return fail(e, -1),
    };
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::Udp { control } = entry.kind else {
        return fail(EINVAL, -1);
    };
    if buf.is_null() {
        return fail(EINVAL, -1);
    }
    let data = unsafe { core::slice::from_raw_parts(buf.cast::<u8>(), count) };
    let mut client = net_fidl::UdpSocketPublicClient::new(bexos_userspace::Rpc(
        bexos_userspace::Channel(control),
    ));
    let mut rb = [0; 8192];
    let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
    let mut ob = [0; 128];
    let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
    match client.send_to(
        &net_fidl::UdpSocketSendToRequest { data, destination },
        &mut rb,
        &mut rh,
        &mut ob,
        &mut oh,
    ) {
        Ok(r) if r.status == net_fidl::Status::Ok => r.actual as isize,
        Ok(r) => fail(errno_from_net(r.status), -1),
        Err(_) => fail(EINVAL, -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn recvfrom(
    fd: c_int,
    buf: *mut c_void,
    count: usize,
    _flags: c_int,
    addr: *mut c_void,
    len: *mut u32,
) -> isize {
    let entry = match FD_TABLE.with(|t| t.get(fd)) {
        Ok(entry) => entry,
        Err(e) => return fail(e, -1),
    };
    let FdKind::Udp { control } = entry.kind else {
        return fail(EINVAL, -1);
    };
    if buf.is_null() {
        return fail(EINVAL, -1);
    }
    let mut client = net_fidl::UdpSocketPublicClient::new(bexos_userspace::Rpc(
        bexos_userspace::Channel(control),
    ));
    let mut rb = [0; 64];
    let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
    let mut ob = [0; 8192];
    let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
    match client.recv_from(
        &net_fidl::UdpSocketRecvFromRequest {},
        &mut rb,
        &mut rh,
        &mut ob,
        &mut oh,
    ) {
        Ok(r) if r.status == net_fidl::Status::Ok => {
            let n = r.data.len().min(count);
            unsafe { ptr::copy_nonoverlapping(r.data.as_ptr(), buf.cast(), n) };
            if let Err(e) = write_sockaddr(r.source, addr, len) {
                return fail(e, -1);
            }
            n as isize
        }
        Ok(r) => fail(errno_from_net(r.status), -1),
        Err(_) => fail(EINVAL, -1),
    }
}

#[repr(C)]
pub struct AddrInfo {
    ai_flags: c_int,
    ai_family: c_int,
    ai_socktype: c_int,
    ai_protocol: c_int,
    ai_addrlen: u32,
    ai_addr: *mut c_void,
    ai_canonname: *mut c_char,
    ai_next: *mut AddrInfo,
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn getaddrinfo(
    node: *const c_char,
    service: *const c_char,
    _hints: *const AddrInfo,
    res: *mut *mut AddrInfo,
) -> c_int {
    if res.is_null() {
        return EINVAL;
    }
    unsafe { res.write(ptr::null_mut()) };
    let port = if service.is_null() {
        0
    } else {
        let bytes = unsafe { core::slice::from_raw_parts(service.cast::<u8>(), cstr_len(service)) };
        parse_port(bytes).unwrap_or(0)
    };
    let mut addresses = [net_fidl::IpAddress::Ipv4(net_fidl::Ipv4Address {
        octets: [127, 0, 0, 1],
    }); 8];
    let mut count = 0usize;
    if node.is_null() {
        count = 1;
    } else {
        let name = unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                node.cast::<u8>(),
                cstr_len(node),
            ))
        };
        if name == "localhost" {
            count = 1;
        } else if let Ok(mut client) = netstack_client() {
            let mut rb = [0; 384];
            let mut rh = [net_fidl::HandleRef { raw: 0 }; 1];
            let mut ob = [0; 512];
            let mut oh = [net_fidl::HandleRef { raw: 0 }; 1];
            if let Ok(response) = client.resolve_host(
                &net_fidl::NetstackResolveHostRequest { hostname: name },
                &mut rb,
                &mut rh,
                &mut ob,
                &mut oh,
            ) {
                if response.status != net_fidl::Status::Ok {
                    return errno_from_net(response.status);
                }
                for index in 0..response.addresses.len().min(addresses.len()) {
                    let Ok(address) = response.addresses.get(index) else {
                        return ENETUNREACH;
                    };
                    addresses[count] = address;
                    count += 1;
                }
            }
        }
    }
    if count == 0 {
        return ENETUNREACH;
    }
    let mut head: *mut AddrInfo = ptr::null_mut();
    for index in (0..count).rev() {
        let socket_address = net_fidl::SocketAddress {
            addr: addresses[index],
            port,
        };
        let addr_len = match socket_address.addr {
            net_fidl::IpAddress::Ipv4(_) => core::mem::size_of::<SockAddrIn>(),
            net_fidl::IpAddress::Ipv6(_) => core::mem::size_of::<SockAddrIn6>(),
        };
        let ai = unsafe { malloc(core::mem::size_of::<AddrInfo>()) as *mut AddrInfo };
        let sa = unsafe { malloc(addr_len) };
        if ai.is_null() || sa.is_null() {
            return ENOMEM;
        }
        let mut len = addr_len as u32;
        if write_sockaddr(socket_address, sa, &mut len).is_err() {
            return EINVAL;
        }
        unsafe {
            ai.write(AddrInfo {
                ai_flags: 0,
                ai_family: match socket_address.addr {
                    net_fidl::IpAddress::Ipv4(_) => AF_INET,
                    net_fidl::IpAddress::Ipv6(_) => AF_INET6,
                },
                ai_socktype: 0,
                ai_protocol: 0,
                ai_addrlen: len,
                ai_addr: sa,
                ai_canonname: ptr::null_mut(),
                ai_next: head,
            });
        }
        head = ai;
    }
    unsafe { res.write(head) };
    0
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn freeaddrinfo(mut ai: *mut AddrInfo) {
    unsafe {
        while !ai.is_null() {
            let next = (*ai).ai_next;
            free((*ai).ai_addr);
            free(ai.cast());
            ai = next;
        }
    }
}

fn parse_port(bytes: &[u8]) -> Option<u16> {
    let mut value = 0u16;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u16)?;
    }
    Some(value)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn poll(fds: *mut c_void, nfds: usize, timeout: c_int) -> c_int {
    if fds.is_null() && nfds != 0 {
        return fail(EINVAL, -1);
    }
    let deadline = (timeout >= 0).then(|| now_ns().saturating_add(timeout as u64 * 1_000_000));
    loop {
        let mut count = 0;
        for index in 0..nfds {
            let pollfd = unsafe { &mut *((fds as *mut PollFd).add(index)) };
            pollfd.revents = 0;
            if pollfd.fd < 0 {
                continue;
            }
            match FD_TABLE.with(|t| t.get(pollfd.fd)) {
                Ok(entry) => {
                    pollfd.revents = readiness(entry, pollfd.events);
                    if pollfd.revents != 0 {
                        count += 1;
                    }
                }
                Err(_) => {
                    pollfd.revents = POLLERR;
                    count += 1;
                }
            }
        }
        if count != 0 || timeout == 0 || deadline.is_some_and(|d| now_ns() >= d) {
            return count;
        }
        bexos_userspace::yield_now();
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn ppoll(
    fds: *mut c_void,
    nfds: usize,
    timeout: *const c_void,
    _sigmask: *const c_void,
) -> c_int {
    let timeout_ms = if timeout.is_null() {
        -1
    } else {
        let ts = unsafe { &*(timeout as *const Timespec) };
        (ts.tv_sec.saturating_mul(1000) + ts.tv_nsec / 1_000_000) as c_int
    };
    poll(fds, nfds, timeout_ms)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn eventfd(initval: c_int, flags: c_int) -> c_int {
    let fd = FD_TABLE
        .with(|t| {
            t.alloc(FdKind::Event {
                value: initval as u64,
            })
        })
        .unwrap_or_else(|e| fail(e, -1));
    if fd >= 0 && flags & O_NONBLOCK != 0 {
        let _ = fcntl(fd, F_SETFL, O_NONBLOCK as c_long);
    }
    fd
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn epoll_create1(_flags: c_int) -> c_int {
    FD_TABLE
        .with(|t| {
            t.alloc(FdKind::Epoll {
                entries: [EpollEntry {
                    fd: -1,
                    events: 0,
                    data: 0,
                }; 32],
                len: 0,
            })
        })
        .unwrap_or_else(|e| fail(e, -1))
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn epoll_ctl(epfd: c_int, op: c_int, fd: c_int, event: *const c_void) -> c_int {
    epoll_ctl_inner(epfd, op, fd, event)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn epoll_wait(
    epfd: c_int,
    events: *mut c_void,
    maxevents: c_int,
    timeout: c_int,
) -> c_int {
    epoll_wait_inner(epfd, events, maxevents, timeout)
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_self() -> usize {
    #[cfg(not(bexos_guest))]
    {
        return 1;
    }
    #[cfg(bexos_guest)]
    {
        let local = ensure_initial_thread_local();
        if local.is_null() {
            return 1;
        }
        unsafe { (*local).pthread_id }
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_setname_np(_thread: usize, _name: *const c_char) -> c_int {
    ENOSYS
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_create(
    thread: *mut usize,
    attr: *const c_void,
    start: extern "C" fn(*mut c_void) -> *mut c_void,
    arg: *mut c_void,
) -> c_int {
    if thread.is_null() {
        return EINVAL;
    }
    reap_detached_threads();
    let id = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed) as usize;
    let attributes = match pthread_attr::read_or_default(attr) {
        Ok(attributes) => attributes,
        Err(error) => return error,
    };
    let stack_size = attributes.stack_size;
    let stack = unsafe { alloc_with_align(stack_size, 16) };
    if stack.is_null() {
        return ENOMEM;
    }
    let thread_pointer = unsafe { alloc_thread_state(id) };
    if thread_pointer.is_null() {
        unsafe { free(stack) };
        return ENOMEM;
    }
    let stack_top = unsafe { stack.cast::<u8>().add(stack_size) } as usize & !15;
    if let Err(e) = THREADS.with(|threads| {
        threads.insert(ThreadStart {
            id,
            start,
            arg,
            result: ptr::null_mut(),
            done: false,
            started: false,
            kernel_handle: 0,
            detached: attributes.detached != 0,
            joining: false,
            stack,
            stack_size,
            thread_pointer,
        })
    }) {
        unsafe { free(stack) };
        unsafe { free_thread_state(thread_pointer) };
        return e;
    }
    let r: Result<kernel_fidl::TaskControlCreateThreadResponse, kernel_fidl::Status> =
        bexos_userspace::ipc::kernel_call(
            3,
            "CreateThread",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlCreateThreadRequest {
                entry_vaddr: pthread_trampoline as *const () as usize as u64,
                stack_top_vaddr: stack_top as u64,
                arg_handle: kernel_fidl::HandleRef { raw: 0 },
            },
        );
    match r {
        Ok(response) if response.status == kernel_fidl::Status::Ok => {
            THREADS.with(|threads| {
                if let Some(entry) = threads
                    .entries
                    .iter_mut()
                    .flatten()
                    .find(|entry| entry.id == id)
                {
                    entry.kernel_handle = response.thread_handle.raw;
                }
            });
            unsafe { thread.write(id) };
            0
        }
        Ok(response) => {
            let _ = THREADS.with(|threads| threads.take_pending(id));
            unsafe { free(stack) };
            unsafe { free_thread_state(thread_pointer) };
            errno_from_kernel(response.status)
        }
        Err(status) => {
            let _ = THREADS.with(|threads| threads.take_pending(id));
            unsafe { free(stack) };
            unsafe { free_thread_state(thread_pointer) };
            errno_from_kernel(status)
        }
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_join(thread: usize, value: *mut *mut c_void) -> c_int {
    if thread == pthread_self() {
        return 35; // EDEADLK
    }
    let handle = match THREADS.with(|threads| threads.claim_join(thread)) {
        Ok(handle) => handle,
        Err(error) => return error,
    };
    if let Err(error) = wait_for_thread_termination(handle) {
        THREADS.with(|threads| threads.cancel_join(thread));
        return error;
    }
    let Some(entry) = THREADS.with(|threads| threads.join(thread)) else {
        THREADS.with(|threads| threads.cancel_join(thread));
        return EIO;
    };
    if !value.is_null() {
        unsafe { value.write(entry.result) };
    }
    release_thread(entry);
    0
}

extern "C" fn pthread_trampoline(_raw: *mut c_void) -> *mut c_void {
    // Creation order is not execution order. Identify the record using the
    // actual kernel-selected stack before installing TLS or running client code.
    let marker = 0u8;
    let stack_address = core::ptr::addr_of!(marker) as usize;
    let Some(entry) = THREADS.with(|threads| threads.take_pending_on_stack(stack_address)) else {
        bexos_userspace::exit();
    };
    set_thread_pointer(entry.thread_pointer);
    let result = (entry.start)(entry.arg);
    pthread_keys::finish_thread();
    THREADS.with(|threads| threads.finish(entry.id, result));
    let _ = bexos_userspace::ipc::kernel_call::<
        kernel_fidl::TaskControlExitThreadRequest,
        kernel_fidl::TaskControlExitThreadResponse,
    >(
        3,
        "ExitThread",
        kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
        &kernel_fidl::TaskControlExitThreadRequest { exit_code: 0 },
    );
    bexos_userspace::exit();
}

fn wait_thread(handle: u64, deadline_nanos: i64) -> Result<(), c_int> {
    if handle == 0 {
        return Err(EINVAL);
    }
    let items = [kernel_fidl::InlineVectorStruct1 {
        h: kernel_fidl::HandleRef { raw: handle },
        signals: kernel_fidl::Signals::TERMINATED,
    }];
    let result = bexos_userspace::ipc::kernel_call::<_, kernel_fidl::TaskControlWaitManyResponse>(
        3,
        "WaitMany",
        kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
        &kernel_fidl::TaskControlWaitManyRequest {
            items: kernel_fidl::WireVector::from_slice(&items),
            deadline_nanos,
        },
    );
    match result {
        Ok(reply) if reply.status == kernel_fidl::Status::Ok => Ok(()),
        Ok(reply) => Err(errno_from_kernel(reply.status)),
        Err(status) => Err(errno_from_kernel(status)),
    }
}

fn thread_terminated(handle: u64) -> bool {
    wait_thread(handle, 0).is_ok()
}

fn wait_for_thread_termination(handle: u64) -> Result<(), c_int> {
    loop {
        match wait_thread(handle, i64::MAX) {
            // WaitMany enqueues its response before parking. Waking asks the
            // caller to recheck the signal, including its ErrTimedOut response.
            Err(EINTR | ETIMEDOUT) => continue,
            result => return result,
        }
    }
}

fn release_thread(entry: ThreadStart) {
    let _ = bexos_userspace::ipc::kernel_call::<_, kernel_fidl::ObjectControlCloseResponse>(
        5,
        "Close",
        kernel_fidl::OBJECT_CONTROL_PUBLIC_METHODS,
        &kernel_fidl::ObjectControlCloseRequest {
            object: kernel_fidl::HandleRef {
                raw: entry.kernel_handle,
            },
        },
    );
    unsafe {
        free(entry.stack);
        free_thread_state(entry.thread_pointer);
    }
}

fn reap_detached_threads() {
    let candidates = THREADS.with(|threads| threads.entries);
    for entry in candidates.into_iter().flatten() {
        if entry.detached && entry.done && thread_terminated(entry.kernel_handle) {
            if let Some(entry) = THREADS.with(|threads| threads.join(entry.id)) {
                release_thread(entry);
            }
        }
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_detach(thread: usize) -> c_int {
    let result = THREADS.with(|threads| {
        if let Some(entry) = threads
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.id == thread)
        {
            if entry.detached || entry.joining {
                return EINVAL;
            }
            entry.detached = true;
            return 0;
        }
        EINVAL
    });
    reap_detached_threads();
    result
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn pthread_exit(value: *mut c_void) -> ! {
    pthread_keys::finish_thread();
    THREADS.with(|threads| threads.finish(pthread_self(), value));
    let _ = bexos_userspace::ipc::kernel_call::<
        kernel_fidl::TaskControlExitThreadRequest,
        kernel_fidl::TaskControlExitThreadResponse,
    >(
        3,
        "ExitThread",
        kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
        &kernel_fidl::TaskControlExitThreadRequest { exit_code: 0 },
    );
    bexos_userspace::exit();
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn sched_yield() -> c_int {
    let r: Result<kernel_fidl::TaskControlYieldThreadResponse, kernel_fidl::Status> =
        bexos_userspace::ipc::kernel_call(
            3,
            "YieldThread",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlYieldThreadRequest {
                target_thread: kernel_fidl::HandleRef { raw: 0 },
            },
        );
    match r {
        Ok(response) if response.status == kernel_fidl::Status::Ok => 0,
        Ok(response) => fail(errno_from_kernel(response.status), -1),
        Err(status) => fail(errno_from_kernel(status), -1),
    }
}

fn futex_wait(uaddr: c_long, expected: c_long, timeout_nanos: i64) -> c_long {
    let start = now_ns();
    let r: Result<kernel_fidl::TaskControlFutexWaitResponse, kernel_fidl::Status> =
        bexos_userspace::ipc::kernel_call_buffered(
            3,
            "FutexWait",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlFutexWaitRequest {
                uaddr: uaddr as u64,
                expected_val: expected as u32,
                timeout_nanos,
                owner_thread: kernel_fidl::HandleRef { raw: 0 },
            },
            &mut [0; 64],
            &mut [0; 32],
        );
    match r {
        Ok(response) if response.status == kernel_fidl::Status::Ok => {
            if timeout_nanos >= 0 && now_ns().saturating_sub(start) >= timeout_nanos as u64 {
                fail(ETIMEDOUT, -1)
            } else {
                0
            }
        }
        Ok(response) if response.status == kernel_fidl::Status::ErrResourceExhausted => {
            fail(EAGAIN, -1)
        }
        Ok(response) => fail(errno_from_kernel(response.status), -1),
        Err(status) => fail(errno_from_kernel(status), -1),
    }
}

fn futex_wake(uaddr: c_long, count: c_long) -> c_long {
    let r: Result<kernel_fidl::TaskControlFutexWakeResponse, kernel_fidl::Status> =
        bexos_userspace::ipc::kernel_call_buffered(
            3,
            "FutexWake",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlFutexWakeRequest {
                uaddr: uaddr as u64,
                wake_count: count as u32,
            },
            &mut [0; 64],
            &mut [0; 32],
        );
    match r {
        Ok(response) if response.status == kernel_fidl::Status::Ok => {
            response.woken_count as c_long
        }
        Ok(response) => fail(errno_from_kernel(response.status), -1),
        Err(status) => fail(errno_from_kernel(status), -1),
    }
}

#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn syscall(
    number: c_long,
    a0: c_long,
    a1: c_long,
    a2: c_long,
    a3: c_long,
    _a4: c_long,
    _a5: c_long,
) -> c_long {
    match number {
        SYS_GETTID => pthread_self() as c_long,
        SYS_FUTEX => match (a1 as c_int) & 0x7f {
            FUTEX_WAIT => {
                let duration = if a3 == 0 {
                    -1
                } else {
                    if a3 as usize % core::mem::align_of::<Timespec>() != 0 {
                        return fail(EINVAL, -1);
                    }
                    let time = unsafe { (a3 as *const Timespec).read() };
                    if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
                        return fail(EINVAL, -1);
                    }
                    match time
                        .tv_sec
                        .checked_mul(1_000_000_000)
                        .and_then(|n| n.checked_add(time.tv_nsec))
                    {
                        Some(n) => n,
                        None => return fail(EINVAL, -1),
                    }
                };
                futex_wait(a0, a2, duration)
            }
            FUTEX_WAKE => futex_wake(a0, a2),
            _ => fail(ENOSYS, -1),
        },
        SYS_EPOLL_CREATE1 => FD_TABLE
            .with(|t| {
                t.alloc(FdKind::Epoll {
                    entries: [EpollEntry {
                        fd: -1,
                        events: 0,
                        data: 0,
                    }; 32],
                    len: 0,
                })
            })
            .unwrap_or_else(|e| fail(e, -1)) as c_long,
        SYS_EPOLL_CTL => {
            epoll_ctl_inner(a0 as c_int, a1 as c_int, a2 as c_int, a3 as *const c_void) as c_long
        }
        SYS_EPOLL_PWAIT => {
            epoll_wait_inner(a0 as c_int, a1 as *mut c_void, a2 as c_int, a3 as c_int) as c_long
        }
        SYS_EVENTFD2 => FD_TABLE
            .with(|t| t.alloc(FdKind::Event { value: a0 as u64 }))
            .unwrap_or_else(|e| fail(e, -1)) as c_long,
        SYS_TIMERFD_CREATE => FD_TABLE
            .with(|t| {
                let _ = a0;
                t.alloc(FdKind::Timer {
                    deadline_ns: u64::MAX,
                })
            })
            .unwrap_or_else(|e| fail(e, -1)) as c_long,
        SYS_TIMERFD_SETTIME => {
            let fd = a0 as c_int;
            let spec = a3 as *const Timespec;
            let delay_ns = if spec.is_null() {
                1_000_000
            } else {
                unsafe {
                    let interval = *spec;
                    let value = *spec.add(1);
                    let _ = interval;
                    if value.tv_sec < 0 || value.tv_nsec < 0 || value.tv_nsec >= 1_000_000_000 {
                        return fail(EINVAL, -1);
                    }
                    value.tv_sec as u64 * 1_000_000_000 + value.tv_nsec as u64
                }
            };
            FD_TABLE
                .with(|t| {
                    t.set_kind(
                        fd,
                        FdKind::Timer {
                            deadline_ns: now_ns().saturating_add(delay_ns),
                        },
                    )
                })
                .map_or_else(|e| fail(e, -1), |_| 0) as c_long
        }
        SYS_GETRANDOM => {
            let buf = a0 as *mut u8;
            let len = a1 as usize;
            let flags = a2 as c_int;
            if buf.is_null() && len != 0 {
                return fail(EINVAL, -1);
            }
            let _ = flags & GRND_NONBLOCK;
            let mut state = now_ns() ^ 0x9e37_79b9_7f4a_7c15;
            for index in 0..len {
                state ^= state << 7;
                state ^= state >> 9;
                state = state.wrapping_mul(0xbf58_476d_1ce4_e5b9);
                unsafe { buf.add(index).write((state >> 32) as u8) };
            }
            len as c_long
        }
        _ => fail(ENOSYS, -1),
    }
}

fn epoll_ctl_inner(epfd: c_int, op: c_int, fd: c_int, event: *const c_void) -> c_int {
    FD_TABLE.with(|t| {
        let Some(ep) = t.entries.get_mut(epfd as usize) else {
            return fail(EBADF, -1);
        };
        let FdKind::Epoll { entries, len } = &mut ep.kind else {
            return fail(EINVAL, -1);
        };
        match op {
            EPOLL_CTL_ADD => {
                if *len >= entries.len() {
                    return fail(ENOMEM, -1);
                }
                let e = unsafe { &*(event as *const EpollEvent) };
                entries[*len] = EpollEntry {
                    fd,
                    events: e.events,
                    data: e.data,
                };
                *len += 1;
                0
            }
            EPOLL_CTL_MOD => {
                let e = unsafe { &*(event as *const EpollEvent) };
                for item in entries.iter_mut().take(*len) {
                    if item.fd == fd {
                        item.events = e.events;
                        item.data = e.data;
                        return 0;
                    }
                }
                fail(EBADF, -1)
            }
            EPOLL_CTL_DEL => {
                for index in 0..*len {
                    if entries[index].fd == fd {
                        entries[index] = entries[*len - 1];
                        *len -= 1;
                        return 0;
                    }
                }
                fail(EBADF, -1)
            }
            _ => fail(EINVAL, -1),
        }
    })
}

fn epoll_wait_inner(epfd: c_int, events: *mut c_void, maxevents: c_int, timeout: c_int) -> c_int {
    if events.is_null() || maxevents <= 0 {
        return fail(EINVAL, -1);
    }
    let deadline = (timeout >= 0).then(|| now_ns().saturating_add(timeout as u64 * 1_000_000));
    loop {
        let snapshot = match FD_TABLE.with(|t| t.get(epfd)) {
            Ok(entry) => entry,
            Err(e) => return fail(e, -1),
        };
        let FdKind::Epoll { entries, len } = snapshot.kind else {
            return fail(EINVAL, -1);
        };
        let mut out = 0;
        for item in entries.iter().take(len) {
            let wanted = ((item.events & EPOLLIN != 0) as c_short * POLLIN)
                | ((item.events & EPOLLOUT != 0) as c_short * POLLOUT);
            let ready = match FD_TABLE.with(|t| t.get(item.fd)) {
                Ok(entry) => readiness(entry, wanted),
                Err(_) => POLLERR,
            };
            if ready != 0 {
                unsafe {
                    (events as *mut EpollEvent)
                        .add(out as usize)
                        .write(EpollEvent {
                            events: item.events,
                            data: item.data,
                        });
                }
                out += 1;
                if out == maxevents {
                    break;
                }
            }
        }
        if out != 0 || timeout == 0 || deadline.is_some_and(|d| now_ns() >= d) {
            return out;
        }
        bexos_userspace::yield_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_completion_retains_kernel_handle_and_never_frees_active_stack() {
        extern "C" fn start(value: *mut c_void) -> *mut c_void {
            value
        }
        let mut table = ThreadTable::new();
        table
            .insert(ThreadStart {
                id: 7,
                start,
                arg: ptr::null_mut(),
                result: ptr::null_mut(),
                done: false,
                started: false,
                kernel_handle: 0,
                detached: true,
                joining: false,
                // Nonallocated sentinel pointers must never be freed by finish.
                stack: 0x1000 as *mut c_void,
                stack_size: 0x1000,
                thread_pointer: 0x2000 as *mut c_void,
            })
            .unwrap();
        let mut second = table.entries[0].unwrap();
        second.id = 8;
        second.stack = 0x4000 as *mut c_void;
        second.started = false;
        table.insert(second).unwrap();
        assert!(table.take_pending_on_stack(0x3000).is_none());
        assert_eq!(table.take_pending_on_stack(0x4800).unwrap().id, 8);
        let entry = table.take_pending_on_stack(0x1800).unwrap();
        assert_eq!(entry.id, 7);
        assert!(table.take_pending_on_stack(0x1800).is_none());
        table.finish(7, 0x3000 as *mut c_void);
        assert!(
            table.join(7).is_none(),
            "CreateThread has not published its handle yet"
        );
        table.entries[0].as_mut().unwrap().kernel_handle = 42;
        assert_eq!(table.claim_join(99), Err(3));
        assert_eq!(
            table.claim_join(7),
            Err(EINVAL),
            "detached threads cannot be joined"
        );
        table.entries[0].as_mut().unwrap().detached = false;
        assert_eq!(table.claim_join(7), Ok(42));
        assert_eq!(
            table.claim_join(7),
            Err(EINVAL),
            "only one joiner may own the handle"
        );
        table.cancel_join(7);
        assert_eq!(
            table.claim_join(7),
            Ok(42),
            "failed waits retain the joinable record"
        );
        let completed = table.join(7).unwrap();
        assert_eq!(completed.kernel_handle, 42);
        assert_eq!(completed.result as usize, 0x3000);
        assert_eq!(completed.stack as usize, 0x1000);
        assert!(table.join(7).is_none());
    }

    #[test]
    fn namespace_resolves_longest_mount_and_data_relative_default() {
        let mut table = NamespaceTable::new();
        table.install_entries(&[
            bexos_userspace::NamespaceEntry {
                path: "/pkg".into(),
                directory: 11,
            },
            bexos_userspace::NamespaceEntry {
                path: "/data".into(),
                directory: 12,
            },
            bexos_userspace::NamespaceEntry {
                path: "/deps/libfoo".into(),
                directory: 13,
            },
        ]);

        let resolved = table.resolve("/deps/libfoo/bin/tool").unwrap();
        assert_eq!(resolved.root, 13);
        assert_eq!(resolved.relative, "bin/tool");

        let resolved = table.resolve("state/db.redb").unwrap();
        assert_eq!(resolved.root, 12);
        assert_eq!(resolved.relative, "state/db.redb");
    }

    #[test]
    fn namespace_rejects_parent_components() {
        let mut table = NamespaceTable::new();
        table.install_entries(&[bexos_userspace::NamespaceEntry {
            path: "/data".into(),
            directory: 12,
        }]);
        assert!(matches!(table.resolve("../pkg/bin/app"), Err(EINVAL)));
    }

    #[test]
    fn linux_open_flags_translate_to_bexos_flags() {
        assert_eq!(fs_flags(0), 1);
        assert_eq!(fs_flags(O_WRONLY | O_CREAT | O_TRUNC), 2 | 8 | 16);
        assert_eq!(fs_flags(O_RDWR | O_DIRECTORY), 1 | 2 | 32);
    }

    #[test]
    fn stat_marks_files_and_directories() {
        let attr = fs_fidl::FileAttributes {
            size_bytes: 7,
            storage_allocated_bytes: 4096,
            creation_time_nanos: 2_000_000_003,
            modification_time_nanos: 4_000_000_005,
            mode: 0o644,
        };
        let file = file_attr_to_stat(attr, fs_fidl::NodeKind::File);
        assert_eq!(file.st_mode & S_IFREG, S_IFREG);
        assert_eq!(file.st_size, 7);
        assert_eq!(file.st_blocks, 8);
        assert_eq!(file.st_mtime, 4);
        assert_eq!(file.st_mtime_nsec, 5);

        let dir = file_attr_to_stat(attr, fs_fidl::NodeKind::Directory);
        assert_eq!(dir.st_mode & S_IFDIR, S_IFDIR);
    }

    #[test]
    fn dirent_contains_name_and_type() {
        let entry = DirEntryCache {
            name: "hello.txt".to_string(),
            kind: fs_fidl::NodeKind::File,
        };
        let dirent = dirent64(&entry, 4);
        assert_eq!(dirent.d_ino, 5);
        assert_eq!(dirent.d_type, DT_REG);
        let name = unsafe {
            core::slice::from_raw_parts(dirent.d_name.as_ptr().cast::<u8>(), "hello.txt".len())
        };
        assert_eq!(name, b"hello.txt");
    }
}

unsafe fn free_thread_state(pointer: *mut c_void) {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    let pointer = if pointer.is_null() {
        pointer
    } else {
        unsafe { *((pointer as *const usize).add(1)) as *mut c_void }
    };
    unsafe {
        free(pointer);
    }
}
