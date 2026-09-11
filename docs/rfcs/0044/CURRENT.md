# RFC 0044: Graphics drivers and kernel buffer allocation — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0044](README.md)

## Implementation summary

Kernel VMOs and delegated hardware resources back a native VirtIO-GPU display/transport driver. Rendering and display operations are userspace responsibilities.

## Implemented behavior

- Kernel memory services allocate/map VMOs and expose DMA/IOMMU operations under authority checks; the shared VirtIO HAL manages driver DMA buffers.
- The display driver owns scanout, bounded command submission, fence sequencing, GPU contexts, shared/host-visible memory operations, and presentation ownership.
- Native compositor/runner code uses the driver’s graphics FIDL. Driver and compositor migration retain mappings, resources, command state, and ownership generations.

## Gaps and deviations

- The runtime display path is VirtIO-based, not a general AMD/Intel/NVIDIA/Apple driver stack or every allocator/swapchain API sketched in the RFC.
- Command completion fences are not hardware VSYNC. Ordinary client presentation may copy into driver-owned backing; imported-resource paths do not imply universally zero-copy host display.
- Physical GPU DMA isolation, recovery, scanout timing, and transplant require hardware-specific acceptance beyond host tests and nested Venus fixtures.

## Sources and validation

Implementation and contract evidence: [kernel/core/src/runtime/memory.rs](../../../kernel/core/src/runtime/memory.rs), [lib/virtio_hal](../../../lib/virtio_hal), [drivers/d1/display/virtio/gpu/src](../../../drivers/d1/display/virtio/gpu/src), [idl/bexos/ui/graphics.fidl](../../../idl/bexos/ui/graphics.fidl), [lib/graphics_runtime/src/scanout.rs](../../../lib/graphics_runtime/src/scanout.rs).

Relevant test sources and Bazel targets: [drivers/d1/display/virtio/gpu/BUILD.bazel](../../../drivers/d1/display/virtio/gpu/BUILD.bazel), [lib/graphics_runtime/tests](../../../lib/graphics_runtime/tests), [testing/e2e/qemu/graphics/BUILD.bazel](../../../testing/e2e/qemu/graphics/BUILD.bazel).

Detailed guides and previously recorded validation: [scened](../../scened.md), [bootui](../../bootui.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
