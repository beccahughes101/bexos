# RFC-0070 Current State

## Status

Phases 1–3 and the opt-in networking portion of Phase 4 are present in the tree. The
runner supports bounded cooperative Linux process and thread groups with
separate copy-on-write address spaces. Offline remains the default, while named
RFC 68 profiles enable direct TCP/UDP or virtual-L2 networking. Phases 2–4 are not marked
guest-accepted: the required AArch64 and x86_64 QEMU scenarios have not both
completed successfully. The AArch64 source-firmware path now boots through
normal-world startup and debugd readiness after correcting overlap between the
secure owner image and its reserved page-table arena. The latest acceptance
run continued through generation 356 without a reported guest failure before
it was stopped at the user's request; x86_64 was not rerun after the final
recovery changes. The complete future design is retained in
[README.md](README.md).

## Upstream import and Bazel targets

Starnix is pinned to Fuchsia revision
`cb5f36aee5510be9565392194f341d0409381db6`. `//third_party/starnix` contains
the complete imported `src/starnix/kernel`, `src/starnix/lib`, and
`hello_starnix` sources, their GN metadata, the upstream BSD license, and a
prototxt import manifest. The original Fuchsia roots are preserved as
`fuchsia_lib.rs` and `fuchsia_main.rs`; the Bazel targets enter through the
corresponding vendored upstream paths with BexOS platform gates:

- `//third_party/starnix:starnix_core`
- `//third_party/starnix:starnix_kernel`

The BexOS configuration replaces Fuchsia component hosting, storage, logging,
and kernel-object services. Phase 3 is implemented by small BexOS-specific VFS,
memory, signal, ABI-layout, and syscall-dispatch modules around the pinned
upstream boundary rather than enabling Fuchsia platform services.

## Compatibility surface

`//lib/compat/zircon` owns BexOS handles and exposes the Phase 2 subset needed
by the runner: status and rights translation, VMO creation/duplication, exact
VMAR mapping, channels, sockets, clocks, futexes, threads, and restricted
execution. Unsupported operations return deterministic `NOT_SUPPORTED` or the
corresponding typed error; the accepted load/write/exit path has no panic stub.

| Zircon-style facility | Phase 2 backing | Current scope |
| --- | --- | --- |
| Handles and rights | BexOS capability handles | owning close and duplicate |
| VMO/VMAR | `bexos_userspace::Memory` | exact map, snapshot, unmap, W^X rights |
| Channels/sockets | BexOS channels/native sockets | pair, send/receive |
| Clock | BexOS monotonic ticks | monotonic only |
| Futex/thread | TaskControl/restricted protocol | wait, wake, kick |
| Restricted state | RFC-0070 protocol 14 | bind, enter, unbind, kick |

`Memory::map_at` is the exact-address primitive used for Linux `PT_LOAD`
segments. It retains the existing bounds, overflow, alignment, cleanup, and
W^X validation in the kernel memory service.

## Public runner contract

`bexos.app.NixRunnerOptions` has stable fields `path = 1`, repeated
`arguments = 2`, repeated `{ name, value } environment = 3`, `rootfs = 4`,
`working_directory = 5`, `uid = 6`, `gid = 7`, `umask = 8`, repeated resource
limits at `9`, `hostname = 10`, and repeated `{ profile, interface_name }`
network attachments at field 11. The launch record is version 3 and still
decodes the version-2 offline record. The rootfs selects either the package or
data startup directory and an optional root-bounded subpath; omission remains
backward-compatible and selects the package root. A manifest selects it with
`runner: "nix"`. Options and migration records are bounded and versioned by
`//lib/starnix_abi`; launch and signal control use the standard RFC 72 FIDL
protocols.

`RunnerPolicy.allow_starnix_runner = 7` defaults to false. Maintained QEMU
product prototxts enable it. Other products, including Android policy, keep
their existing microVM fallback or denial behavior.

