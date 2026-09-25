# Drivers And Storage

For the supported standalone SDK workflow, including D1 bind manifests,
structured resource startup, signing, product grants, and replacement testing,
see [Out-of-Tree D1 Drivers](dev/drivers.md). This document inventories the
drivers and storage stack implemented inside the BexOS repository.

## Driver Tiers In The Current Tree

The current repository has:

- D0-style in-kernel early hardware support for boot-critical paths such as early UART and interrupt/timer setup.
- D1 native Rust drivers under `drivers/d1/<type>/<vendor>/<device>`.
- A D2 WASM smoke target under `drivers/d2/testing/bexos/smoke`.

The long-term D2 runtime is still future work; the current D2 target validates the build platform shape.

## D1 PCI Root

Package: `bexos.driver.pci_root`

Targets:

- `//drivers/d1/bus/generic/pci:pci`
- `//drivers/d1/bus/generic/pci:pci_root_bus`
- `//drivers/d1/bus/generic/pci:pci_root_bus_tests`
- `//drivers/d1/bus/generic/pci:replacement_archive`

The PCI root driver is wave 0 and exposes multi-instance
`bexos.power.DevicePowerControl`. It consumes appd's privileged
`bexos.hardware.manager.DeviceRegistry` service during boot, enumerates QEMU PCI
bus 0, and registers each discovered device with explicit parent topology and a
typed MMIO resource for every memory BAR rather than only BAR0. It depends on
`//kernel/core` for PCI model support.

## D1 PL011 UART

Package: `bexos.driver.uart.pl011`

Targets:

- `//drivers/d1/serial/arm/pl011:uart`
- `//drivers/d1/serial/arm/pl011:pl011`
- `//drivers/d1/serial/arm/pl011:pl011_tests`
- `//drivers/d1/serial/arm/pl011:replacement_archive`

The PL011 driver is wave 0 and exposes multi-instance `bexos.power.DevicePowerControl`. The kernel keeps only early UART behavior; PL011 is otherwise a userspace D1 component.

## D1 PL031 RTC

Package: `bexos.driver.rtc.pl031`

Targets:

- `//drivers/d1/rtc/arm/pl031:rtc`
- `//drivers/d1/rtc/arm/pl031:pl031`
- `//drivers/d1/rtc/arm/pl031:pl031_tests`
- `//drivers/d1/rtc/arm/pl031:replacement_archive`

The PL031 driver is wave 0, maps only QEMU's `0x09010000` RTC MMIO page, and
exposes singleton `bexos.time.RtcHardware` for UTC-nanosecond reads/writes.
It rejects dates before 2020 and values outside the PL031 seconds range. Its
heart-transplant state preserves the control and migration channels, client
channels, request counter, and live MMIO mapping.

## D1 NVMe

Package: `bexos.driver.storage.nvme`

Targets:

- `//drivers/d1/storage/nvmexpress/nvme:nvme`
- `//drivers/d1/storage/nvmexpress/nvme:nvme_driver`
- `//drivers/d1/storage/nvmexpress/nvme:nvme_async_tests`
- `//drivers/d1/storage/nvmexpress/nvme:nvme_tests`
- `//drivers/d1/storage/nvmexpress/nvme:replacement_archive`

The NVMe driver is wave 1. Its manifest includes PCI bind rules for class
`0x01`, subclass `0x08`, and prog-if `0x02`. It exposes multi-instance
`bexos.storage.block.BlockDevice` and `bexos.power.DevicePowerControl`, accepts
per-client block endpoints replayed by appd, and implements D1 lifecycle
`PrepareStop` on the internal lifecycle channel.

The deployed driver ELF is std-linked through the BexOS libc shim and runs its
FIDL service loop on Tokio. Hardware commands expose async entrypoints with
bounded cooperative completion polling. Queue memory and payload buffers are
mapped through the per-device IOMMU domain handed out by appd, and the domain
mapping tokens, retained block endpoints, registered buffers, and FIFOs remain
explicit migration records. The BexFS block client recreates FIFO and buffer
registration through
the preserved control channel when a FIFO send failure proves the request was
not submitted; uncertain in-flight writes return an error to the caller. The
library is split into
admin, block, controller, guest, hardware, namespace, PRP, queue, and spec
modules.

