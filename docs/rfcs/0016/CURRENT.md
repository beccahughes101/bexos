# RFC 0016: D1 PCI and UART migration — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0016](README.md)

## Implementation summary

PCI enumeration and PL011 serial service behavior reside in D1 userspace drivers; kernel early diagnostics remain available.

## Implemented behavior

- The PCI root driver enumerates devices/BAR resources and registers typed device nodes for appd matching. The PL011 driver serves the serial FIDL interface.
- AArch64 BootFS includes both as early drivers. x86_64 uses its own console configuration rather than a PL011 device.
- Both D1 implementations include migration code and replacement archives to retain mappings, control endpoints, and device state without repeating cold initialization.

## Gaps and deviations

- The kernel still contains early boot/panic serial and platform discovery helpers; removing all hardware knowledge from ring 0 was not the implemented boundary.
- PL011 is a bounded polling bring-up path, not a full interrupt-driven TTY/line-discipline implementation.
- Host models and QEMU transplant tests do not establish physical PCI hotplug or arbitrary UART compatibility.

## Sources and validation

Implementation and contract evidence: [drivers/d1/bus/generic/pci/src](../../../drivers/d1/bus/generic/pci/src), [drivers/d1/serial/arm/pl011/src](../../../drivers/d1/serial/arm/pl011/src), [idl/bexos/hardware/serial.fidl](../../../idl/bexos/hardware/serial.fidl), [device/virtual/qemu/base/aarch64/bootfs_manifest.prototxt](../../../device/virtual/qemu/base/aarch64/bootfs_manifest.prototxt), [kernel/src/arch](../../../kernel/src/arch).

Relevant test sources and Bazel targets: [drivers/d1/bus/generic/pci/BUILD.bazel](../../../drivers/d1/bus/generic/pci/BUILD.bazel), [drivers/d1/serial/arm/pl011/BUILD.bazel](../../../drivers/d1/serial/arm/pl011/BUILD.bazel), [testing/e2e/qemu/update/BUILD.bazel](../../../testing/e2e/qemu/update/BUILD.bazel).

Detailed guides and previously recorded validation: [drivers storage](../../drivers-storage.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
