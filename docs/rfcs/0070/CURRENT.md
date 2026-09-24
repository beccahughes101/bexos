# RFC-0070 Current State

## Status

Phases 1–3 are implemented in the tree for the static, single-Linux-task
runtime described below. Phase 2 and Phase 3 are not marked guest-accepted:
the required AArch64 and x86_64 QEMU executions have not both completed
successfully, and this Phase 3 change was explicitly validated without e2e
runs. Phase 4 remains proposed. The complete future design, including features
outside the current single-task boundary, is retained in [README.md](README.md).

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
| VMO/VMAR | `bexos_userspace::Memory` | exact map, unmap, W^X rights |
| Channels/sockets | BexOS channels/native sockets | pair, send/receive |
| Clock | BexOS monotonic ticks | monotonic only |
| Futex/thread | TaskControl/restricted protocol | wait, wake, kick |
| Restricted state | RFC-0070 protocol 14 | bind, enter, unbind, kick |

`Memory::map_at` is the exact-address primitive used for Linux `PT_LOAD`
segments. It retains the existing bounds, overflow, alignment, cleanup, and
W^X validation in the kernel memory service.

## Public runner contract

`bexos.app.NixRunnerOptions` has stable fields `path = 1`, repeated
`arguments = 2`, repeated `{ name, value } environment = 3`, and
`rootfs = 4`. The rootfs selects either the package or data startup directory
and an optional root-bounded subpath; omission remains backward-compatible and
selects the package root. A manifest selects it with `runner: "nix"`. Options,
appd-to-runner launch records, and signal-control messages are bounded and
versioned by `//lib/starnix_abi`.

`RunnerPolicy.allow_starnix_runner = 7` defaults to false. Maintained QEMU
product prototxts enable it. Other products, including Android policy, keep
their existing microVM fallback or denial behavior.

Appd resolves the package payload, validates the Linux ELF, authenticates the
embedded platform runner by SHA-256, launches only that trusted native image,
and transfers a payload VMO plus the versioned startup record. Partial launch
failures terminate the process and release mappings, VMOs, channels, threads,
and process handles.

## Linux execution scope

The trusted `//services/starnix_runner` runtime creates one Starnix task, maps
a matching-architecture ELF64 static `ET_EXEC` or static PIE image, builds the
Linux argc/argv/environment/`AT_NULL` stack, binds restricted state, and
dispatches architecture-specific AArch64 or x86_64 Linux syscall tables.
`PT_INTERP` remains rejected.

The Phase 3 syscall slice includes:

- descriptor I/O (`read`, `write`, vectored and positioned I/O, seek, dup,
  close/range, flags, sync, and terminal ioctls);
- pathname and metadata operations (`open/openat/openat2`, stat variants,
  directory enumeration, access, cwd/chdir, mkdir, unlink/rmdir, and truncate);
- anonymous and file-copy mappings, `munmap`, `mprotect`, `msync`, `madvise`,
  `brk`, futex wait/wake, and x86_64 FS/GS base control;
- signal actions and masks, queued delivery, alternate stacks, sigreturn,
  `kill`/`tgkill`, appd-delivered process-control signals, clocks, sleep/yield,
  identity, uname, limits, and random bytes.

Unknown syscall numbers return `ENOSYS`. Recognized operations outside the
current boundary return deterministic Linux errors, normally `ENOTSUP`.
Pointer and transfer lengths are checked against mapped guest ranges and
bounded allocations.

The mounted VFS uses the selected `/pkg` or `/data` startup directory as `/`,
prevents `..` from escaping it, and overlays the other explicitly supplied
startup namespace directories. Package/dependency mounts are read-only; data,
tmp, and writable BexFS directories retain their granted rights. `/dev/null`
and `/dev/zero` are synthetic. Command launches use the three native startup
sockets for stdin/stdout/stderr, allowing the shell/terminal frontend (and its
scened presentation) to own interactive I/O; service launches use the native
console log path.

The normal and replacement runner ELFs, signed archives, runtime digest, and
AArch64/x86_64 static Linux fixtures are all Bazel-built. The acceptance
fixture must emit exactly `hello starnix\n` and exit zero.

## Heart transplant

A migratable `nix` service checkpoints its architecture, upstream revision,
ABI version, bounded options, restricted register image, current split mapping
descriptors and VMO offsets, signal actions/masks/pending set/alternate stack,
active signal frames, terminal and directory cursor state, cwd, reopenable file
descriptor metadata, mapping VMOs, and migration endpoint. The candidate
rejects a revision, ABI, architecture, or launch-option mismatch, adopts
mappings at identical virtual addresses, restores the runtime state, binds a
new restricted state object, and resumes after kernel cutover. Appd issues a
restricted kick whenever migration or process-control signal delivery needs a
safe point, so a guest spinning without syscalls reaches the non-returning
vector within the deadline.

`//testing/e2e/qemu/starnix` contains a static hello fixture and a looping
fixture whose register state, writable guest memory, process identity, and
output sequence are checked across replacement.

## Phase 3 validation record

The focused host unit suites and runner builds pass. No command under
`//testing/e2e` was changed or run for this phase:

```sh
bazel test //lib/starnix_abi:tests \
  //third_party/starnix:starnix_core_tests \
  //third_party/starnix:starnix_kernel_tests \
  //services/starnix_runner:tests \
  //services/appd:appd_tests //services/appd:shell_policy_tests
bazel build //lib/starnix_abi //lib/compat/zircon \
  //third_party/starnix:starnix_core \
  //third_party/starnix:starnix_kernel \
  //services/starnix_runner:starnix_runner_elf \
  //services/starnix_runner:replacement_elf

bazel build --config=x86_64 //lib/starnix_abi //lib/compat/zircon \
  //third_party/starnix:starnix_core \
  //third_party/starnix:starnix_kernel \
  //services/starnix_runner:starnix_runner_elf \
  //services/starnix_runner:replacement_elf
bazel run @rules_rust//:rustfmt
```

Detailed host results and the older Phase 2 QEMU attempt are recorded in
[testing status](../../testing-status.md). A successful host build or unit test
is not evidence of Linux guest execution or LTP guest conformance.

## Explicit remaining gaps

The current Phase 3 acceptance boundary is a static ELF and one Linux task.
Dynamic binaries and `PT_INTERP`, fork/clone and multiple Linux tasks or
threads, rename/link/symlink, pipes and polling, complete `/proc` and `/sys`,
the full LTP corpus, networking, Android, OCI ingestion, container lifecycle,
namespaces, cgroups, Alpine/Python/Redis, Docker/Podman/Kubernetes, and direct
hardware access are unimplemented. They remain future design and are not
deleted from the RFC. Phase 4 owns OCI and networking.
