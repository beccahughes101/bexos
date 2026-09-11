# RFC 0047: Scene compositor architecture — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0047](README.md)

## Implementation summary

Scened is an implemented, modular userspace compositor with shared Flatland state, imported buffers, input dispatch, and CPU/GPU composition paths.

## Implemented behavior

- Per-client sessions stage atomic updates and commit geometry used for both presentation and hit-testing. Capability-backed references authorize embedding and shell control.
- Rendering selects supported direct-import, Vello/Venus, or CPU paths; damage, fences, admission, metrics, and recovery are separate modules.
- Migration records preserve logical scenes, pending presentations, mappings, input, shell/session identity, and controls; renderer/GPU state is reconstructed through supported adapters.

## Gaps and deviations

- The RFC’s sample main.rs is a sketch, not the structure of the actual multi-module service.
- The compositor has bounded sessions/transforms/buffer budgets, timer pacing, and incomplete physical GPU/VSYNC support. Direct import is not universal zero-copy scanout.
- Accessibility semantics, broad hardware acceptance, and all sustained effect budgets remain incomplete; the historical measurement reports contain specific misses and workload limits.

## Sources and validation

Implementation and contract evidence: [services/scened/src](../../../services/scened/src), [lib/flatland](../../../lib/flatland), [lib/flatland_render](../../../lib/flatland_render), [lib/graphics_runtime/src/migration.rs](../../../lib/graphics_runtime/src/migration.rs), [idl/bexos/ui/graphics.fidl](../../../idl/bexos/ui/graphics.fidl).

Relevant test sources and Bazel targets: [services/scened/tests](../../../services/scened/tests), [testing/performance/scened/BUILD.bazel](../../../testing/performance/scened/BUILD.bazel), [testing/e2e/qemu/graphics/BUILD.bazel](../../../testing/e2e/qemu/graphics/BUILD.bazel).

Detailed guides and previously recorded validation: [scened](../../scened.md), [scened validation](../../scened-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
