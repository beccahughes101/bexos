# RFC 0016: D1 PCI and UART migration

- Created: 2026-08-27T10:28:38-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

PCI enumeration and serial-device behavior move into D1 drivers. The kernel retains only the hardware primitives and early-boot and panic diagnostics needed before userspace is available.

## Design overview

PCI root-bus enumeration and full serial-device behavior live in D1 userspace
drivers. The kernel keeps only the minimum PL011 polling path needed for early
boot logs and panic output.

## Implemented Targets

- `//drivers/d1/bus/generic/pci:pci_root_bus`
- `//drivers/d1/serial/arm/pl011:pl011`
- `//idl:hardware_manager_fidl_rust`
- `//idl:serial_fidl_rust`

Both root D1 driver packages are listed as Wave 0 early drivers in
`device/virtual/qemu/base/aarch64/bootfs_manifest.prototxt`. Hardware child drivers can use
manifest bind rules and start on demand after a matching device node is
registered.

## PCI Root Bus

The D1 PCI root-bus driver owns config-space enumeration for the first
implementation slice. On QEMU AArch64 `virt` and x86_64 Q35, it scans bus 0 through ECAM,
skips invalid vendor IDs, decodes class/subclass/prog-if, sizes BAR0, assigns
architecture-selected PCI MMIO window addresses, enables memory-space/bus-master bits, and emits
generic device-node records suitable for `DeviceRegistry.RegisterDeviceNode`.
`appd` indexes installed driver manifests and binds matching children,
such as the NVMe D1 driver, from those generic properties.

The kernel no longer runs a normal-boot PCI scanner or NVMe hardware smoke
driver. Its role is to mint raw ECAM/MMIO VMOs and interrupt capabilities for
authorized D1 drivers as the runtime transport is completed.

## UART

UART uses a two-tier model:

- `kernel/src/arch/aarch64/early_uart` provides the synchronous PL011 polling
  sink. `kernel/src/arch/x86_64/early_uart.rs` provides COM1 logging on Q35.
- `//drivers/d1/serial/arm/pl011:pl011` contains the full userspace PL011 driver
  library surface for initialization and poll-mode byte I/O, with the
  `bexos.hardware.serial.Device` FIDL protocol defining the external serial
  device contract.
- Q35 uses `//drivers/d1/serial/virtio/console` for userspace serial and debug
  transport, keeping COM1 diagnostics separate from the `debug0` port.

Future work can add interrupt-driven RX/TX rings, line discipline, baud
switching beyond the QEMU defaults, and runtime IRQ handoff.

## Device Registry

`appd` now has an in-memory device registry model for registered device
nodes. It supports duplicate detection, unregister, property lookup, optional
MMIO/IRQ resources in the runtime model, lifecycle state tracking
(`Unbound`, `Binding`, `Active`, `Quiescing`, `Suspended`, and bind failure),
and replacement of the node set for snapshot/restore-safe state handling.

## Division of Responsibilities

| Subsystem | Kernel Ring 0 Role | D1 Userspace Role |
| --- | --- | --- |
| PCI Root (`pci_root_bus`) | Provides raw physical MMIO/ECAM VMOs and MSI/MSI-X interrupt allocation handles. | Walks PCIe config space, parses BARs, assigns memory windows, and registers discovered devices with `appd`. |
| UART (`uart:pl011`) | Synchronous diagnostic output serialized with userspace logging. | Owns PL011 device behavior and exposes the serial byte-stream FIDL service; Q35 uses virtio-console for userspace serial. |
