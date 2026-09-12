# BexOS Current State

This directory documents the implementation that exists in this repository today. The longer-range architecture lives in [the RFCs](rfcs/README.md), with a `CURRENT.md` implementation and gaps snapshot beside each RFC; this directory is the shorter, factual map of the current code, build graph, boot image, interfaces, services, drivers, and test surfaces.

## System Snapshot

BexOS is currently an experimental Rust OS built with Bazel. The implemented system targets QEMU AArch64 and is organized around:

- a no-std AArch64 kernel in `//kernel`;
- a host-testable kernel core library in `//kernel/core`;
- generated Rust bindings from BexOS FIDL definitions in `//idl`;
- userspace ELF components packaged from prototxt app manifests;
- appd broker, lifecycle, namespace, driver wave, and migration logic;
- appd AppManager and domain-association cache state;
- D1 native drivers for PCI, PL011 UART, NVMe, BexFS, archivefs, MemFS, disk images, VirtIO-Net, and deterministic I2C/SPI controller services;
- core services for VFS, debug, tracing, updates, trust, TEE access, power, users, keychain, fonts, networking, time, jobs, and storage verification;
- Trusty secure-world firmware with KeyMint, Gatekeeper, storage, AVB, AuthMgr
  FE/BE, the retained BexOS orchestrator, and a pluggable external TEE driver
  ABI loaded by `teed`;
- QEMU nongui/workstation product assembly under `//device/virtual/qemu`, sharing `base` configuration.

The current boot product is `virtual_aarch64`. Its product assembly places
boot-critical platform services and D1 storage drivers in BootFS, and puts
`storage_verify`, `bexos.lib.crypto`, `bexos.lib.net`,
`bexos.driver.network.virtio_net`, `bexos.service.netstackd`,
`bexos.service.timed`, and `bexos.service.jobd` into the system
image/autoinstall set. The storage-image packages are preinstalled on the
`STORAGE` package partition and activated after appd pivots from BootFS.

## Current Documentation Map

- [CLI Reference](cli.md): complete bexctl command syntax, output modes, login, and terminal behavior.

- [Architecture](architecture.md): runtime layout, boot flow, ownership boundaries, and current gaps.
- [Build, Assembly, And Tooling](build-assembly-tooling.md): Bazel rules, code generation, image assembly, and developer commands.
- [IDL And ABI](idl-and-abi.md): FIDL/protobuf contracts and generated Rust surfaces.
- [Kernel](kernel.md): `//kernel` and `//kernel/core` responsibilities.
- [Appd And Userspace](appd-userspace.md): manifest handling, broker, lifecycle, runners, namespace, config, waves, and migration.
- [WASM Runtime](wasm-runtime.md): implementation status, guest interfaces, limits, lifecycle, and verification.
- [System and User UI](sysui.md): package selection, authentication, desktop windows, isolation, and recovery.
- [Dioxus WASM native UI](dioxus.md): Rust WASI component apps, shared UI component packaging, native GPU/CPU rendering, input, and transplant behavior.
- [Local Fonts](fonts.md): `fontd`, system/user tiers, read-only shared VMOs, matching, shaping clients, and the disabled dynamic-fetch boundary.
- [Drivers And Storage](drivers-storage.md): D1 drivers, block/filesystem stack, Linux shim, and D2 smoke target.
- [I2C And SPI Services](i2c_spi.md): scoped D1 I2C/SPI controllers, deterministic backend, topology, migration, and validation state.
- [Services](services.md): appd, jobd, debugd, traced, vfsd, usersd, keychaind, trustd, netstackd, timed, updated, teed, powerd, i2cd, spid, and storage_verify.
- [Storage Preinstalled Apps](storage-preinstalled-apps.md): QEMU storage package checklist and launch rules.
- [Secure Runtime And Updates](secure-runtime-updates.md): Trusty integration,
  external TEE driver selection, trusted-app packages, update engine,
  replacement archives, and heart transplant state.
- [x86_64 Support](x86_64-support.md): architecture selection, Q35 handoff, standalone Trusty, and verification.
- [QEMU Product](qemu-product.md): board configuration, boot image contents, startup waves, and debug flow.
- [Tracing](tracing.md): `traced`, trace categories, debugd collection, host client/e2e helpers, instrumentation macros, and current limits.
- [Testing Status](testing-status.md): major Bazel test targets and current verification coverage.
- [GitHub Actions CI](ci.md): Linux runner setup, firmware preparation, build/test matrices, and diagnostics.

