# RFC-0070 Current State

## Implemented scope: Phase 1 restricted execution

The kernel restricted-execution substrate is implemented for AArch64 and
x86_64. This is a microkernel execution primitive, not Linux syscall emulation
and not container support. The complete future design remains in
[README.md](README.md).

The generated `bexos.kernel.Restricted` syscall protocol is protocol ID 14 and
has four methods: `BindState(options, state_vmo)`, `UnbindState(options)`,
`Enter(options, vector_entry, context)`, and `Kick(thread, options)`. Every
options value must be zero. A successful `Enter` changes the live return frame
and therefore never returns through its FIDL caller; a failed call returns a
normal `Status` response.

`//lib/restricted_abi` is the public no-std shared-page ABI. Version 1 uses an
exactly 4096-byte VMO, magic `BEXRST01`, architecture tags 1 (AArch64) and 2
(x86_64), and a 64-byte header. The header records `SYSCALL`, `EXCEPTION`, or
`KICK`, an architecture exception code, and a fault address. The fixed
AArch64 body contains x0–x30, SP, PC, PSTATE, TPIDR_EL0, and TPIDRRO_EL0. The
x86_64 body contains the SysV GPR set, RIP, RSP, user RFLAGS, FS base, and GS
base. The vector callback is a non-returning C-ABI function receiving the
opaque context in x0/RDI and the reason in x1/RSI.

Binding requires a writable, non-device, non-BootFS, one-page VMO. The kernel
retains its own reference, rejects double bind and active unbind, and releases
the retained reference on unbind, thread exit, or process termination. Enter
validates the state header, executable guest and vector addresses, writable
stacks, canonical user TLS values, and architecture tag. It forces EL0/ring-3
return state and masks privileged PSTATE/RFLAGS bits. Host and guest GPR, TLS,
SIMD/FPU, vector, active, and pending-kick state are kept separately in the
thread record.

Status behavior is deterministic: non-zero options, malformed state, invalid
addresses or VMO shape, and dead/unbound kick targets return `ErrInvalidArgs`;
insufficient VMO/thread rights return `ErrAccessDenied`; wrong handle types
return `ErrInvalidHandle`; double bind, re-enter while active, and active
unbind return `ErrAlreadyExists`.

AArch64 reflects every restricted `svc` as `SYSCALL`. x86_64 programs LSTAR
and reflects `syscall` as `SYSCALL`; `int 0x80`, `sysenter`/invalid opcodes,
and unresolved user faults are `EXCEPTION` exits. Syscall exits report the
architectural next PC. Fault exits retain the faulting PC and include ESR on
AArch64 or `(vector << 32) | error_code` on x86_64 plus the fault address when
applicable. Lazy anonymous write faults are resolved first. Device and timer
interrupts stay kernel-owned and normally resume the restricted frame.

`Kick` is idempotent for a live bound thread. An inactive thread consumes the
pending kick on its next Enter. An active thread exits at the next interrupt or
scheduling boundary; a target assigned to another CPU receives a reschedule
interrupt.

Full runtime snapshots are version 21 and incremental records are version 3.
Version 20 snapshots containing interrupt state remain supported by the decoder.
Both retain their immediately older decoders. Restricted bindings, retained
VMO identity, host/guest contexts, both AArch64 TLS registers, vector values,
active state, transition state, and pending kicks are serialized. Restore
validates the referenced VMO and state invariants before scheduling a thread.

## Userspace and acceptance fixture

`bexos_userspace::restricted` creates and maps the state VMO, binds and
unbinds it, enters restricted execution, and kicks thread handles. Its unsafe
enter contract documents the executable-vector, writable-stack, C ABI, and
non-returning callback requirements.

`//testing/e2e/qemu/kernel/restricted:restricted_test` packages a native probe
for both maintained architectures. The probe checks raw `syscall`/`svc`
register and next-PC reflection, changes the return register and re-enters,
checks an exception without a kernel failure, kicks a restricted sibling, then
unbinds and requires debugd/kernel health.

Host validation completed on 2026-09-23:

- AArch64 and x86_64 `//kernel:kernel` builds passed.
- AArch64 and x86_64 restricted probe builds passed.
- `//lib/restricted_abi:tests`, `//kernel/core:core_tests`, and
  `//kernel/core:architecture_tests` passed under both architecture configs.
- FIDL, userspace, matrix-coverage, and final Bazel rustfmt validation passed.

The dual-architecture QEMU command was attempted, but neither probe result is
claimed. AArch64 boots through drivers and wave-6 services before an existing
storage-channel failure prevents `timed` and terminates appd before probe
launch. The x86_64 target is blocked before QEMU by an existing non-exhaustive
`firmware::Call::Resolve` match in the external secure monitor. Exact commands
and diagnostics are recorded in [testing status](../../testing-status.md); live
acceptance must not be inferred from the passing host builds.

## Explicit Phase 2–4 gaps

There is no vendored Starnix tree, Zircon compatibility layer, Linux ELF
runner, Linux syscall table, Linux VFS, Android runtime, OCI image ingestion,
container lifecycle, namespace/cgroup emulation, or Linux networking bridge.
No Alpine, Python, Redis, Docker, Podman, Kubernetes, or general Linux binary
execution is claimed. Those remain the future Phase 2–4 design in the RFC.
