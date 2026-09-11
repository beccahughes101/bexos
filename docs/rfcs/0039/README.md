# RFC 0039: Dioxus WASM applications and native rendering

- Created: 2026-08-30T22:11:56-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Dioxus applications use a WASI component boundary and a separately packaged shared UI component. Native services retain compositor, GPU, and migration ownership, with the full Blitz DOM stack as the long-term integration target.

## Design overview

Dioxus Native and Blitz fit BexOS because they let a Rust application keep a familiar component model while BexOS keeps native compositor, GPU, and migration state out of the guest ABI. The implemented boundary is a WASI 0.2 application component plus a separately packaged shared WASM component, composed by the runner before execution. The long-term goal is still the full Blitz headless DOM stack behind that component boundary.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION COMPONENT (WASI 0.2, app package)                               │
│                                                                             │
│  Dioxus app state, signals, callbacks, lifecycle hooks, checkpoint model     │
│  Stable SDK calls into imported WIT instance: bexos:wasm/dioxus@1.0.0        │
└────────────────────────────────────┬────────────────────────────────────────┘
                                     │ component-model import, no Rust ptrs
┌────────────────────────────────────▼────────────────────────────────────────┐
│ SHARED UI COMPONENT (/deps/com.bexos.lib.dioxus)                            │
│                                                                             │
│  Implemented export: bexos:wasm/dioxus@1.0.0                                │
│  Current tree: value DOM + CSS subset + layout/text scene projection         │
│  Target backend: Blitz DOM + Stylo + Taffy + Parley + scene generation       │
└────────────────────────────────────┬────────────────────────────────────────┘
                                     │ validated 2D scene batches
┌────────────────────────────────────▼────────────────────────────────────────┐
│ NATIVE WASM RUNNER                                                           │
│                                                                             │
│  Scene validation, Vello/Venus GPU renderer, CPU replay fallback,            │
│  Flatland content/fence presentation, checkpoint/migration quiescence        │
└────────────────────────────────────┬────────────────────────────────────────┘
                                     │ FlatlandSession content + fences
┌────────────────────────────────────▼────────────────────────────────────────┐
│ scened                                                                       │
│                                                                             │
│  Compositor-owned session, input routing, viewport state, buffer release     │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Implemented package and component boundary

The application and shared UI library are separate packages. The application declares a normal `runner: "wasm"` process and an explicit `component_imports` entry. The shared library package declares `library_exports.kind: WASM_COMPONENT`. The native ELF behavior remains the compatibility default when the kind is omitted.

Appd resolves the component dependency through the existing package/version registry, rejects cycles, missing exports, ABI mismatches, graph limit violations, and native library substitution, then passes the application payload and dependency component payloads to the runner. The runner composes them with pinned `wac-graph` before compiling with Wasmtime/Pulley. The running instance retains the resolved dependency identities, so a shared library update takes effect on relaunch or on an application transplant that prepares a compatible new graph.

This boundary is intentional: Dioxus app code owns application state and lifecycle, while DOM/CSS/layout/text/scene generation can evolve in the shared component without changing each app package. The interface passes values and byte buffers only; it does not pass Rust callbacks, raw pointers, GPU objects, VMOs, or renderer handles across components.

## Blitz and Dioxus integration target

The target shared component implementation uses Dioxus Native/Blitz headless DOM integration as the reconciler target. Dioxus template and mutation traffic maps to DOM operations for templates, dynamic nodes, attributes, listeners, text updates, insertion, removal, and reordering. Blitz then uses Stylo for selectors and computed style, Taffy for flex/grid layout, Parley for text shaping/layout, and a selected Blitz painter to emit the BexOS scene ABI.

The current repository lands the stable packaging, WIT, scene ABI, renderer, host presentation, demo, migration scaffolding, and a bounded value-document projection in the shared component. The checked-in shared component accepts raw scenes for compatibility and accepts encoded document trees for normal Dioxus-style apps; it resolves tag/class/id selectors, a limited CSS declaration set, flex/grid-style layout, and glyph runs before forwarding a validated scene to the host UI interface. The full Blitz DOM/Stylo/Taffy/Parley body remains the target replacement behind the same exported component once the dependency set is Bazel-pinned and WASI-patched.

Dioxus/Blitz dependencies should remain Bazel-pinned as a compatible 0.7-era set. WASI-specific patches should keep layout single-threaded, route time through BexOS hostcalls, disable browser/windowing integrations and system-font discovery, and load packaged fonts, stylesheets, and images through granted filesystem mounts.