Appd resolves the package and dependency directories without interpreting the
Linux payload. The platform prototxt registry selects an independently signed
Starnix provider package; one provider instance is mapped into the component
job and receives `ComponentRunner.Start` with opaque `NixRunnerOptions`, the
directory capabilities, and the versioned startup record. Starnix validates
the option type, path, bounds, and ELF format itself. The
fixed BootFS native host performs this mapping with delegated
job/process/VMAR handles.
`ComponentController.SendSignal` replaces the former private signal
transport. Partial launch failures terminate the complete component job and
release mappings, VMOs, channels, threads, and process/job handles.

## Linux execution scope

The trusted `//services/starnix_runner` runtime creates one Starnix process and
maps matching-architecture ELF64 `ET_EXEC` or PIE images. Static images and
dynamic images using an absolute `PT_INTERP` are accepted; the interpreter is
resolved inside the selected rootfs and mapped at a separate load bias. The
Starnix core and runner libraries receive the Bazel guest-architecture setting,
with configuration-specific tests covering executable validation, syscall
selection, ABI layouts, and `uname` identity on both AArch64 and x86_64. The
initial Linux stack includes argc/argv/environment and the loader aux vector
(`AT_PHDR`, `AT_PHENT`, `AT_PHNUM`, `AT_PAGESZ`, `AT_BASE`, `AT_ENTRY`,
credentials, random bytes, and `AT_EXECFN`). Appd validates the main ELF while
the runner validates and loads the rootfs interpreter.

The Phase 3 syscall slice includes:

- descriptor I/O (`read`, `write`, vectored and positioned I/O including
  `preadv`/`pwritev`, `sendfile`, seek, dup,
  close/range, flags, sync, terminal ioctls, one-way pipes, local stream
  `socketpair`, pathname and abstract-namespace `AF_UNIX` stream sockets,
  shutdown, `eventfd`, `epoll`, `timerfd`, and readiness through poll/select
  variants);
- pathname and metadata operations (`open/openat/openat2`, stat variants,
  directory enumeration, `access`/`faccessat2`, cwd/chdir/fchdir, mkdir,
  unlink/rmdir, rename, hard links, symlinks and no-follow metadata/readlink,
  filesystem capacity/type queries through `statfs`/`fstatfs`,
  permission and ownership mutation through the chmod/chown families,
  extended-attribute get/set/list/remove variants, `/proc/self/exe` readlink,
  truncate, and in-place `execve`/`execveat`);
- descriptor synchronization and advisory-I/O hints, including
  `fsync`/`fdatasync` and validated `fadvise64`;
- anonymous and file-copy mappings, `munmap`, `mprotect`, `msync`, `madvise`,
  bounded `mremap` shrink and `MREMAP_MAYMOVE`/`MREMAP_FIXED` relocation,
  `brk`, futex wait/wake plus selective nonzero bitset wait/wake with absolute
  deadlines, atomic/comparative `FUTEX_WAKE_OP`, plain
  `FUTEX_REQUEUE` and `FUTEX_CMP_REQUEUE` across private or shared queues,
  bounded futex2 `futex_waitv` vectors with private/shared entries and indexed
  wake results,
  private and shared priority-inheritance lock/try-lock/unlock with
  owner-directed scheduling, timed waits, handoff, and atomic condition-wait
  transfer through `FUTEX_WAIT_REQUEUE_PI`/`FUTEX_CMP_REQUEUE_PI`,
  per-thread robust-list registration and bounded owner-death recovery/wakeup,
  per-thread `rseq` registration/unregistration with virtual CPU 0 publication,
  fork/thread/exec lifecycle semantics, and x86_64 FS/GS base control;
- signal actions and masks, pending-set inspection, interruptible
  `rt_sigsuspend` waits, queued delivery, alternate stacks, sigreturn,
  `kill`/`tgkill`, appd-delivered process-control signals, clocks, sleep/yield,
  configured and mutable real/effective/saved/filesystem identities,
  supplementary groups, hostname/umask, mutable resource limits, common
  process queries, mutable nice priority, default-policy scheduler
  query/update calls, affinity/sysinfo/rusage/prctl helpers, and random bytes.