## Important Repo Conventions

- Use Bazel for builds, tests, image generation, FIDL generation, proto encoding, and app packaging.
- App manifests and platform configuration are prototxt sources compiled by Bazel.
- Generated files are build artifacts and should not be committed.
- Rust formatting is run through `bazel run @rules_rust//:rustfmt`.
- `docs/rfcs/*/README.md` describes long-term design. Update the relevant guide in `docs` and RFC `CURRENT.md` when behavior changes; preserve future-facing RFC design material.

## High-Level Boot Product

The QEMU product currently assembles these platform packages:

- `bexos.platform.appd`
- `bexos.driver.pci_root`
- `bexos.driver.uart.pl011`
- `bexos.driver.rtc.pl031`
- `bexos.driver.storage.nvme`
- `bexos.driver.storage.bexfs`
- `bexos.driver.storage.archivefs`
- `bexos.driver.storage.memfs`
- `bexos.driver.storage.diskimage`
- `bexos.driver.network.virtio_net`
- `bexos.service.vfsd`
- `bexos.service.teed`
- `bexos.driver.debugd`
- `bexos.service.traced`
- `bexos.service.updated`
- `bexos.service.trustd`
- `bexos.service.powerd`
- `bexos.service.usersd`
- `bexos.service.keychaind`
- `bexos.service.fontd` (workstation)
- `bexos.service.netstackd`
- `bexos.service.timed`
- `bexos.service.jobd`
- `bexos.platform.storage_verify`
- `bexos.lib.crypto`
- `bexos.lib.net`

The BootFS startup waves are declared in
`device/virtual/qemu/base/aarch64/bootfs_manifest.prototxt`; later storage-preinstalled
services carry their wave in their package manifests:

- wave 0: PCI root, PL011 UART, and PL031 RTC
- wave 1: NVMe
- wave 2: BexFS, archivefs, and MemFS
- wave 3: diskimage, vfsd, teed, debugd, traced, updated, and trustd
- wave 4: powerd and usersd
- package wave 5: keychaind, fontd and netstackd
- package wave 6: timed
- package wave 7: jobd

`appd` is the core bootstrap package.

## Known Current Gaps

This is a bring-up system, and a host-testable model or generated ABI is not
the same as a production-complete runtime path. The current implementation
boundaries are:

### Platform, Kernel, And Hardware

- **Platform coverage:** maintained virtual products include QEMU AArch64 and
  the explicit Q35 x86_64 software-TEE development product. There is no validated
  physical-board or RISC-V boot path, and integrated x86 Trusty remains unavailable.
  The raw AArch64 QEMU
  kernel boot uses QEMU's built-in PSCI service through the SMC conduit with
  `secure=off,virtualization=on,iommu=smmuv3`. It preserves the maintained four-core SMP
  topology with PSCI `CPU_ON`, maps `SUSPEND_TO_RAM` to PSCI `CPU_SUSPEND`
  standby until an interrupt, and completes reboot/poweroff through PSCI reset
  and shutdown. Q35 uses ACPI, APIC/IOAPIC, HPET and CMOS RTC, with virtio-console
  userspace serial and COM1 diagnostics. Full PSCI `SYSTEM_SUSPEND`, physical boards, and other ISAs are
  intentionally outside the current maintained platform.
- **Secure boot and isolation:** QEMU now boots with a version 3 handoff and a
  signed boot-evidence record. The kernel verifies the dev boot root signature
  and BootFS SHA-256 before appd starts; appd verifies the platform-policy and
  orchestrator measurements before launching general userspace. The QEMU policy
  enables emulated RPMB anti-rollback state, SMMUv3 exposure, and per-device
  IOMMU domains for current DMA-capable D1 drivers. Trusty/TF-A interfaces are
  present and fail closed where hardware support is required, but physical-board
  measured boot and RPMB enforcement are not yet board-validated.
