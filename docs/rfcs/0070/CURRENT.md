# RFC-0070 Current State

## Status

Phase 1 is implemented. The complete Phase 2 implementation is present in the
tree and its host builds pass; Phase 2 is not marked accepted until the new
AArch64 and x86_64 QEMU tests both complete successfully. Phases 3–4 remain
proposed. The complete future design is retained in [README.md](README.md).

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

The Phase 2 configuration intentionally replaces Fuchsia component hosting,
storage, logging, and kernel-object services. It does not claim that the
source-visible Phase 3 modules are enabled.

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
`arguments = 2`, and repeated `{ name, value } environment = 3`. A manifest
selects it with `runner: "nix"`. Options and appd-to-runner launch records are
bounded and versioned by `//lib/starnix_abi`.

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
dispatches the imported Phase 2 syscall boundary. Supported syscalls are:

- `write` to stdout or stderr;
- `exit`;
- `exit_group`.

Unknown syscall numbers return Linux `ENOSYS`; bad descriptors and pointers
return `EBADF` and `EFAULT`. `PT_INTERP` is rejected. The bootstrap namespace is
in memory and contains only launch metadata required by the single task.

The normal and replacement runner ELFs, signed archives, runtime digest, and
AArch64/x86_64 static Linux fixtures are all Bazel-built. The acceptance
fixture must emit exactly `hello starnix\n` and exit zero.

## Heart transplant

A migratable `nix` service checkpoints its architecture, upstream revision,
ABI version, bounded options, restricted register image, mapping descriptors,
mapping VMOs, and migration endpoint. The candidate rejects a revision, ABI,
architecture, or launch-option mismatch, adopts mappings at identical virtual
addresses, restores registers, binds a new restricted state object, and resumes
after kernel cutover. Appd issues a restricted kick whenever it waits for a
Starnix source response, so a guest spinning without syscalls reaches the
non-returning vector safe point within the migration deadline.

`//testing/e2e/qemu/starnix` contains a static hello fixture and a looping
fixture whose register state, writable guest memory, process identity, and
output sequence are checked across replacement.

## Validation record

The following builds have passed for the default AArch64 configuration and for
`--config=x86_64` where noted:

```sh
bazel build //lib/starnix_abi //lib/compat/zircon \
  //third_party/starnix:starnix_core \
  //third_party/starnix:starnix_kernel \
  //services/starnix_runner:starnix_runner_elf \
  //services/starnix_runner:replacement_elf \
  //testing/e2e/qemu/starnix:archive

bazel build --config=x86_64 //lib/starnix_abi //lib/compat/zircon \
  //third_party/starnix:starnix_core \
  //third_party/starnix:starnix_kernel \
  //services/starnix_runner:starnix_runner_elf \
  //services/starnix_runner:replacement_elf \
  //testing/e2e/qemu/starnix:archive
```

Unit-test, rustfmt, and QEMU attempt results are recorded in
[testing status](../../testing-status.md). The requested final validation
stopped before dual-architecture guest acceptance: the first AArch64 attempt
reached appd and failed the `nix` launch, the x86_64 prerequisite was repaired,
and the diagnostic guest rerun was then skipped at the requester's direction.
A successful build is not evidence of Linux guest execution.

## Explicit Phase 3–4 gaps

Persistent root filesystems, general VFS/POSIX coverage, dynamic binaries and
`PT_INTERP`, fork/clone and multiple Linux tasks or threads, general sockets
and networking, Android, OCI ingestion, container lifecycle, namespaces,
cgroups, Alpine/Python/Redis, Docker/Podman/Kubernetes, and direct hardware
access are unimplemented. These remain future design and are not deleted from
the RFC.