- cooperative `clone`/`clone3` thread groups with architecture-correct TLS,
  parent/child TID stores, clear-TID futex wakeup, unique TIDs, group/thread
  exit, sleeping, and futex wait/wake scheduling. Thread registers, waits,
  clear-TID state, and signal frames are versioned for transplant;
- bounded `fork`, `vfork`, and process-style `clone` with copy-on-write or
  explicitly shared VMOs, independent descriptor tables with duplicated
  handle-backed descriptions, inherited signal dispositions, parent/child TID
  stores, process-aware PID/PPID queries and signals, orphan reparenting,
  zombie retention, and blocking or `WNOHANG` `wait4`. Address spaces are
  serialized over the runner's restricted thread rather than represented as
  separate BexOS processes.

An attachment-less workload remains offline: non-local socket families return
`EAFNOSUPPORT`. Named attachments build one Linux network namespace with
loopback and ordered interfaces. Direct interfaces use permanently
domain/table-scoped RFC 68 providers for IPv4/IPv6 TCP and UDP. Virtual-L2
interfaces use leased vswitchd rings and a runner-local packet plane for TCP,
UDP, ICMP, and authorized `AF_PACKET` traffic. Longest-prefix and metric route
selection, `SO_BINDTODEVICE`, DNS, common socket options, nonblocking
connect/accept, polling, packet info, multicast membership, and Linux error
translation are namespace aware. Packet-info enablement and joined multicast
groups are socket state and survive runner transplant.

`clone`/`unshare`/`setns` implement `CLONE_NEWNET`; `/proc/<pid>/ns/net` file
descriptors retain namespace ownership. A new namespace contains loopback
only. Route netlink exposes links, addresses, routes, veth pairs, software
bridges, interface moves, MTU, and link state. `/proc/net`, `/sys/class/net`,
route/netfilter/sock-diag netlink, common network ioctls, namespace-local
firewall/NAT state, and the bounded networking capability set are carried in
the runner snapshot. Netfilter batches are atomic. The supported nftables and
legacy IPv4 iptables subset covers interface/address/protocol/port/state
matches, counters, accept/drop/reject, and bounded SNAT/DNAT/masquerade;
unsupported extensions fail the complete update with `EOPNOTSUPP`. Host
profile policy remains immutable from Linux.

TCP/UDP controls, raw queues, vNIC ring mappings/backlogs, provider/lease
handles, namespace descriptors, routes, neighbors, bridges/veth, firewall, and
conntrack state participate in heart transplant. Existing offline snapshots
remain accepted. The reusable Ethernet ring client lives outside netstackd so
both netstackd and the Starnix L2 backend use the same checked ring codec.

Unknown syscall numbers return `ENOSYS`. Recognized operations outside the
current boundary return deterministic Linux errors, normally `ENOTSUP`.
Pointer and transfer lengths are checked against mapped guest ranges and
bounded allocations.

Descriptor behavior includes Linux close-on-exec separation from file status
flags: `dup`/`dup2` clear it, `dup3` and `F_DUPFD_CLOEXEC` can set it,
`F_GETFD`/`F_SETFD` expose it, and `close_range` supports both close and
close-on-exec marking. Process descriptor tables remain independently cloned
across `fork`.

Path access and existing-file opens apply Linux owner/group/other mode bits to
the task's real, effective, or filesystem credentials as appropriate;
supplementary groups and root's execute-bit rule are included. Backing storage
capability rights remain an additional restriction rather than being replaced
by emulated UID 0. Ownership and permission mutation also require Linux root or
the applicable owner/group relationship.