- **Kernel hardening and memory:** the maintained AArch64 product now uses
  page-granular kernel mappings for kernel text RX, rodata read-only/NX,
  mutable kernel data, stacks, page tables, heap, and ordinary RAM RW/NX, plus
  Device/NX MMIO. WXN and PAN are enabled when supported, kernel stack guard
  pages are left unmapped, and native AArch64 Rust targets are built with the
  pinned `nightly/2026-07-16` toolchain and BTI/PAC branch-protection policy.
  The kernel reports detected, compiled, entropy-ready, and active BTI/PAC/
  speculation controls separately; PAC activates only with hardware support,
  compiled protection, and boot/RNDR entropy. appd constructs ELF processes
  through delegated hierarchical VMARs, per-segment image/library children,
  TLS and stack VMARs, and an unmapped stack guard. Anonymous VMOs are lazy:
  reads use the kernel zero page, first writes commit a single page, kernel
  copies commit before writing, and legacy DMA pinning materializes a contiguous
  backing without overcharging logical reservations. Device DMA-capable drivers
  use IOMMU-domain mappings instead of raw pins. The vDSO time page is
  implemented as a read-only seqlock VMO with libc fallback.
- **Scheduling and SMP:** the QEMU runtime uses the kernel scheduler as the
  source of CPU ownership, with per-CPU current tasks, affinity-aware
  selection, EDF admission, fair-quantum accounting, architecture-selected
  one-shot timers and reschedule interrupts for idle secondary CPUs. `ProfileProvider`
  and the profile, affinity, wait-many, and yield methods on `TaskControl` are
  routed through the guest syscall path. Resource groups have a `system` root
  and built-in children; manifest-defined groups are validated as a
  same-package hierarchy and created through the structured v2 limits ABI.
  Anonymous VMO allocations charge the creator's resource-group hierarchy and
  reject allocations above any high watermark. Realtime launch permission
  requires the manifest permission and a package/signer platform allowlist,
  rather than hardware-access tier. Synchronous channel call/read/reply is
  modeled with single-use reply tokens and priority donation while a server is
  handling a call. GPU limits are reservations-only enforcement at the resource-group layer;
  reservations charge ancestors atomically and are released when their capability
  is closed. The product also includes the scened compositor, VirtIO-GPU transport,
  a Vello/Venus compositor worker, and the Dioxus WASM UI runner path described
  in [scened/shared Flatland](scened.md) and [Dioxus WASM native UI](dioxus.md).

### Runners, Drivers, And Component Lifecycle

- **Runner coverage:** native AArch64 ELF and standalone WASM applications are
  supported. The WASM runtime includes WASI 0.2, child sandboxes, checkpoint
  migration, and composed component imports for Dioxus shared UI libraries; see
  [WASM Runtime](wasm-runtime.md) and [Dioxus WASM native UI](dioxus.md) for
  interfaces, limitations, and verification. The D2 WASM driver target
  remains a build smoke test. Web, Android, Nix, and MicroVM execution are not
  implemented runtime paths. The ELF dynamic
  loader supports the hermetic BexOS AArch64 ABI for manifest-declared shared
  libraries, not arbitrary Linux/glibc shared-library compatibility.
- **Namespace, config, and trace lifecycle:** startup ABI v8 carries named
  namespace entries separately from positional role resources plus optional
  component-config, linker-data, and trace-producer descriptors. Package-store
  launches receive `/pkg`, `/data`, `/tmp`, shared vault, and `/deps/...`
  entries through that vector. BootFS waves still use BootFS resolver-backed
  library loading before storage is ready; stable dependency-directory proxy
  pivoting for early BootFS services remains future work. Component config blobs are validated
  `BEXCFG` v1/v2 snapshots; v2 carries schema fingerprint and generation
  metadata. Appd exposes privileged get/set/reset config lifecycle operations,
  carries accepted overrides through appd heart transplant, and applies them to
  subsequent launches. Per-instance prepare/commit live reconfiguration remains
  future work.
- **Driver lifecycle:** native D1 driver lifecycle is implemented for the
  current PCI/NVMe/VirtIO path. Appd serves the generated
  `bexos.hardware.manager.DeviceRegistry` contract, tracks parented device
  topology, retains canonical typed hardware-resource leases, hands
  rights-reduced duplicates to matched drivers through startup ABI v8, publishes
  per-device service instances with `device.node_id` metadata, and preserves
  topology, resources, retained provider endpoints, recovery-image metadata, and
  recovery counters across appd heart transplant. PCI root registers every
  memory BAR, unregister cascades descendants before parents, D1 drivers receive
  `DriverLifecycle.PrepareStop`, and recovery retries the same candidate once
  before excluding it for fallback rebinding. Exhausted recovery marks the node
  bind-failed and closes retained routes so clients do not hang indefinitely.
  There is still no D2 driver runtime beyond the smoke target.
