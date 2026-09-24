# Current Architecture

## Runtime Layers

BexOS currently has these implementation layers:

- **IDL and schemas** in `idl/`, including kernel FIDL, app lifecycle/debug/migration FIDL, hardware/storage/vfs/power FIDL, app manifest proto, and platform assembly/config/device protos.
- **Kernel implementation** in `kernel/`, built as no-std AArch64 binaries for the main kernel and update kernel.
- **Kernel core model** in `kernel/core/`, a no-std Rust library used by the kernel and host tests for memory, IPC, scheduler, runtime records, kernel service handling, PCI/NVMe models, BootFS, and transplant handoff data. The model includes profile handles, synchronous channel call/reply tokens, resource-group CPU/memory/GPU accounting, and SMP scheduler ownership rules.
- **Userspace support libraries** in `lib/userspace` and `lib/bexos_libc`, used by userspace binaries for startup, allocator, boot records, migration, generated IDL interactions, and the Rust `std` POSIX/libc shim. The libc shim routes kernel operations through generated FIDL over the userspace syscall transport, including VMAR-backed heap growth, clock, socket, ProfileProvider, and TaskControl compatibility entrypoints.
- **Appd** in `services/appd`, a std-linked service ELF that acts as the
  init-style platform service, manifest registry, service broker, driver
  startup/lifecycle coordinator, live device registry, runner policy gate,
  namespace/config provider, debug/lifecycle surface, and migration participant.
- **D1 native drivers** in `drivers/d1/<type>/<vendor>/<device>`, currently covering PCI root, VirtIO-Net, Intel e1000e/igb, PL011 UART, NVMe block, BexFS, archivefs, diskimage, and a Linux shim support crate. Appd is the driver manager and launches isolated native ELF processes; there is no generic shared-object-loading devhost. Hardware driver manifests must declare heart-transplant lifecycle support. Shared driver mechanics live in `lib/driver_runtime` and direct-plane recovery journals in `lib/device_dataplane`.
- **Core services** in `services/`, currently debugd, vfsd, updated, powerd,
  usersd, keychain, fontd, timed, networkd, multi-instance netstackd, vswitchd,
  jobd, teed, appd, and storage_verify. Service ELFs are std-linked through the
  BexOS libc shim.
- **Secure-side Trusty integration** in `third_party/trusty`, `secure/`, and
  `lib/tee_driver_*`, including the pinned QEMU Trusty/TF-A firmware build, the
  upstream KeyMint, Gatekeeper, storage, AVB, AuthMgr FE/BE, the retained BexOS
  orchestrator, and the external ABI-v1 TEE driver packages loaded by `teed`.
  Secure ConfirmationUI remains future work.
- **Host tools** in `tools/`, including FIDL code generation, app archive creation, product assembly, disk/BootFS image generation, QEMU running, and `bexctl`.

## Current Boot Shape

The QEMU products are `//device/virtual/qemu/nongui` and
`//device/virtual/qemu/workstation`, with shared board configurations under
`//device/virtual/qemu/base/{aarch64,x86_64}`. Bazel builds:

- `//kernel:kernel`;
- `//device/virtual/qemu/nongui:bootfs.img`;
- `//device/virtual/qemu/nongui:boot_handoff.bin`;
- `//device/virtual/qemu/nongui:qemu_nvme_gpt.img`;
- platform/product/config binary protos;
- package manifests for all boot components;
- std-linked userspace ELF binaries for platform services plus NVMe, BexFS, archivefs, and diskimage, with no-std userspace ELF binaries retained for lower-level D1 drivers.

The kernel receives a BootFS and handoff records, starts `appd`, reserves a per-process heap VMAR, and the appd startup planner launches wave-ordered platform components. D1 drivers and services are regular package-manifest components with ELF runner options, service flags, waves, lifecycle settings, and optional exposed/consumed service metadata.

## Authority Boundaries

The current code models these authority boundaries:

- Kernel capabilities are represented with typed object handles, rights, signals, and kernel-service protocols.
- Appd owns app/package manifests, permission checks, published interface
  visibility, service binding, worker launch, runner policy, driver matching,
  namespace/config delivery, canonical D1 hardware-resource leases, parented
  device topology, retained driver provider endpoints, and crash-recovery
  budgets across its own heart transplant state.
- Appd grants realtime scheduling only when a prototxt manifest permission and
  the platform prototxt package/signer allowlist agree.
- `jobd` owns condition-aware background job scheduling, durable/transient job
  state routing, provider watcher subscriptions, and execution-deadline
  enforcement while delegating declared worker process creation and termination
  back to appd's privileged worker launcher.
- Kernel service access for privileged operations is represented by `bexos.kernel.SystemPrivileged` and the `BEXOS_SYSTEM_PRIVILEGED` permission.
- Platform policy controls native ELF runner access and driver allowlisting.
- Device drivers expose per-device service instances and can consume system
  services through manifest-declared service links.
- Migration uses explicit control protocols and generation numbers instead of implicit package-name authority.
- `fontd` owns font validation, per-user visibility, deterministic matching, and canonical read-only VMO distribution. Consumers own only reconstructible mapped/shaping caches.
- Appd assigns each application a named network domain and launches isolated
  networkd/netstackd instances. Networkd owns egress/split-DNS policy,
  netstackd enforces table scope again, and vswitchd is the sole physical NIC
  consumer in the normal product.

## Configuration Model

Configuration is prototxt-first:

- App manifests use `idl/bexos/app/manifest.proto`.
- Platform product definitions use `idl/bexos/platform/assembly.proto`.
- Platform policy uses `idl/bexos/platform/config.proto`.
- Board/device records use `idl/bexos/platform/device.proto`.

The QEMU product compiles these prototxt inputs through Bazel genrules and Starlark macros, then includes the resulting binary protos in BootFS or product assembly outputs.

## Current Implementation Limits

The architecture should be read as a bring-up system, not a production OS. The
current tree has host-testable models and QEMU-targeted binaries. QEMU secure
boot evidence, policy/orchestrator measurement checks, emulated antirollback,
and per-device DMA domains for current DMA-capable D1 drivers are enforced in
the maintained product. Production-grade physical secure boot/RPMB validation,
package signing infrastructure, compositor/app UX, full storage persistence
policy, and complete live replacement across all component classes still need
more work.

## Graphical boot UI

The workstation's D1 VirtIO-GPU service owns scanout and retained DMA resources.
`splashd` renders the early boot UI through Vello CPU; `scened` takes exclusive
presentation ownership after its first completed frame matches the splash.
The kernel only reserves and delegates optional firmware framebuffer memory.
Appd remains the bootstrap coordinator and reports real storage/readiness events.
Shared graphics geometry, rendering, scene transactions, and ownership logic are
separate from userspace transport and migration. See [boot UI](bootui.md) for
interfaces, validation, and the boundary with the full desktop design.
