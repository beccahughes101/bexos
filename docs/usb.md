# USB

The current USB implementation is a host-mode D1 stack split across four
processes:

- `bexos.driver.usb.xhcid` (`//drivers/d1/usb/xhci`) binds PCI xHCI
  controllers with class `0x0c`, subclass `0x03`, prog-if `0x30`.
- `bexos.service.usbd` (`//services/usbd`) owns USB topology, descriptor policy,
  interface claims, and appd registration of USB descendants.
- `bexos.driver.input.usb_hid` (`//drivers/d1/input/usb_hid`) binds boot
  keyboard and boot mouse interfaces and exposes the existing
  `bexos.hardware.input.InputDevice` protocol.
- `bexos.driver.storage.usb_bot` (`//drivers/d1/storage/usb/bot`) binds SCSI
  bulk-only mass-storage interfaces and exposes the existing
  `bexos.storage.block.BlockDevice` protocol with the `REMOVABLE` flag.

The shared `//lib/usb_host` crate contains USB constants, defensive descriptor
parsing, class policy, transfer and buffer validation, boot HID report
translation, SCSI BOT helpers, xHCI capability parsing, and migration codecs.
Class drivers receive scoped USB interface channels through `BUS_CONTROL`
resources; xHCI MMIO, interrupt, and IOMMU resources stay in `xhcid`.

## Current Behavior

`xhcid` requires an isolated IOMMU domain and an MMIO BAR. It parses xHCI
capability registers, retains controller MMIO/interrupt/IOMMU handles plus command-ring,
event-ring, and scratchpad DMA VMO mappings and DMA tokens across migration,
accepts buffer registration through the controller IOMMU domain, validates transfer IDs, endpoint numbers, buffer ranges, and
directions, and refuses to unregister buffers while work is in flight.

`usbd` accepts controller grants, registers as the controller bus manager,
maintains generation-tagged device and interface topology, applies the default
class policy for hubs, boot HID, and SCSI BOT, rejects composites unless every
interface is supported, and registers claimed USB interfaces with appd as USB
device nodes carrying only a scoped `BUS_CONTROL` channel.

USB HID tracks held keyboard modifiers/keys and mouse buttons, emits release
state through reset reports, and preserves the subscriber sink, sequence number,
held state, interface channel, and driver lifecycle channel across replacement.

USB BOT tracks LUN, capacity, command tags, sense summary, endpoint stalls,
transport reset count, registered block buffers, FIFO endpoints, queued request
IDs, and the completed-write watermark across replacement. Flushes are exposed
through the existing block FIFO opcode. The filesystem path remains explicit:
BexFS is mounted through the existing VFS/filesystem APIs; there is no automount
and no new filesystem format.

## Validation Status

Focused USB build coverage:

```sh
bazel build //idl:usb_host_fidl_rust \
  //lib/usb_host:tests \
  //drivers/d1/usb/xhci:tests \
  //services/usbd:tests \
  //drivers/d1/input/usb_hid:tests \
  //drivers/d1/storage/usb/bot:tests
```

Package/appd integration build coverage:

```sh
bazel build //services/appd:appd \
  //drivers/d1/usb/xhci:xhcid \
  //services/usbd:usbd \
  //drivers/d1/input/usb_hid:usb_hid \
  //drivers/d1/storage/usb/bot:usb_bot \
  //drivers/d1:manifest_services_test
```

QEMU AArch64 USB smoke coverage is declared and builds at
`//testing/e2e/qemu/usb:usb_host_smoke_test_aarch64`. The launched QEMU command
includes `qemu-xhci,msi=off,msix=off`, `usb-kbd`, `usb-mouse`, and `usb-hub`.
On 2026-09-09 the VM did not reach the USB readiness markers before the run was
interrupted to avoid leaving a long-lived helper VM.

Outstanding validation remains for full hardware xHCI command/event/transfer
ring operation, external hub hotplug, actual HID input delivery from xHCI
interrupt completions, SCSI BOT data-path read/write/flush against a USB-backed
BexFS volume, x86_64 guest runtime execution, and physical hardware.
