# Rust `std` on BexOS

BexOS supports Rust applications and higher-level services in the Rust `std`
world by compiling them for the Linux-family `aarch64-unknown-linux-gnu` target
and linking a small BexOS libc shim. The shim exports the libc/POSIX symbols
Rust expects and translates the useful parts into BexOS capabilities, generated
FIDL protocols, and kernel object syscalls.

This is a compatibility layer, not Linux emulation. Code should treat it as the
stable path for ordinary application and daemon work, while using explicit
BexOS APIs for kernel-specific capability, migration, and driver behavior.

## `std` Support Table

| Rust API area | Status | BexOS backing |
| --- | --- | --- |
| `std::alloc` | Implemented | `malloc`, `calloc`, `realloc`, `free`, and `posix_memalign` grow from the process heap VMAR using anonymous VMO mappings. |
| `std::thread` | Partial | `pthread_create`, `pthread_join`, detach/exit, pthread identity, pthread-specific values, and `sched_yield` map to BexOS thread/runtime state. TLS destructors and full pthread attributes are not implemented. |
| `std::sync` | Partial | In-process atomics work normally; futex waits/wakes route through `TaskControl.FutexWait` and `TaskControl.FutexWake`. |
| `std::time` | Partial | `clock_gettime` uses `Clock.GetTime` with a counter fallback; `nanosleep` yields until the requested deadline. |
| `std::fs::File` | Implemented for mounted app namespaces | `open`, `read`, `write`, `seek`, `metadata`, `set_len`, `sync_all`, and close route to BexOS fs FIDL. |
| `std::fs::read_dir` | Implemented for mounted app namespaces | libc adapts BexOS `Directory.ReadEntries` vectors into Linux `DIR*`/`dirent64` iteration. |
| `std::fs` create/remove dirs/files | Partial | `create_dir`, `remove_file`, and `remove_dir` map to BexOS open/create/unlink. Rename, symlinks, chmod, and canonicalization are not implemented. |
| `std::net::TcpStream` / `TcpListener` | Partial | POSIX socket calls use `bexos.net.Netstack` FIDL and kernel socket byte streams. |
| `std::net::UdpSocket` | Partial | UDP socket calls use `bexos.net.Netstack` FIDL. |
| `std::env` | Minimal | `getenv` returns null; cwd APIs are not implemented. Prefer startup config and explicit namespace paths. |
| async runtimes | Partial | Tokio multi-thread builds against the BexOS std-link path. Poll, epoll, eventfd, timerfd, sockets, futexes, and threads exist for common runtime compatibility. Full Linux parity is not promised. |

## Linux Libc Function Table

| Function or family | Status | Notes |
| --- | --- | --- |
| `malloc`, `calloc`, `realloc`, `free`, `posix_memalign` | Implemented | Backed by `//lib/allocator` over 2 MiB heap VMAR chunks; oversized allocations get dedicated VMO mappings. |
| `memcpy`, `memmove`, `memset`, `memcmp`, `bcmp`, `strlen` | Implemented | Basic C memory/string primitives. |
| `__errno_location` | Implemented | Single process-wide errno slot. |
| `getauxval`, `dl_iterate_phdr` | Stubbed | Return empty values sufficient for current static linking. |
| `_Unwind_*`, `abort` | Stubbed | Abort/unwind paths terminate or return minimal values. |
| ELF `PT_TLS` startup | Partial | Appd aggregates executable and manifest-declared shared-object static TLS after the AArch64 TCB, maps the first thread with that initialized template, publishes the same template through startup linker data for libc-created pthreads, and starts the thread with `TPIDR_EL0` set to the TCB base. TLS destructors and Linux dynamic-loader TLS models such as TLSDESC are not implemented. |
| `clock_gettime`, `nanosleep` | Partial | Real-time, monotonic, and boottime clocks are supported. |
| `open`, `open64`, `openat` | Partial | Supports mounted BexOS namespace paths, read/write/create/truncate/directory/append flags. |
| `read`, `write`, `writev`, `close` | Partial | Supports BexOS file fds plus existing socket/event/timer behavior. |
| `lseek64`, `ftruncate64` | Implemented for files | Routes to BexOS `File.Seek` and file length support. |
| `fstat64`, `stat64`, `lstat64`, `fstatat64` | Partial | Fills Linux/aarch64-compatible core fields from BexOS file attributes. Symlinks are treated like normal paths because symlinks are unsupported. |
| `fsync`, `fdatasync` | Implemented for files | Routes to BexOS `File.Sync`. |
| `mkdir`, `unlink`, `rmdir`, `unlinkat` | Partial | Uses BexOS directory create/unlink behavior. Recursive remove is left to Rust/user code. |
| `opendir`, `fdopendir`, `readdir`, `readdir64`, `rewinddir`, `closedir`, `dirfd` | Partial | libc owns an in-memory directory cursor populated from `Directory.ReadEntries`. |
| `statx` | Stubbed | Returns `ENOSYS` so Rust falls back to `stat64`. |
| `socket`, `connect`, `bind`, `listen`, `accept`, `send`, `recv`, `sendto`, `recvfrom`, `shutdown` | Partial | TCP/UDP over `bexos.net.Netstack`; unsupported domains/types fail with errno. |
| `getsockname`, `getpeername`, `getaddrinfo`, `freeaddrinfo` | Partial | Enough for localhost, DNS through netstack, and TCP address reporting. |
| `poll`, `ppoll`, `epoll`, `eventfd`, `timerfd` | Partial | Compatibility for Tokio/mio-style async runtimes; readiness is only as rich as current fd kinds. |
| `pthread_*`, `sched_yield`, `syscall(SYS_FUTEX)` | Partial | Backed by kernel `TaskControl` FIDL where supported. |
| `mmap64`, `munmap` | Unsupported | File-backed mappings are not implemented through libc. Use BexOS memory APIs directly. |
| `getcwd`, `chdir` | Unsupported | Relative libc paths resolve under `/data`; there is no mutable cwd. |
| `readlink`, `rename`, `chmod`, `pipe2` | Unsupported | Return `ENOSYS` until the corresponding BexOS model exists. |
| `ioctl` | Minimal | Supports `FIONBIO` for socket/runtime nonblocking setup; other requests return `ENOSYS`. |

