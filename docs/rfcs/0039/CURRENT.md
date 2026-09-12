# RFC 0039: Dioxus WASM applications and native rendering — current implementation

- Reviewed: 2026-09-12
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0039](README.md)

## Implementation summary

WASM application/shared-component packaging, a validated native scene boundary,
and the first native retained-document renderer are implemented. The full
Blitz/Dioxus/Parley target remains incomplete, but documents now flow through a
native Stylo/Taffy host path for the system and user shells.

## Implemented behavior

- Apps declare WASM component imports; the separately signed Dioxus library exposes the WIT interface and accepts bounded document or scene submissions.
- `lib/dioxus_dom` implements the value-tree ABI. Version 2 carries stylesheet origins, raw author stylesheets, attributes, and node state bits while retaining v1 decode compatibility.
- `lib/ui/runtime` renders retained documents natively: it installs user-agent/user/author stylesheets into Stylo, projects the supported layout subset into Taffy, and emits validated scene batches owned by the runner.
- Service lifecycle and UI state participate in checkpoint/rebuild; renderer pointers are not serialized. SysUI/UserUI and the demo exercise the component boundary, and retained shell documents redraw on `bexos.ui.theme` updates from prefsd.

## Gaps and deviations

- The current native renderer is a bounded BexOS document projection, not the full Blitz/Parley DOM integration proposed for replacement behind the interface.
- Generic image and glyph/text paths still do not provide full asset decoding, rich shaping, or font rasterization. ABI support for these commands is not complete visual fidelity.
- JavaScript/Hermes/QuickJS/React Native bridges and direct general WASI-WebGPU exposure are not implemented. Unsupported operations and migration limits remain explicit.

## Sources and validation

Implementation and contract evidence: [apps/dioxus_shared](../../../apps/dioxus_shared), [lib/dioxus_dom/src/lib.rs](../../../lib/dioxus_dom/src/lib.rs), [lib/ui](../../../lib/ui), [lib/dioxus_scene/src/raster.rs](../../../lib/dioxus_scene/src/raster.rs), [lib/dioxus_render/src/lib.rs](../../../lib/dioxus_render/src/lib.rs), [lib/wasm_runtime/wit/bexos.wit](../../../lib/wasm_runtime/wit/bexos.wit), [services/wasm_runner](../../../services/wasm_runner), [services/prefsd](../../../services/prefsd).

Relevant test sources and Bazel targets: [lib/dioxus_scene/BUILD.bazel](../../../lib/dioxus_scene/BUILD.bazel), [lib/dioxus_dom/BUILD.bazel](../../../lib/dioxus_dom/BUILD.bazel), [lib/dioxus_render/BUILD.bazel](../../../lib/dioxus_render/BUILD.bazel), [apps/dioxus_demo/BUILD.bazel](../../../apps/dioxus_demo/BUILD.bazel).

Detailed guides and previously recorded validation: [dioxus](../../dioxus.md), [wasm runtime](../../wasm-runtime.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
