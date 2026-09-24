# Kernel FIDL Services

BexOS exposes the first kernel control surface through generated BexOS FIDL in
`idl/bexos/kernel`.

## IDL Surface

The library is split across files and compiled as one Bazel target:

- `types.fidl`: `Rights`, `Signals`, and `Status`.
- `ipc.fidl`: `ChannelControl`.
- `memory.fidl`: `VirtualMemory`, `VmoFlags`, and `VmarFlags`.
- `task.fidl`: `TaskControl`.
- `system.fidl`: `SystemPrivileged`.
- `debug.fidl`: `KernelDebugControl`.
- `restricted.fidl`: `Restricted` bind, enter, unbind, and asynchronous kick.

`types.fidl` also defines `SystemPowerState` for the privileged power handoff.

`//idl:kernel_fidl_rust` generates the no-std Rust bindings. Generated files
are Bazel outputs and are not committed.

## Runtime Shape

`kernel/core::kernel_services` owns the allocation-backed host-testable runtime:

- Channel endpoints are paired handle records with read/write/transfer/signal
  rights, dynamically growing queues, byte payloads, and transferred handles.
- VMOs are page-aligned records backed by dynamically sized frame metadata.
  Anonymous VMOs consume frames, releasing VMOs recycles those frames, and CoW
  clones share reference-counted pages until a VM backend resolves a write
  fault.
- VMARs are owned by VM-space records. Each process gets a root VMAR, child
  VMARs attenuate mapping permissions, and recursive non-root VMAR teardown
  removes mappings in the destroyed subtree.
- Mappings are owned by VMAR records, reject overlaps within the same address
  space, and deny WRITE+EXECUTE requests to preserve W^X.
- Tasks include process-owned thread records, saved EL0 context fields, CPU
  affinity masks, futex wait/wake state, `WaitMany` signal scanning, and
  scheduler state.
- Privileged system records cover process containers, VM spaces, resource
  groups, interrupt bindings, and checkpoint byte-count metadata.

`VirtualMemory` exposes first-class VMAR calls:

- `CreateSubVmar(parent_vmar, offset, size_bytes, flags)` allocates a nested
  virtual address region and returns a VMAR handle plus its absolute base.
- `MapVmo(vmar, vmo, vmo_offset, vmar_offset, size_bytes, flags)` maps a VMO
  range into the target VMAR. Non-zero `vmar_offset` requires
  `CAN_MAP_SPECIFIC`, and VMAR flags cannot exceed the parent VMAR's mapping
  permissions.
- `UnmapVmar(vmar, vaddr, size_bytes)` removes a mapping inside that VMAR
  subtree.
- `DestroyVmar(vmar)` recursively destroys a non-root VMAR and removes its
  mappings. Root VMAR destruction is rejected.

`VirtualMemory` keeps the current-process `Map` and `Unmap` calls and also
exposes explicit address-space variants as compatibility wrappers over each VM
space's root VMAR:

- `MapInVmSpace(vm_space, vmo, vmo_offset, size_bytes, target_vaddr,
  requested_rights)`
- `UnmapInVmSpace(vm_space, vaddr, size_bytes)`

`SystemPrivileged` exposes `StartThreadInProcess(process_handle,
address_space_handle, entry_vaddr, stack_top_vaddr, arg_handle)` so appd
can start threads inside a specific process and VM space.

`SystemPrivileged` also owns resource-group management:

- `CreateResourceGroup(name, cpu_shares, memory_limit_pages)` creates a flat
  group, installs its scheduler CPU shares, and returns both the numeric group id
  used by `CreateProcess` and a `RESOURCE_GROUP` handle used for management.
- `SetResourceGroupLimits(group_handle, cpu_shares, memory_limit_pages)` updates
  mutable limits. CPU shares must be non-zero; memory pages are recorded for
  policy/debug and are not enforced yet.
- `GetResourceGroup(group_handle)` returns the current id, name, shares, and
  memory page limit.

`SystemPrivileged.RequestSystemPowerState(state)` is the privileged kernel
handoff for whole-system power transitions. `SUSPEND_TO_RAM`, `REBOOT`, and
`POWEROFF` are valid requests; `SUSPEND_TO_DISK` returns `ErrInvalidArgs` until
hibernate image/resume storage policy exists. The host-testable control plane
records accepted requests. The current QEMU raw-kernel runtime backs the same
FIDL method with QEMU's SMC PSCI service: `SUSPEND_TO_RAM` enters PSCI
`CPU_SUSPEND` standby until an interrupt, while reboot and poweroff invoke PSCI
`SYSTEM_RESET` and `SYSTEM_OFF`. `SYSTEM_SUSPEND` is not part of QEMU's fake
PSCI implementation, so it is not used for the current product.