## Startup Requirements

Rust `std` applications that need BexOS services must receive startup and
install it into libc before using `std::net` or `std::fs`:

```rust
bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    let control = bexos_userspace::Channel(channel);
    let startup = bexos_userspace::Startup::receive(control).expect("startup");
    bexos_libc::install_startup(&startup);

    // std::fs and std::net can now use granted startup capabilities.
    let _ = std::fs::write("state.txt", b"hello from /data\n");

    let _ = bexos_userspace::Startup::ready(control);
    loop {
        bexos_userspace::yield_now();
    }
}
```

For filesystem access, appd supplies namespace directory handles and
paths in startup. Storage-backed applications normally receive:

When an application declares BexOS shared-library dependencies, startup version
5 may also include `linker_data`/`linker_data_len`. `Startup::receive` installs
that metadata before returning, runs declared library constructors once, and
makes exported manifest-prefix symbols available to client crates. Component
config remains in the separate `config`/`config_len` fields.

| Namespace | Use |
| --- | --- |
| `/pkg` | Read-only package contents. Use absolute paths such as `/pkg/bin/app`. |
| `/data` | Writable package data. Relative libc and `std::fs` paths resolve here. |
| `/deps/<name>` | Read-only dependency package roots, when declared by the manifest. |

For networking, the process manifest must consume the netstack service and the
launcher must provide a `bexos.net.Netstack` service grant. After
`install_startup`, libc socket calls can find that endpoint.

## Choosing `no_std`, `alloc`, Libc, or `std`

Use `no_std` for D0 kernel code, early boot, exception paths, and low-level
drivers that must run before heap, namespace, service grants, or thread support
exist.

Use `no_std` plus `alloc` for isolated libraries and D1 components that benefit
from dynamic collections but still need a small, explicit runtime surface.

Use BexOS libc without leaning on broad `std` behavior when porting a focused C
or Rust dependency that only needs memory, time, files, sockets, futexes, or
basic polling.

Use full Rust `std` for D2 services and D3 applications where developer
velocity, crates.io compatibility, collections, formatting, threading, and
ordinary file/network I/O matter more than strict real-time determinism.

Tokio support currently targets the multi-thread runtime with `net`, `time`,
`fs`, `io-util`, `sync`, and macros enabled. `process` and Unix signal support
remain out of scope until BexOS has matching process and signal models. Current
std/Tokio ports still use explicit BexOS channel, memory, migration, and
syscall APIs internally where those semantics are part of the service contract.

D1 userspace drivers may use `std` only when heart transplant support and the
driver's latency model can tolerate allocator, thread, and blocking behavior.
Pre-allocate critical rings and buffers, avoid ambient blocking in interrupt or
packet paths, and keep hardware-facing capability use explicit.

NVMe, BexFS, archivefs, diskimage, VirtIO-Net, trustd, debugd, networkd,
netstackd, vswitchd, traced,
vfsd, powerd, timed, and teed are current concrete std-linked Tokio examples. BexFS, archivefs, and
diskimage keep no-std-compatible cores for host tests and shared logic; NVMe and
VirtIO-Net deploy as std-linked Tokio drivers directly. Their deployed ELFs are
built with `std_service_binary` and use two-worker Tokio runtimes to schedule
FIDL loops and migration quiescence points. NVMe exposes async hardware command
entrypoints with bounded cooperative polling; VirtIO-Net does the same for
packet queue completion while retaining live NIC queue state across heart
transplant without reset. trustd and debugd use Tokio around their service loops
and wire handlers while preserving their explicit BexOS capability and channel
interfaces. timed and teed use the same std/Tokio service wrapper and shared
`//lib/userspace_async` cooperative IPC helpers while preserving their FIDL wire
ABIs and explicit BexOS capability handles. vfsd uses the same async runtime
shape for its direct manager-channel loop while preserving package-store mount
state across migration. Filesystem and image block operations remain synchronous
inside their async server boundaries.
