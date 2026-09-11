# RFC 0045: Input routing and event delivery — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0045](README.md)

## Implementation summary

D1 input devices feed scened directly; shared routing code uses committed geometry, focus, and capture to deliver bounded view events.

## Implemented behavior

- VirtIO input and USB HID publish the input-device contract. Appd supplies device grants and hotplug attachment; scened subscribes without blocking frame deadlines.
- Flatland input libraries handle pointer/key/touch state, settings, event queues, coordinate transforms, capture, and gesture helpers. View focus/stacking is capability controlled.
- Input device and compositor checkpoints retain endpoint/queue/held-input state and session generation; lock/logout revokes the user input hierarchy.

## Gaps and deviations

- There is no separate general inputd service matching every layer in the drawing; routing is integrated with scened and native D1 providers.
- The VirtIO path uses a timer polling fallback. Host gesture tests and deterministic USB fixtures do not establish broad physical touchpad/gamepad/Bluetooth support.
- The shell’s internationalized input/IME and complete system-gesture/overlay priority behavior remain incomplete.

## Sources and validation

Implementation and contract evidence: [drivers/d1/input/virtio](../../../drivers/d1/input/virtio), [drivers/d1/input/usb_hid](../../../drivers/d1/input/usb_hid), [services/appd/src/guest/input_hotplug.rs](../../../services/appd/src/guest/input_hotplug.rs), [services/scened/src/input.rs](../../../services/scened/src/input.rs), [services/scened/src/input_link.rs](../../../services/scened/src/input_link.rs), [lib/flatland_input](../../../lib/flatland_input), [idl/bexos/ui/input.fidl](../../../idl/bexos/ui/input.fidl).

Relevant test sources and Bazel targets: [lib/flatland_input/tests](../../../lib/flatland_input/tests), [services/scened/tests/controls.rs](../../../services/scened/tests/controls.rs), [testing/e2e/qemu/graphics/BUILD.bazel](../../../testing/e2e/qemu/graphics/BUILD.bazel).

Detailed guides and previously recorded validation: [scened](../../scened.md), [usb](../../usb.md), [sysui](../../sysui.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