- **Heart transplant durability and coverage:** appd and the shipped
  long-running services/drivers carry explicit migration state, and every
  shipped `HEART_TRANSPLANT` ELF service/D1 driver now has a Bazel-built
  replacement archive guarded by `//:heart_transplant_coverage_test`. Appd
  stages verified replacements under content-addressed package-store archive
  IDs before launching candidates, commits the selected archive path, manifest
  metadata, content hash, and accepted generation in one durable registry
  transaction, and restores service floors from those records after reboot.
  BootFS/system manifests remain baselines; verified durable selections win
  after persistent storage is available. Future runner classes still require a
  registered migration adapter before appd will stage or transfer ownership.
  Netstack TCP heart transplant now checkpoints and restores smoltcp TCP tuple,
  sequence/window, timer, RTT, assembler, and bounded RX/TX buffer state for
  migrated sockets rather than reconnecting established flows. On QEMU AArch64,
  TEE updates route through the provider-neutral `TeeManager.UpdateTeeCore`
  ABI. The Trusty design uses TF-A-controlled secure A/B slots and the upstream
  `rpmb_dev` proxy over QEMU's `rpmb0` virtio-serial port, not physical rollback
  protection.

### Identity, Storage, Permissions, And Applications

- **Permissions and consent:** manifests, scoped grants, method filtering,
  separated system/per-user redb placement, encrypted nonzero-UID grant stores,
  and optional capability handles with provider rebinding are active in appd
  after the VFS handoff. Installed packages, opener/domain-association state,
  and permission declarations can be reloaded after reboot. Declared required
  and optional permissions are auto-granted when their scopes validate. The
  remaining permissions gap is trusted consent UI.
- **Packages and app lifecycle:** versioned installs, active pins, probation,
  explicit rollback, multi-version storage policies, protected-app crash-loop
  restart/rollback, and inactive archive pruning are implemented. Active pins
  are preserved as first-class checkpoint state and mirrored into the active
  SYS_STATE slot's `slot_a/slot_b/pinned_apps.redb` store, and `VersionManager`
  reports allocated archive bytes for disk-backed packages. Job workers remain
  governed by jobd's execution lifecycle. Ambiguous opener matches still return
  `PROMPT_PENDING_USER` because there is no picker UI.
- **Jobs:** jobd loads declared jobs from app manifests through appd's
  `WorkerLauncher`, clamps dynamic requests to those signed declarations, stores
  durable UID 0 jobs in `/data/system/bexos.service.jobd/jobs.redb`, stores
  durable nonzero-UID jobs in `/data/users/<uid>/jobs.redb` while the user is
  unlocked, keeps transient jobs in memory/migration state only, subscribes to
  power/network/time/user/package watcher channels, launches due workers
  automatically through appd, batches eligible flex-window work under one wake
  lease, and calls appd to terminate workers on completion, launch failure, or
  execution timeout. Nonzero-UID job stores live inside the encrypted per-user
  home images that vfsd mounts after usersd supplies the user's U-KEK.
- **Application and UI stack:** the graphical product includes a Vello CPU boot
  splash, a minimal CPU scened, and a D1 VirtIO-GPU display driver; see
  [boot UI](bootui.md) and [scened/shared Flatland](scened.md). VirtIO input,
  cached multilingual desktop text, and fenced Flatland presentations are
  implemented. There is no USB/Bluetooth input stack, desktop shell, permission
  or opener system UI, accessibility stack, browser/web runtime, or production
  app-store ecosystem in the current build product.

### Trust, Updates, Networking, Time, And Observability

- **TEE and key protection:** the canonical QEMU ARM64 product builds TF-A with
  Trusty as BL32 and includes upstream KeyMint, Gatekeeper, secure storage,
  AVB, AuthMgr FE/BE, and the retained BexOS orchestrator. `teed` loads a signed
  external ABI-v1 TEE driver selected by its prototxt manifest: Trusty for the
  product and software only for explicit emulated tests. QEMU owns the upstream
  RPMB proxy and persistent development image. Physical-board validation and
  secure ConfirmationUI remain future work.