The appd runner path uses these kernel calls for process launch:

- `SystemPrivileged.CreateProcess` creates the isolated process container and
  VM-space handle in the resolved resource group.
- `VirtualMemory.MapInVmSpace` maps package-provided executable file VMOs,
  page-aligned anonymous zero-fill VMOs, and stack VMOs into that address
  space.
- `ChannelControl.CreateChannel` creates the initial service-manager channel;
  appd keeps one endpoint and passes the child endpoint to the process.
- `SystemPrivileged.StartThreadInProcess` starts the main thread at the
  validated runner entrypoint with the stack top and optional argument handle.

App-service creates package-declared resource groups before launching processes.
Processes can name either a package-declared group or one of the built-ins
(`system`, `foreground`, `background`, `driver`). Unknown names fail launch
instead of silently falling back.

The generated Rust FIDL bindings currently encode `handle:OPTIONAL` as a raw
handle field. Kernel code treats raw handle `0` as `None`.

The kernel binary keeps a global control plane and dispatches generated FIDL
calls through `svc #1`.

`Restricted` is raw syscall protocol ID `14`. Its one-page, retained state VMO
and per-thread host/guest contexts implement RFC-0070 Phase 1 on AArch64 and
x86_64. A successful `Enter` replaces the syscall return frame; raw guest
`svc`/`syscall` traps return through the registered non-returning vector. Kicks
are pending for inactive threads and send a reschedule interrupt to active
remote-CPU targets. Versioned full and incremental kernel snapshots include
the binding, contexts, TLS, VMO reference, active state, and pending kick.

`KernelDebugControl.ListProcesses` exposes bounded process debug records for
`debugd` and host tests. In the bare-metal syscall table it is protocol id `6`;
QEMU `debugd` uses it to populate the first `bexctl ps` response.

## SVC ABI

`svc #0` remains the linked `appd` readiness signal. `svc #1` dispatches a
single generated FIDL call:

```text
x0  protocol id: 1 ChannelControl, 2 VirtualMemory, 3 TaskControl, 4 SystemPrivileged,
                 5 ObjectControl, 6 KernelDebugControl, 8 Clock, 14 Restricted
x1  method ordinal
x2  request bytes pointer
x3  request bytes length
x4  request handle array pointer, as u64 handles
x5  request handle count
x6  response bytes pointer
x7  response byte capacity
x8  response handle array pointer, as u64 handles
x9  response handle capacity
```

Return registers:

```text
x0  dispatch Status value
x1  response byte length
x2  response handle count
```

## App-Service Publication

`services/appd::publish_kernel_services` registers public singleton
interfaces for `ChannelControl`, `VirtualMemory`, and `TaskControl`.
`SystemPrivileged` is also a singleton. App-service binds it through generated
method-capability metadata: process, resource-group, interrupt, checkpoint, and
power methods require `BEXOS_SYSTEM_PRIVILEGED`, while realtime wall-clock
adjustment requires `SET_TIME`.

Power management adds `//idl:power_fidl_rust` as a separate userspace-facing
library. The kernel handoff remains in `bexos.kernel.SystemPrivileged` so only
privileged services can request final hardware transitions.

## Current Limits

The control-plane records, channel queues, payload buffers, transferred-handle
lists, VMO page lists, and scheduler records are allocation-backed and return
`NoMemory` on allocation failure. The bare-metal kernel initializes its heap
from linker-provided free RAM before it creates dynamic service state. The
`ReadMessage` syscall path encodes responses from dynamic scratch storage, so
the old fixed staging arrays are no longer part of the EL0 dispatch path.

The shared ELF/package loader API exists in `kernel/core::loader`. App-service
uses it for ELF64/AArch64 validation, and the kernel can resolve immutable
embedded package entries through `EmbeddedPackageProvider`. Bazel builds the
standalone `appd` ELF and `appd.bootpkg` package artifact.

The boot kernel still installs EL0 mappings for the linked `appd` stub. Dynamic
page-table installation from the package/appd runner path and lower-EL
CoW fault resolution remain integration follow-ups.
Persistent virtio/block/filesystem package storage and signature-chain
verification are not implemented; the initial package provider is embedded and
immutable. The supported QEMU smoke topology is configured by
`device.prototxt`; the current board uses four CPUs, and secondary CPUs enter
the interrupt-capable scheduler idle path after CPU0 releases global init.