## D1 VirtIO-Net

Package: `bexos.driver.network.virtio_net`

Targets:

- `//drivers/d1/nic/virtio/net:virtio_net`
- `//drivers/d1/nic/virtio/net:virtio_net_driver`
- `//drivers/d1/nic/virtio/net:virtio_net_tests`
- `//drivers/d1/nic/virtio/net:replacement_archive`

The VirtIO-Net driver is a D1 native Rust driver for QEMU `virtio-net-pci`. It
uses `virtio-drivers` with a BexOS HAL for per-device-domain DMA, PCI BAR MMIO
mapping, and raw Ethernet send/receive. Its manifest binds PCI virtio network devices
with vendor `0x1af4` and modern/transitional network device IDs. It exposes
multi-instance `bexos.hardware.ethernet.Device` and
`bexos.power.DevicePowerControl`.

The deployed VirtIO-Net ELF is std-linked through the BexOS libc shim and runs
on a two-worker Tokio runtime. Its heart-transplant adapter preserves control
and migration channels, Ethernet and power endpoints, the internal lifecycle
endpoint, FIFOs, registered shared VMOs, DMA mapping tokens, MMIO/ECAM mappings,
MAC/health/running state, and adopted VirtIO transport and queue state. Adoption
retains the live NIC without clearing device status, reprogramming queues, or
relying on a reset during activation. Netstack recreates Ethernet buffers and a
FIFO through the preserved device control channel after recovery and republishes
normal link status through its watcher path.

Unlike the boot-critical storage drivers, the QEMU product places VirtIO-Net in
the system image and preinstalls its `.bex` archive on the `STORAGE` package
partition. App-service imports preinstalled package manifests after the storage
pivot and runs a post-storage driver bind pass for matching D1 drivers.

## D1 Intel Ethernet

Packages: `bexos.driver.network.e1000e` and `bexos.driver.network.igb`

Targets:

- `//drivers/d1/nic/intel/common:intel_common_tests`
- `//drivers/d1/nic/intel/e1000e:e1000e_driver`
- `//drivers/d1/nic/intel/igb:igb_driver`

The common Intel library contains bounded legacy descriptor rings, DMA-range
validation, MAC discovery, reset/quiesce and interrupt-mask behavior, register
programming, and a versioned controller checkpoint. e1000e matches device IDs
`10d3`, `1539`, and `15b8`; igb matches `10c9` and `1521`. Each package defaults
to one isolated process per PCI function and requires MMIO, interrupt, IOMMU and
private bus-control resources. The ELF runtime retains the BAR mapping, hardware
capabilities, service endpoints and controller checkpoint during heart transplant.

Physical link traffic and MSI/MSI-X behavior have not yet been validated on a
real Intel adapter. The current testable descriptor model is not evidence of
physical-hardware acceptance.

## D1 USB xHCI

Package: `bexos.driver.usb.xhcid`

Targets:

- `//drivers/d1/usb/xhci:xhcid`
- `//drivers/d1/usb/xhci:xhcid_elf`
- `//drivers/d1/usb/xhci:tests`
- `//drivers/d1/usb/xhci:replacement_archive`

The xHCI driver binds PCI USB 3 controllers, requires an appd-provided IOMMU
domain, maps the controller MMIO BAR, parses xHCI capability registers, owns
controller hardware resources, and exposes private
`bexos.usb.host.XhciController` instances to `usbd`. Its migration state keeps
controller handles, MMIO mapping, retained command-ring/event-ring/scratchpad
DMA mappings and DMA tokens, bus-manager channels, registered buffers, queued
transfer IDs, and completion subscribers.

## D1 USB HID

Package: `bexos.driver.input.usb_hid`

Targets:

- `//drivers/d1/input/usb_hid:usb_hid`
- `//drivers/d1/input/usb_hid:usb_hid_elf`
- `//drivers/d1/input/usb_hid:tests`
- `//drivers/d1/input/usb_hid:replacement_archive`

The USB HID driver binds boot-protocol keyboard and mouse USB interfaces and
exposes `bexos.hardware.input.InputDevice`. It receives a scoped USB interface
channel through a `BUS_CONTROL` resource, tracks held keys/buttons, emits reset
state after disconnect or replacement, and migrates the input sink, sequence
number, held state, lifecycle channel, and interface channel.

