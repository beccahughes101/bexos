# RFC 0039: Dioxus WASM applications and native rendering — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0039](README.md)

## Implementation summary

WASM application/shared-component packaging and a validated native scene boundary are implemented. The full Blitz/Dioxus DOM rendering target remains incomplete.

## Implemented behavior

- Apps declare WASM component imports; the separately signed Dioxus library exposes the WIT interface and accepts bounded document or scene submissions.
- Lib/dioxus_dom implements value-tree decoding, simple selectors/CSS, bounded layout, and scene projection. The native runner validates scene batches and owns CPU/GPU presentation resources.
- Service lifecycle and UI state participate in checkpoint/rebuild; renderer pointers are not serialized. SysUI/UserUI and the demo exercise the component boundary.

## Gaps and deviations

- The shared component is a bounded custom projection, not the full Blitz/Stylo/Taffy/Parley DOM integration proposed for replacement behind the interface.
- Generic image and glyph commands in the inspected Dioxus render/replay paths use synthetic rectangles/colors rather than full asset decoding and font rasterization. ABI support for these commands is not complete visual fidelity.
- JavaScript/Hermes/QuickJS/React Native bridges and direct general WASI-WebGPU exposure are not implemented. Unsupported operations and migration limits remain explicit.

## Sources and validation

Implementation and contract evidence: [apps/dioxus_shared](../../../apps/dioxus_shared), [lib/dioxus_dom/src/lib.rs](../../../lib/dioxus_dom/src/lib.rs), [lib/dioxus_scene/src/raster.rs](../../../lib/dioxus_scene/src/raster.rs), [lib/dioxus_render/src/lib.rs](../../../lib/dioxus_render/src/lib.rs), [lib/wasm_runtime/wit/bexos.wit](../../../lib/wasm_runtime/wit/bexos.wit), [services/wasm_runner](../../../services/wasm_runner).

Relevant test sources and Bazel targets: [lib/dioxus_scene/BUILD.bazel](../../../lib/dioxus_scene/BUILD.bazel), [lib/dioxus_dom/BUILD.bazel](../../../lib/dioxus_dom/BUILD.bazel), [lib/dioxus_render/BUILD.bazel](../../../lib/dioxus_render/BUILD.bazel), [apps/dioxus_demo/BUILD.bazel](../../../apps/dioxus_demo/BUILD.bazel).

Detailed guides and previously recorded validation: [dioxus](../../dioxus.md), [wasm runtime](../../wasm-runtime.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
