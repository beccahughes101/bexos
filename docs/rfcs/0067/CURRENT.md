# RFC 0067 — driver framework: current state

- Reviewed: 2026-09-23
- Design: [RFC 0067](README.md)

## Selected architecture

The implemented coordinator is `appd`. A driver host is the driver's native ELF
process; BexOS does not run a separate `driver_manager` service and does not load
arbitrary shared objects into a generic `devhost`. The design in the RFC README
is retained as the long-term design.

Driver manifests are prototxt. Drivers default to `ISOLATED`, one process and
one IOMMU domain per node. `COLOCATED` is bounded by
`max_instances_per_host`. `HOST_SHARED` is restricted to an active child whose
parent is served by the same process and signed package. Every driver process
must use the `HEART_TRANSPLANT` lifecycle. Drivers declaring ambient network or
socket access, missing bind rules, or missing required hardware resources are
rejected.

## Implemented source behavior

- The generic appd device coordinator accepts authenticated PCI, USB, I2C and
  SPI registrars; persists canonical topological paths, parent/child ownership,
  hardware leases, host membership, recovery and acquisition state; binds new
  nodes through the same path as boot devices; and removes subtrees post-order.
- `bexos.hardware.manager` preserves existing DeviceRegistry ordinals and adds
  topological identity, a private `DriverHostController`, and `PciDeviceControl`.
  PCI root supplies BARs, distinct per-BDF IOMMU domains, interrupt objects and
  private bus-control channels. Bus mastering begins disabled and appd enables
  it only after driver readiness; removal and recovery disable mastering and
  reset before releasing a host.
- Kernel interrupt objects have appended acknowledge and mask methods. Delivery
  is one-shot until acknowledge and the object model enforces a configurable
  per-vector flood limit whose default is 100,000 events per second. Existing
  method ordinals are unchanged.
- Startup ABI v11 appends driver-host controller and recovery handles. A frozen
  v10 wire decoder remains, and v6-v10 decoding retains its original layout.
- `lib/driver_runtime` implements bind-context validation, resource ownership,
  isolation/colocation/host-sharing authorization, atomic group transitions and
  versioned migration records. `lib/device_dataplane` implements versioned
  Ethernet/block journals that retain provider FIFO endpoints, VMOs, buffer IDs
  and started state without replacing client-visible handles.
- Appd retains provider endpoints in its route table. Recovery quiesces hardware,
  retries the same installed image once, falls back through ranked candidates,
  clears exclusions after a stable interval, replays retained routes, and closes
  exhausted endpoints. Coordinator and route state use backward-compatible
  checkpoint decoders.
- Netstack consumes up to 16 Ethernet grants keyed by device node ID, chooses the
  lowest node ID as the deterministic primary, polls every link, and checkpoints
  every link independently.
- VirtIO-Net and NVMe retain their existing direct FIFO/VMO data paths and
  heart-transplant adapters. The shared appd data-plane journal and private
  backend FIDL contracts exist; full setup-proxy adoption by both existing
  drivers is not yet complete, so universal crash continuity is not claimed.
- Intel support is split into shared descriptor/register/DMA logic and isolated
  e1000e/igb packages. e1000e IDs are `10d3`, `1539`, and `15b8`; igb IDs are
  `10c9` and `1521`. The shared model covers MAC discovery, bounded RX/TX rings,
  malformed descriptors, wraparound, DMA bounds, reset/quiesce, interrupt
  masking and versioned controller migration. The ELF runtime retains MMIO and
  hardware capabilities across transplant.
- x86 exports validated ACPI MCFG segment/bus information and the PCI MMIO
  aperture. AArch64 exports the board PCI/DT-equivalent aperture through the same
  architecture interface. PCI enumeration no longer chooses a QEMU-only bus
  range at runtime.
- Pkgd prototxt mappings carry bounded PCI selectors and priority. Resolution is
  exact BDF, vendor/device, then class fallback. Appd queues one bounded request
  per waiting node, checkpoints it, verifies the downloaded driver manifest
  against the canonical identity, installs through the signed archive path and
  re-enters normal binding. Offline/no-mapping devices remain unbound.

## Validation evidence

Final-tree Bazel evidence is recorded here only after commands complete. The
implementation batch is intended to cover appd, startup compatibility, kernel
core, PCI, VirtIO-Net, Intel common/runtime/packages, netstack, pkgd mapping,
driver runtime and data-plane journals on both supported guest architectures.

At this documentation edit point, no final-tree Bazel run or dual-architecture
QEMU driver-framework acceptance run has completed. Consequently there is no
measured recovery interruption for this revision, and the RFC's sub-50 ms target
is **not claimed**.

## Remaining acceptance and hardware limitations

- The dedicated dual-architecture QEMU scenario still needs to demonstrate
  concurrent VirtIO-Net/e1000e/igb, signed on-demand acquisition, hotplug and
  unplug, multi-function isolation, planned transplant, forced crash, retained
  endpoints/VMOs, and uninterrupted traffic through an unaffected NIC.
- The shared data-plane journal must be wired into every VirtIO-Net, NVMe and
  appd setup path before crash reconstruction can be claimed uniformly.
- Kernel interrupt object semantics are implemented, but end-to-end IOAPIC/GIC
  vector acknowledgement, PCI MSI/MSI-X programming, interrupt flooding and
  physical Intel RX/TX remain hardware-validation items.
- AArch64 exposes the maintained board PCI window; parsing arbitrary third-party
  device trees and validating physical SMMU handoff remain future work.
- Recovery interruption must be measured in the acceptance guest. The result,
  including any value above 50 ms, must replace the unmeasured statement above.