The mounted VFS uses the selected `/pkg` or `/data` startup directory as `/`,
prevents `..` from escaping it, and overlays the other explicitly supplied
startup namespace directories. Package/dependency mounts are read-only; data,
tmp, and writable BexFS directories retain their granted rights. `/dev/null`
and `/dev/zero`, random/full devices, the offline `/proc` process/memory/mount
view, and the CPU-oriented `/sys` tree are synthetic. `/proc/self/maps` and
`/proc/thread-self/maps` are refreshed from the active address space before
each syscall, including permissions, private/shared state, file offsets, and
stack/image/vDSO labels. The corresponding `/proc/*/task` directories are
refreshed from the cooperative thread group, while `status` and `stat` expose
the active PID/TID, parent, credentials, supplementary groups, task name, nice
value, and thread count. Command launches use the three native startup sockets
for stdin/stdout/stderr, allowing the
shell/terminal frontend (and its scened presentation) to own interactive I/O;
service launches use the native console log path.

## OCI runtime and opt-in networking

`//lib/oci` strictly validates an OCI image-layout archive without performing
network I/O. It selects a matching Linux architecture, verifies descriptor
sizes and SHA-256 digests through index, manifest, config, and layers, verifies
uncompressed diff IDs, bounds per-layer and whole-image allocations and entry
counts, accepts raw, gzip, and zstd layer media types, rejects duplicate outer
archive paths, validates tar checksums and PAX/GNU long names, and
produces an ordered layer plan including whiteouts, opaque directories, files,
directories, symlinks, and hard links. Bounded raw `SCHILY.xattr.*` and strict
base64 `LIBARCHIVE.xattr.*` PAX records, including binary
`security.capability`, are preserved through the common filesystem and direct
BexFS sinks without following the final symlink. Unsafe absolute or
parent-traversing archive paths are rejected. The new
`bexos.container.ContainerManager` FIDL and
`ContainerSpec` protobuf define a durable lifecycle. Version-2 records add
ordered network attachments at field 13; version-1 offline records remain
readable. `CreateWithNetwork`, `LaunchContainerWithNetwork`, and versioned
inspection are additive FIDL methods, and the CLI accepts repeatable
`--network PROFILE[:IFNAME]`. Omitting every flag remains offline.

`//services/containerd` is the authenticated, transplantable lifecycle owner.
It resolves only `ArtifactKind::Container` through pkgd, verifies the OCI
layout for the matching architecture, merges bounded image/request process
configuration, applies layers through the common filesystem protocol into a
private staging directory, writes `config.prototxt`, and publishes it with one
directory rename. Startup reloads durable records. Create, start, signal,
delete, inspect, and sorted list are implemented. Appd alone creates the child
`apps` resource group and authenticates the embedded Starnix runner.
Process-control clients use containerd proxies so daemon monitoring and CLI
waits cannot race on one channel receive queue.

`//apps/container_cli` packages create, start, run, inspect, list, signal, and
delete. Network access is supplied only by the ordered, policy-authorized
attachments in the durable container record; there is no ambient network
capability. Daemon, CLI, pkgd, and the runner archive are included in both
product graphs.