## Scene and rendering boundary

BexOS does not expose WASI-WebGPU for Dioxus UI. The guest emits a high-level 2D display list; the native runner validates the complete batch before rendering. The first version covers explicit paths, fills/strokes, linear gradients, transforms, clipping/layers, glyph runs, images, and shadows. Validation checks byte length, finite coordinates, dimensions, command and asset limits, layer balance, image opacity, asset ownership, and resource budgets. Unsupported operations fail explicitly and leave the last valid frame on screen.

The native runner owns all GPU handles, Vulkan/WGPU objects, shaders, VMOs, caches, and buffer mappings. It checks authenticated grants before creating a view or using GPU transport. Unsigned child sandboxes keep the existing private-compute policy and receive no graphics or hardware grants.

Rendering defaults to GPU when `bexos.hardware.display.DisplayCoordinator` grants the required `GpuTransport` methods. The runner uses the native Venus worker and Vello renderer with a bounded queue and completion polling. Initialization failure, device loss, renderer failure, or absent grants fall back to CPU replay of the same scene commands. The app can query the active backend and last failure through the UI interface.

Presentation uses `FlatlandSession` content and fence APIs. The runner keeps double-buffered VMOs and reuses a buffer only after compositor release; GPU completion alone is not buffer retirement. This preserves the existing readback path and avoids requiring zero-copy GPU import for Dioxus UI.

## Input, scheduling, and viewport

Scened routes session-local pointer, keyboard, Unicode text, wheel, and focus events. The runner exposes those as bounded input batches through the Dioxus WIT interface so Blitz can deliver them to Dioxus listeners. Flatland ordinal 24 adds a session-local viewport query for resize and scale updates without giving the application shell control.

The app drives updates through bounded lifecycle dispatch. A Dioxus app renders when its model is dirty, presentation state changes, input arrives, or animation work is due. The runner keeps input, rendering completion, and migration quiescence responsive by refusing unbounded presentation queues and draining GPU work before cutover.

## Heart transplant

Application checkpoints are model checkpoints. A Dioxus app records serializable state such as counters, text input content, scroll offsets, focus identifiers, editable values, selection, backend observations, and stable logical view IDs. Restore reconstructs Dioxus/Blitz and scene state from that model. Engine stacks, renderer pointers, Vello caches, GPU objects, VMOs, and compositor-owned native handles are not serialized.

The runner migration codec includes dependency identities, component payload identities, UI resource state, retained asset metadata, pending presentation sequences, and renderer quiescence requirements. Appd prepares dependency composition, compilation, and replacement stores before the final quiescence point. The original app resumes if restore fails, and cutover is deferred while outstanding rendering work or compositor releases cannot be transferred safely.

The installable demo includes original and replacement manifests with `HEART_TRANSPLANT`; it exercises a counter, editable text, scrolling, simple flex/grid-style layout, image command, backend status, and checkpoint/restore hooks.

## Future JavaScript and React Native bridge

JavaScript and React Native remain future layers on top of the same shared component boundary. A separately packaged Hermes or QuickJS component could run under `/deps/com.bexos.lib.hermes`, translate JSX/native view output into Blitz DOM mutations, and reuse Stylo/Taffy/Parley plus the BexOS scene ABI. That bridge should not bypass the validated 2D scene protocol or expose WebGPU directly to ordinary UI apps.

Raw WebGPU belongs to workloads that need application-authored shaders, 3D graphics, CAD, games, or GPU compute. Standard enterprise UI, dashboards, forms, and shell surfaces should stay on the host-rendered 2D boundary because it has a smaller ABI and a clearer security story.

## Verification and measurement policy

Current verification must distinguish three things:

- Host builds/tests that prove package resolution, WIT/component composition, scene validation, CPU replay, Vello conversion, grant checks, buffer retirement logic, and migration rollback behavior.
- QEMU/software-Vulkan tests that prove visible rendering and native GPU submission through Venus/Lavapipe, plus forced CPU fallback, resize, multiple-client isolation, cleanup, and replacement behavior.
- Physical-GPU acceleration evidence, which requires hardware-specific runs and should never be inferred from software Vulkan.

Size, boot-time, and performance numbers should be recorded only after measurement. Until then, design notes may state expectations, but current docs must not present unmeasured values as accepted results.