- **Trust and distribution:** app archives are now v2-only: v1 archive magic is
  rejected, the signature footer records algorithm, signer-chain bytes, BLAKE3
  authenticated-content digest, and signature, and manifests are accepted only
  after the archive verifier succeeds. Bazel selects a BexOS ecosystem bundle
  through `//ecosystem/bexos` (defaulting to the dev profile) and embeds the
  public policy/root/TUF/update-key bundle in BootFS at
  `/system/ecosystem/bexos.bundle`; private signing material is outside that
  bundle. `trustd` exposes algorithm/tier-aware validation results, keeps
  immutable app roots separate from dynamic enterprise roots, rejects revocation
  rollback/expired payloads, enforces method filtering for mutation ordinals,
  and preserves dynamic trust state through heart transplant. `appd` records
  verified signer metadata in the app registry and direct URL installs use the
  shared strict HTTPS URL parser with best-effort well-known refresh after a
  trusted install. `updated` persists TUF client rollback state and hands
  verified app target bytes directly to appd. The checked-in prod profile is a
  deliberately non-deployable template; operational production must supply
  external public roots/endpoints/policy plus a private or HSM signer. The
  current trustd verifier now performs DER X.509 issuer-signature path
  validation for ordered leaf/intermediate/root chains, checks CA and key-usage
  constraints, enforces code-signing EKU, validates certificate time windows,
  verifies Ed25519 and ECDSA P-256 signatures, and applies certificate/SPKI/
  serial revocation across the selected path. The shared distribution client
  keeps its policy/parser core product-safe and exposes a buildable
  `//lib/distribution:distribution_live` rustls/netstack HTTPS adapter for
  callers that opt into live networking: it resolves hosts through Netstack,
  connects TCP sockets, exports TLS roots from trustd's REDB bundle, builds a
  rustls root store with hostname/SNI verification, and parses bounded HTTP/1.1
  content-length and chunked responses through the shared `//lib/net:net_secure`
  implementation. The current QEMU product can build the live adapter, but it
  still selects the core distribution client and does not enable live
  app/update fetching by default.
- **Networking:** the packet data plane is dual-stack for configured IPv4 and
  IPv6. `netstackd` accepts static IPv6, IPv6 DNS/bootstrap, and SLAAC config
  fields, installs link-local/static IPv6 addressing, uses smoltcp for TCP and
  UDP sockets, returns mixed A/AAAA resolver results, and keeps strict DoH
  fail-closed when no secure bootstrap path is available. Its migration state
  now carries the dual-stack config and restores preserved socket/listener
  handles over the retained Ethernet resources during activation.
- **Time and power policy:** timed applies initial/failed/manual corrections as
  realtime steps, slews bounded subsequent network corrections through the
  kernel realtime transform, persists target quality state, and uses the QEMU
  PL031 RTC for startup UTC bootstrap plus best-effort writeback. The kernel
  exposes a read-only seqlock time page for libc fast-path monotonic, boottime,
  and realtime reads with FIDL fallback. powerd now reports provider-backed
  battery/thermal/DVFS availability instead of fabricated measurements, applies
  generic low-battery and thermal throttling policy to optional DVFS providers
  and the built-in background resource group, and quiesces devices
  transactionally before PSCI suspend/reboot/poweroff. QEMU still has no
  synthetic battery, thermal, or DVFS hardware, and `SUSPEND_TO_DISK`
  remains explicitly unsupported.
- **Tracing:** traced manages sessions and producer registration, appd
  provisions startup ABI v8 trace VMOs, debugd proxies trace collection to
  traced, Perfetto protobuf is the default `.pftrace` export, explicit legacy
  BexOS FXT remains selectable, and the FIDL compiler supports tracing's import
  of `bexos.kernel.Status`. Kernel syscall/scheduler/IPC/IRQ/fault paths,
  appd, vfsd, netstackd, debugd, and traced have implemented tracepoints plus
  focused non-e2e Bazel coverage. The general UI stack is still absent, so
  `ui_frames` is supported by the writer/exporter but has no runtime producer.
- **Verification boundary:** coverage combines focused host protocol/lifecycle
  tests with tagged QEMU smoke/e2e tests for the pinned Trusty firmware and
  lifecycle-owned RPMB proxy. There is no physical-hardware, multi-board, production
  secure-boot/RPMB, hardware SMMU integration, or full GUI/app-stack test
  coverage.
