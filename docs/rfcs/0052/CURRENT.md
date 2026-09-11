# RFC 0052: USB controller and device services — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0052](README.md)

## Implementation summary

USB has D1 controller/bus/class service scaffolding, defensive protocol models, and migration records. A complete hardware transfer data path is not established.

## Implemented behavior

- Xhcid maps controller capability registers and allocates IOMMU-backed command/event/scratchpad resources. It validates buffer registrations and transfer requests.
- Usbd models topology, descriptor/class policy, interface claims, and scoped descendant registration. Boot HID and SCSI BOT class drivers expose existing input/block contracts.
- Controller, bus manager, and class drivers have HEART_TRANSPLANT manifests, replacement archives, and records for endpoints, buffers, topology, and class state.

## Gaps and deviations

- In xhcid, SubmitTransfer currently increments buffer in-flight state and queues a transfer ID. The inspected service/hardware code does not submit that request to a hardware ring or deliver hardware completion events; allocation of rings is not an operational xHCI engine.
- The recorded AArch64 smoke run did not reach USB readiness. Actual enumeration/hotplug, HID traffic, BOT read/write/flush, x86 guest behavior, and physical hardware remain unvalidated.
- Device mode, MSI-X, isochronous scheduling, general HID, UAS, audio/video/CDC classes, stronger peripheral authorization, and automount are missing.

## Sources and validation

Implementation and contract evidence: [drivers/d1/usb/xhci/src/hardware.rs](../../../drivers/d1/usb/xhci/src/hardware.rs), [drivers/d1/usb/xhci/src/service.rs](../../../drivers/d1/usb/xhci/src/service.rs), [drivers/d1/usb/xhci/src/migration.rs](../../../drivers/d1/usb/xhci/src/migration.rs), [services/usbd/src](../../../services/usbd/src), [drivers/d1/input/usb_hid](../../../drivers/d1/input/usb_hid), [drivers/d1/storage/usb/bot](../../../drivers/d1/storage/usb/bot), [lib/usb_host](../../../lib/usb_host).

Relevant test sources and Bazel targets: [drivers/d1/usb/xhci/BUILD.bazel](../../../drivers/d1/usb/xhci/BUILD.bazel), [services/usbd/BUILD.bazel](../../../services/usbd/BUILD.bazel), [lib/usb_host/BUILD.bazel](../../../lib/usb_host/BUILD.bazel), [testing/e2e/qemu/usb/BUILD.bazel](../../../testing/e2e/qemu/usb/BUILD.bazel).

Detailed guides and previously recorded validation: [usb](../../usb.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
