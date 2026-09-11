# RFC 0054: Media codecs and resource accounting — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0054](README.md)

## Implementation summary

Kernel resource accounting and graphics buffers exist, but the media broker and codec-worker architecture is not implemented.

## Implemented behavior

- Kernel resource groups and appd manifest limits provide CPU/memory accounting and GPU reservations. VMOs and graphics protocols provide reusable shared-buffer infrastructure.
- The OS can launch native/WASM service processes with scoped capabilities; these are prerequisites rather than media-specific session behavior.

## Gaps and deviations

- No mediad package, CodecSession FIDL, media buffer-pool broker, isolated codec-worker selection, hardware video decoder/encoder driver, or integrated OxideAV pipeline was found.
- The proposed delegated resource-accounting token that charges brokered codec work to the requesting app is not implemented simply by creating process resource groups.
- Codec recovery/heart transplant, stream checkpointing, format interoperability, throughput, and hardware acceleration have no dedicated implementation acceptance here. Memory-safety claims in the design are not validation results.

## Sources and validation

Implementation and contract evidence: [kernel/core/src/kernel_services/scheduler.rs](../../../kernel/core/src/kernel_services/scheduler.rs), [kernel/core/src/runtime/memory.rs](../../../kernel/core/src/runtime/memory.rs), [idl/bexos/kernel/scheduler.fidl](../../../idl/bexos/kernel/scheduler.fidl), [idl/bexos/ui/graphics.fidl](../../../idl/bexos/ui/graphics.fidl), [services/appd/src/waves.rs](../../../services/appd/src/waves.rs).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [kernel](../../kernel.md), [scened](../../scened.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
