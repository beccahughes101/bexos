# Kernel

## Build Targets

The current kernel package exposes:

- `//kernel:kernel`: selected-architecture no-std kernel (`aarch64-virt.ld` or `x86_64-q35.ld`).
- `//kernel:update_kernel`: selected-architecture replacement (`aarch64-update.ld` or `x86_64-update.ld`), stripped and built with `--cfg bexos_update_kernel`.
- `//kernel/core:core`: host-testable no-std kernel model library.
- `//kernel/core:core_tests`: host tests for the core library.
- `//kernel/smoke`: smoke target for kernel build/link behavior.

Both kernel binaries use panic abort, static relocation, and forced frame pointers.

## Kernel Runtime Modules

`kernel/src` contains the hardware-facing kernel implementation:

- `main.rs`: kernel entry flow.
- `arch`: `ArchAPI` and compile-time `CurrentArch`, implemented in `aarch64` and `x86_64`.
- Architecture modules own boot assembly, private trap frames, CPU, MMU, interrupts, time, early console, power and transplant control.
- `memory`: shared physical memory ownership and reclamation policy.
- `ipc`: runtime IPC integration.
- `sched`: CPU-local scheduler integration, one-shot timer deadline selection,
  and architecture-selected reschedule interrupts for secondary CPUs.
- `syscall`: syscall dispatch.
- `tracing`: kernel trace producer attachment metadata and privileged
  attach/detach state.
- `userspace`: external BootFS appd bring-up.
- `migration` and `transplant`: live update / heart transplant integration.
- `panic`: panic handling.
- Architecture power modules use AArch64 PSCI or Q35 power control.
- `state`: global kernel state records.

## Kernel Core Library

`kernel/core` factors the model and logic that can be tested outside the bare-metal kernel:

- `bootfs`: BootFS records and lookup.
- `cpu_features`: CPU feature modeling.
- `ipc`: channel and capability model.
- `kernel_services`: FIDL-facing service handlers for debug, handle, IPC, memory, power, scheduler, system, task, time, and VMAR operations, including synchronous channel call/reply tokens, profile handles, structured resource groups, and GPU reservation accounting.
- `loader`: compatibility reexports and package-provider types; `//lib/elf` owns shared ELF validation and loading plans.
- `memory`: frame allocation and memory accounting.
- `mmu`: address-space and mapping model.
- `nvme` and `pci`: device-facing model pieces used by drivers/tests.
- `psci`: host-testable PSCI function IDs, version/feature decoding, signed
  return-code decoding, and power-operation mapping.
- `runtime`: process, VM space, VMO, mapping, thread, futex, interrupt,
  profile, resource group, IOMMU domain, DMA mapping token, snapshot,
  incremental runtime, handover, and memory records.
- `sched`: scheduler and resource-group model, including per-CPU ownership,
  EDF/fair accounting, quota windows, and hierarchical limit metadata.
- `transplant`: migration codec and handoff state.

Service retirement removes source scheduling, capabilities, and mappings during
the ownership transaction. Private pages remain kernel-owned in zero-reference
VMOs until maintenance scrubs and releases them, at most four pages per batch.
The scheduler arms a maintenance deadline while work remains; a later handover's
quiesced phase pauses reclamation. Runtime snapshot version 14 preserves these
pending VMOs and partial progress across full and incremental kernel handovers.
Allocator release always follows scrubbing. QEMU service-update tests require
both the bounded cutover and subsequent complete memory reclamation.

## Implemented Kernel Concepts

The current implementation models:

- universal capabilities with object IDs and rights;
- channel creation and message transfer;
- VMO and VMAR mapping operations, including root-VMAR construction handles,
  nested sub-VMAR creation, VMAR-relative mapping/unmapping, recursive
  non-root VMAR destruction, and compatibility flat mapping calls;
- lazy anonymous VMOs backed by a kernel zero page until first write, with
  single-page write-fault commitment, kernel-copy commitment, contiguous
  materialization for DMA pins, and logical resource-group reservation separate
  from committed physical-frame accounting;
- a per-process root VMAR and read/write heap VMAR reservation, with heap pages
  backed by anonymous VMOs only when userspace maps allocator chunks;
- AArch64 page-granular kernel identity mappings: text RX, rodata read-only/NX,
  writable data/BSS/stacks/page tables/heap/RAM RW/NX, Device/NX MMIO, WXN, PAN
  when supported, and unmapped kernel-stack guard pages;
- AArch64 hardening feature state that records detected, compiled, entropy-ready,
  and active BTI/PAC/speculation controls separately; CPU0 initializes the
  policy, secondary CPUs replay it, PAC uses kernel key B and per-process
  userspace key A when entropy is available, and address-space switches load
  the selected userspace key;
- process and address-space creation through privileged system services;
- resource groups with CPU shares, structured CPU/memory/GPU limits, anonymous
  VMO high-watermark enforcement, and GPU reservation release on handle close;
- monotonic/boottime/realtime clock reads, a read-only map-capable
  `TimePageV1` VMO for libc fast-time reads, bounded realtime slew accounting,
  and time-page/clock transform preservation in snapshot and incremental
  heart-transplant state;
- thread creation, futex wait/wake, wait-many, profile assignment, CPU affinity, and yield;
- physical VMO creation, legacy pinning, and domain-backed DMA mapping for D1
  drivers;
- kernel process/debug records;
- kernel update state for platform update requests;
- migration handover and preserved state records, including versioned VMO
  backing-kind/page-state data and preserved userspace PAC key material.
- privileged kernel trace-control attach/detach FIDL plus tracepoints for
  syscall dispatch, scheduler decisions, channel IPC flows, IRQ handling, and
  sync-fault/page-fault style exception handling.

## Current Boot Responsibility

The kernel remains responsible for:

- early serial logging;
- initial memory and MMU setup;
- interrupt/timer setup;
- installing kernel service capabilities;
- resolving boot handoff data from `x0` or the QEMU fixed descriptor address;
- validating RAM, BootFS, boot evidence, update/snapshot, and CPU topology
  ranges;
- probing QEMU PSCI through SMC, starting secondary AArch64 CPUs with
  `CPU_ON`, and using PSCI `CPU_SUSPEND`, `SYSTEM_RESET`, and `SYSTEM_OFF` for
  the current QEMU AArch64 power primitive surface;
- verifying the dev-root signed boot-evidence record and BootFS SHA-256 before
  loading BootFS from the external handoff range;
- launching the BootFS-provided appd EL0 bootstrap;
- acting as the authority for low-level handles, process creation, mappings, and migration handover.

Longer-term policy is deliberately kept in appd where practical.
## Restricted execution

RFC-0070 Phase 1 adds per-thread restricted execution on AArch64 and x86_64;
Phase 2 uses it for the trusted single-task Starnix runner.
Normal host and restricted guest contexts, guest SIMD/FPU state, architecture
TLS, the retained one-page state VMO, callback vector, active state, and pending
kick are kernel-owned thread state and are included in full and incremental
heart-transplant snapshots. Restricted `svc`/`syscall` instructions are never
dispatched as BexOS native syscalls. Unresolved user exceptions are reflected;
timer/device interrupts and resolvable lazy anonymous writes stay in the
kernel. See [RFC-0070 current state](rfcs/0070/CURRENT.md) for the ABI and the
explicit non-Linux scope.
The runner maps validated static Linux ELF segments with the normal
`VirtualMemory.MapInVmSpace` exact-address path and resumes transplant
candidates from versioned userspace mapping/register records. No Linux syscall
implementation or Linux object is added to the kernel.