## D1 USB Mass Storage

Package: `bexos.driver.storage.usb_bot`

Targets:

- `//drivers/d1/storage/usb/bot:usb_bot`
- `//drivers/d1/storage/usb/bot:usb_bot_elf`
- `//drivers/d1/storage/usb/bot:tests`
- `//drivers/d1/storage/usb/bot:replacement_archive`

The USB BOT driver binds SCSI bulk-only storage interfaces and exposes removable
`bexos.storage.block.BlockDevice` instances. It uses the existing block FIFO
request/response protocol and records LUN, capacity, tags, sense summary,
stalls, reset count, buffers, FIFO channels, queued requests, and completed
write watermark across replacement. Explicit BexFS mounting uses the existing
filesystem stack; automount is not implemented.

## D1 BexFS

Package: `bexos.driver.storage.bexfs`

Targets:

- `//drivers/d1/storage/bexos/bexfs:bexfs`
- `//drivers/d1/storage/bexos/bexfs:bexfs_std`
- `//drivers/d1/storage/bexos/bexfs:bexfs_driver`
- `//drivers/d1/storage/bexos/bexfs:bexfs_async_tests`
- `//drivers/d1/storage/bexos/bexfs:bexfs_tests`
- `//drivers/d1/storage/bexos/bexfs:replacement_archive`

BexFS is wave 2 and exposes singleton `bexos.fs.Filesystem`. Its on-disk and
filesystem core remains a no-std library used by host image tooling, tests, and
appd sys-state decoding. The deployed driver ELF is std-linked through the
BexOS libc shim and runs the FIDL service loop on Tokio while keeping roseFS and
block-device operations synchronous. Current modules cover block integration,
format, filesystem operations, GPT handling, async guest service adapters,
keys, server, and sys-state initialization.

User volumes are unlocked with a 32-byte UKEK derived by an authenticated
Trusty KeyMint HMAC over `bexos.ukek.v1\0 || uid_le64`. `usersd` transfers the
UKEK to `vfsd` only after successful Gatekeeper verification and zeroizes its
copy immediately. Raw UKEKs are never persisted. The static QEMU image key is
limited to prebuilt system/test volumes and is not the user-volume credential
path.

## D1 ArchiveFS

Package: `bexos.driver.storage.archivefs`

Targets:

- `//drivers/d1/storage/bexos/archivefs:archivefs`
- `//drivers/d1/storage/bexos/archivefs:archivefs_std`
- `//drivers/d1/storage/bexos/archivefs:archivefs_driver`
- `//drivers/d1/storage/bexos/archivefs:archivefs_async_tests`
- `//drivers/d1/storage/bexos/archivefs:archivefs_tests`
- `//drivers/d1/storage/bexos/archivefs:replacement_archive`

ArchiveFS is wave 2 and exposes singleton `bexos.storage.archive.ArchiveManager`. It mounts package archives and memory archives over the archive FIDL surface. Its archive model remains no-std-compatible, while the deployed driver is a std-linked Tokio service with live migration support for mounted archives, open endpoints, and file cursors.

## D1 MemFS

Package: `bexos.driver.storage.memfs`

Targets:

- `//drivers/d1/storage/bexos/memfs:memfs`
- `//drivers/d1/storage/bexos/memfs:memfs_std`
- `//drivers/d1/storage/bexos/memfs:memfs_driver`
- `//drivers/d1/storage/bexos/memfs:memfs_tests`
- `//drivers/d1/storage/bexos/memfs:memfs_async_tests`
- `//drivers/d1/storage/bexos/memfs:replacement_archive`

MemFS is wave 2 and exposes singleton `bexos.storage.memfs.MemfsManager`.
It creates fresh in-memory directory roots for app `/tmp` mounts. The core model
is no-std-compatible, while the deployed D1 driver is a std-linked Tokio service
with heart-transplant support for tmp instances, open endpoints, file contents,
directory state, and file cursors.

## D1 DiskImage

Package: `bexos.driver.storage.diskimage`

Targets:

- `//drivers/d1/storage/bexos/diskimage:diskimage`
- `//drivers/d1/storage/bexos/diskimage:diskimage_std`
- `//drivers/d1/storage/bexos/diskimage:diskimage_driver`
- `//drivers/d1/storage/bexos/diskimage:diskimage_async_tests`
- `//drivers/d1/storage/bexos/diskimage:diskimage_tests`
- `//drivers/d1/storage/bexos/diskimage:replacement_archive`