The normal and replacement runner ELFs, independently signed provider
archives, and AArch64/x86_64 Linux fixtures are all Bazel-built. Appd does not
embed a Starnix ELF or runtime digest; provider identity, path, and expected
signer come from the RFC 72 platform registry. In addition to the static
hello and looping fixtures, the archive contains a real dynamic musl PIE with
`PT_INTERP=/lib/ld-musl.so.1` and `DT_NEEDED=libc_musl.so`, plus those runtime
files inside the fixture root. It creates a pipe, forks, transfers data from
child to parent, checks `waitpid` status, and exercises writable `/data`,
close-on-exec descriptor duplication, hard links, relative symlinks/readlink,
and xattr set/get/list/remove across a shared hard-link inode, then grows and
verifies an anonymous mapping with `MREMAP_MAYMOVE`. It also writes two pages
through `MAP_SHARED`, splits their protections with `mprotect`, synchronizes
them, and reads both file offsets back. File creation applies the configured
umask and ownership, and the fixture exercises `chmod`, `fchmod`,
`fchown`, supplementary groups, and no-op `setresuid`/`setresgid` transitions.
It also checks `O_CREAT|O_EXCL` and `O_NOFOLLOW` rejection semantics,
Linux `statx` and `statfs`/`fstatfs` layouts, positional vectored I/O,
`sendfile` data and offset semantics, advisory I/O, mutable process priority,
scheduler queries, a populated `/proc/self/maps` image/stack view, and
synchronized `/proc/thread-self/status` process/thread identity, and selective
shared and private futex bitset wait/wake
operations. It also blocks, queues, and observes a pending local signal, then
verifies that a
child-delivered signal wakes `sigsuspend` and runs its handler. The fixture also
registers and queries a robust futex list in a shared-address-space child and
verifies `FUTEX_OWNER_DIED` plus waiter-bit preservation after that owner exits.
It then contends a shared PI futex between parent and `CLONE_VM` child, verifies
the waiter bit and ownership handoff, and checks self-lock and unowned-unlock
errors. A second shared-memory child wakes the second entry of a futex2 wait
vector and the parent verifies the returned index. A final child blocks on one
shared futex, the parent requeues it without waking to a second futex, and the
child resumes only after the destination queue is woken.
The fixture also uses `FUTEX_WAKE_OP` to atomically increment a second shared
futex, compare its original value, and conditionally wake a waiting child.
Finally, it moves a child from a condition futex to a held PI mutex with the PI
requeue pair, observes the waiter bit, and verifies ownership handoff.
An aligned restartable-sequence area is registered in a new shared-VM child;
the fixture checks CPU identifiers, duplicate-registration and mismatched
unregistration errors, successful unregistration, and ABI-size validation.
It emits exactly
`dynamic musl process ok\n`, and exits zero. Both architecture-selected
archives build and their ELF metadata has been inspected; guest execution is
not claimed because the final Linux fixture scenario described below was not
completed.

## Heart transplant

A migratable `nix` service checkpoints its architecture, upstream revision,
ABI version, bounded options, restricted register image, current split mapping
descriptors and VMO offsets, signal actions/masks/pending set/alternate stack,
active signal frames, terminal and directory cursor state, credentials,
supplementary groups, process priority, hostname, umask, limits, task name,
cwd, reopenable file descriptor metadata, local socket handles, mapping VMOs,
and migration endpoint. The candidate also retains every cooperative process
address space and mapping metadata, PID/PPID and parent-exit signal, vfork
relationship, pending child-memory writes, zombies, and every thread's register
frame, TID,
clear-TID address, blocked futex/sleep/wait/signal-suspend state, and
signal-frame stack, including each thread's robust-list head and PI-futex
scope, owner, deadline, and wait ordering, plus bounded futex2 wait vectors.
Per-thread `rseq` address, ABI length, and signature are versioned with the
runtime state; restored tasks continue to publish the runner's single virtual
CPU identifier. Open synthetic procfs/sysfs/device descriptors retain their
exact bytes, directory type, status flags, and cursor across transplant rather
than being reconstructed from their pathname.
Containerd retains its authenticated pkgd/appd endpoints, bound clients,
pending resolution, running process controls, stdio drains, and
process-control proxies.

The candidate
rejects a revision, ABI, architecture, or launch-option mismatch, adopts
mappings at identical virtual addresses, restores the runtime state, binds a
new restricted state object, and resumes after kernel cutover. Appd issues a
restricted kick whenever migration or process-control signal delivery needs a
safe point, so a guest spinning without syscalls reaches the non-returning
vector within the deadline.

`//testing/e2e/qemu/starnix` contains a static hello fixture, the dynamic musl
process/IPC fixture, and a looping fixture whose register state, writable
guest memory, process identity, and output sequence are checked across
replacement.

## Validation record

The focused host unit suites, integration suites, and both maintained product
assemblies passed during implementation. The validation record includes:

```sh
bazel test //lib/starnix_abi:tests \
  //lib/starnix_net:tests \
  //third_party/starnix:starnix_core_tests \
  //third_party/starnix:starnix_kernel_tests \
  //services/starnix_runner:tests \
  //services/appd:appd_tests //services/appd:shell_policy_tests \
  //services/pkgd:tests //lib/pkg_config:pkg_config_tests \
  //lib/oci:tests //lib/oci_bexfs:tests \
  //services/containerd:tests //apps/container_cli:tests \
  //services/networkd:internal_tests \
  //services/netstack:internal_tests //services/netstack:netstackd_tests \
  //drivers/d1/storage/bexos/memfs:memfs_tests \
  //drivers/d1/storage/bexos/memfs:memfs_async_tests \
  //drivers/d1/storage/bexos/bexfs:bexfs_tests \
  //drivers/d1/storage/bexos/bexfs:bexfs_async_tests \
  //drivers/d1/storage/bexos/archivefs:archivefs_tests \
  //drivers/d1/storage/bexos/archivefs:archivefs_async_tests \
  //lib/bexos_libc:internal_tests //lib/bexos_libc:bexos_libc_tests \
  //:heart_transplant_coverage_test
# Architecture-sensitive runner and ABI targets also pass with --config=x86_64.
bazel build //services/appd:appd \
  //services/starnix_runner:starnix_runner_archive \
  //services/starnix_runner:replacement_archive \
  //services/containerd:containerd_archive \
  //services/containerd:replacement_archive \
  //services/networkd:networkd_archive //services/networkd:replacement_archive \
  //services/netstack:netstackd_archive //services/netstack:replacement_archive \
  //services/vswitchd:vswitchd_archive //services/vswitchd:replacement_archive \
  //apps/container_cli:container_cli \
  //device/virtual/qemu/nongui:virtual_aarch64_product_assembly
bazel build --config=x86_64 //services/appd:appd \
  //services/starnix_runner:starnix_runner_archive \
  //services/starnix_runner:replacement_archive \
  //services/containerd:containerd_archive \
  //services/containerd:replacement_archive \
  //services/networkd:networkd_archive //services/networkd:replacement_archive \
  //services/netstack:netstackd_archive //services/netstack:replacement_archive \
  //services/vswitchd:vswitchd_archive //services/vswitchd:replacement_archive \
  //apps/container_cli:container_cli \
  //device/virtual/qemu/nongui:virtual_x86_64_product_assembly
bazel build //testing/e2e/qemu/starnix:archive
bazel build --config=x86_64 //testing/e2e/qemu/starnix:archive
bazel run @rules_rust//:rustfmt
```

The secure-owner page tables and stacks now live beyond the bounded owner
code/state image, with a linker assertion preventing that overlap from
returning. A source-firmware AArch64 run subsequently passed the harness boot
phase and debugd readiness. Repeated continuation runs reached fixture upload
and appd's durable installation path, exposing and correcting package-cache,
read-only boot-state, ArchiveFS resource/error, containerd startup-order, and
nested timeout defects. The final rerun after raising debugd's aggregate
lifecycle deadline and the host client's outer deadline was stopped at the
user's request, and no x86_64 guest run was performed for the final tree.
Accordingly, neither the dynamic-musl success marker nor the transplant
scenario has a passing guest result. The user also requested that the final
regression matrices be skipped; the latest storage and timeout changes have
focused passing coverage but not a new complete matrix. Detailed results are
recorded in [testing status](../../testing-status.md). Host builds, unit tests,
and a completed platform boot are not evidence of Linux guest execution or
LTP guest conformance.

## Explicit remaining gaps

The current implementation has bounded cooperative process/thread groups, but
does not yet have a passing guest acceptance run for those semantics. The full
LTP corpus, Android, broader namespaces, Alpine/Python/Redis guest acceptance,
and Docker/Podman/Kubernetes compatibility are not accepted. Direct hardware
access and ambient, attachment-free guest networking remain excluded. The product has no
default remote registry trust root, so deployments must explicitly provision
pkgd repository/trust policy. All broader future design remains in the RFC
rather than being deleted.
