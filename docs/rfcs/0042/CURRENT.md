# RFC 0042: Flatland rendering, effects, and scheduling — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0042](README.md)

## Implementation summary

Scened implements Flatland transactions, layout/style/text integration, CPU effects, GPU composition, and scheduling/accounting support.

## Implemented behavior

- Shared Flatland libraries provide committed geometry, layout, styling, text, input, and rendering. Scened composes imported surfaces and supports direct presentation and a Vello/Venus worker.
- CPU paths implement bounded effects and damage processing; presentation has acquire/release/completion synchronization and failure recovery.
- The service requests deadline scheduling and records CPU/allocation/frame metrics. Migration preserves logical/pending/committed state and rebuilds renderer resources.

## Gaps and deviations

- Hardware VSYNC is unavailable in the inspected service startup path; requested 120 Hz is timer pacing, not measured physical 120 Hz acceptance.
- The retained 2026-09-08 reports distinguish native and nested Venus workloads, including fallback budget misses. They do not establish universal 1080p effect budgets or physical GPU throughput.
- Big/LITTLE policy, general hardware drivers, full accessibility, and every planned effect/animation path remain broader than this virtual-product implementation.

## Sources and validation

Implementation and contract evidence: [services/scened/src/lib.rs](../../../services/scened/src/lib.rs), [services/scened/src/gpu.rs](../../../services/scened/src/gpu.rs), [services/scened/src/presentation.rs](../../../services/scened/src/presentation.rs), [services/scened/src/metrics.rs](../../../services/scened/src/metrics.rs), [lib/flatland_layout](../../../lib/flatland_layout), [lib/flatland_style](../../../lib/flatland_style), [lib/flatland_render](../../../lib/flatland_render).

Relevant test sources and Bazel targets: [services/scened/BUILD.bazel](../../../services/scened/BUILD.bazel), [testing/performance/scened/BUILD.bazel](../../../testing/performance/scened/BUILD.bazel), [testing/e2e/qemu/graphics/BUILD.bazel](../../../testing/e2e/qemu/graphics/BUILD.bazel).

Detailed guides and previously recorded validation: [scened](../../scened.md), [scened validation](../../scened-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