DiskImage is wave 3. It exposes singleton
`bexos.storage.diskimage.DiskImageManager`. Existing unencrypted `Attach`
remains supported for plain file-backed block devices. The manager also creates
and opens versioned encrypted images for user storage. Encrypted images use an
authenticated header, a sector allocation bitmap, AES-256-XTS sector
encryption with keys derived from the user's U-KEK, and keyed per-sector
integrity tags. Unwritten sectors read as zeroes and wrong keys or modified
headers/ciphertext fail closed. The deployed driver is a std-linked Tokio
service with heart-transplant support for manager state, attached file-backed
block devices, protected key handles, registered buffers, and FIFO endpoints.

## Linux Shim

`//drivers/d1/support/linux/shim` is a Rust support crate for compatibility-style
driver work. It has a no-std default target plus a std target for BexOS-backed
MMIO/DMA adapters and Tokio cooperative wait helpers. Current maintained
DMA-capable drivers use the kernel domain-backed mapping path directly; the shim
still carries legacy pin helpers for compatibility code that has not been moved
onto strict per-device domains. Current modules cover DMA, errors, IRQ, MMIO,
page handling, PCI, synchronization, and std-only task/BexOS runtime integration.
It has host tests under `linux_shim_tests` and `linux_shim_std_tests`.

## Storage Stack

The current storage stack is layered as:

- NVMe exposes a block device surface.
- BexFS consumes block access and provides filesystem behavior over RoseFS-backed logic.
- ArchiveFS mounts app archives and package archive memory.
- DiskImage adapts file-backed disk images into block devices, including the
  encrypted sparse user image format.
- VFSd coordinates package store initialization, package archive read/write/delete/list, system service data directories, and user data directory access.
- VFSd coordinates user filesystem lifecycle for `services/usersd`: creating
  per-user vault directories, unlocking encrypted images, revoking handles on
  lock, and deleting both image slots plus state on removal.
- Storage verification is a system-image component configured by product config.

## User Filesystems

The current user filesystem implementation uses encrypted per-user DiskImage
files stored under `vault/users/<uid>` in the outer `STORAGE` BexFS volume. Each
user has two slots, `slot_a.img` and `slot_b.img`, plus a checksummed
`state.bin` generation record. Images start at 16 MiB. vfsd monitors mounted
usage and, at 60% allocated utilization, creates the inactive slot at a larger
size, doubling until projected utilization is at most 30%. Resize state records
distinguish `PREPARING`, `COMMITTED`, and `STABLE`: preparing keeps the old
slot authoritative, committed promotes the candidate, and stable records the
final active slot. If the outer STORAGE volume cannot hold the shadow copy, the
old volume remains valid and the resize reports `NO_SPACE`.

Per-app user data remains scoped through `GetUserDataDirectory(uid, package_id)`
and is created inside the unlocked user image at
`apps/<package_id>/data`, which appd binds into the launched process namespace
as `/data`. Appd-authorized shared vaults are looked up through
`GetSharedVaultDirectory` and mounted as `/shared/<name>`. User-context local
vaults live inside the user image under `shared/local/<name>/data`; domain vaults
live under `shared/domain/<canonical_domain>/<name>/data`; system-only vaults
remain in outer STORAGE under `data/system/shared/local/<name>/data`.

System services can request a package-scoped system data directory under
`data/system/<package_id>`. The keychain service requests
`GetUserHomeDirectory(uid)`, so `keychain.redb` lives inside that user's
encrypted image and is available only while the user is unlocked. UID 0 launches
use system data directories; encrypted user images are reserved for nonzero
user IDs.

There is no legacy directory migration: the product had no existing users at
the time this layout landed. Per-user capacity has no configured ceiling; it
doubles from 16 MiB until arithmetic limits or real STORAGE exhaustion prevents
the next crash-safe shadow resize.

The QEMU disk image is built as a GPT disk with a generated initial `SYS_STATE`
BexFS partition and a storage partition. New SYS_STATE images use the v2
two-record sequenced format; appd upgrades older v1 records after a successful
boot.
