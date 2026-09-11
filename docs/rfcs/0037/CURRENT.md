# RFC 0037: Graphical boot and display handoff — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0037](README.md)

## Implementation summary

Workstation products implement splash rendering and a generation-checked handoff to scened. Appd coordinates boot; there is no separate bootsvc.

## Implemented behavior

- Splashd renders progress from actual appd readiness events. Scened waits for readiness, imports the frozen frame, acknowledges ownership transfer, and transitions to its desktop composition.
- The D1 display driver owns scanout and presentation authority. The boot ABI can also carry framebuffer metadata delegated as a non-executable VMO; rendering stays in userspace.
- Splash, compositor, display, and input components have migration/replacement support preserving mappings, queues, ownership generations, and pending handoff state.

## Gaps and deviations

- QEMU normally establishes display through VirtIO-GPU, not a firmware GOP-to-native-GPU transfer. Optional framebuffer support is not acceptance of every firmware/board path.
- Animation uses timer pacing and deadline budgets; hardware VSYNC and uninterrupted measured 60 FPS are not established by those settings.
- The design’s standalone bootsvc and universal seamless takeover pipeline differ from the concrete appd/splashd/scened implementation. Physical display continuity remains unverified.

## Sources and validation

Implementation and contract evidence: [services/splashd/src](../../../services/splashd/src), [services/scened/src/render.rs](../../../services/scened/src/render.rs), [drivers/d1/display/virtio/gpu/src](../../../drivers/d1/display/virtio/gpu/src), [lib/boot/framebuffer.rs](../../../lib/boot/framebuffer.rs), [device/base/graphics/graphics.aib.prototxt](../../../device/base/graphics/graphics.aib.prototxt).

Relevant test sources and Bazel targets: [services/splashd/tests/migration_tests.rs](../../../services/splashd/tests/migration_tests.rs), [testing/e2e/qemu/graphics/BUILD.bazel](../../../testing/e2e/qemu/graphics/BUILD.bazel), [lib/graphics/tests/graphics_tests.rs](../../../lib/graphics/tests/graphics_tests.rs).

Detailed guides and previously recorded validation: [bootui](../../bootui.md), [scened](../../scened.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
