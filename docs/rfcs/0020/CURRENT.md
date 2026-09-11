# RFC 0020: In-process WASM sandboxes — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0020](README.md)

## Implementation summary

The WASM runtime hosts bounded children in independent engine stores and exposes spawn/invoke/control operations with restricted delegation.

## Implemented behavior

- Child instantiation separates core modules, command components, and service components. Signature/origin checks reject unsigned components importing ambient WASI.
- Runtime resources, budgets, and hostcalls enforce parent-authority attenuation and bounded handle-free unsigned messaging. Pause/resume/termination are implemented controls.
- Core and service children have checkpoint paths; migration coordinates paused child state and resource validation. Engine checks and host interfaces provide the in-process security boundary.

## Gaps and deviations

- QuickJS/Javy compilation, JavaScript packaging, and the unsigned graphics/input examples remain future work.
- Live command children explicitly reject snapshotting and require restart; support for some children is not universal checkpointability.
- Sub-VMARs do not create hardware isolation between stores in one native process. Fuel/epoch limits do not by themselves prove every native host operation is preemptible.

## Sources and validation

Implementation and contract evidence: [lib/wasm_runtime/src/child.rs](../../../lib/wasm_runtime/src/child.rs), [lib/wasm_runtime/src/sandbox.rs](../../../lib/wasm_runtime/src/sandbox.rs), [lib/wasm_runtime/src/resources.rs](../../../lib/wasm_runtime/src/resources.rs), [lib/wasm_runtime/src/budget.rs](../../../lib/wasm_runtime/src/budget.rs), [lib/wasm_runtime/src/migration.rs](../../../lib/wasm_runtime/src/migration.rs), [lib/wasm_runtime/wit/bexos.wit](../../../lib/wasm_runtime/wit/bexos.wit).

Relevant test sources and Bazel targets: [lib/wasm_runtime/src/tests](../../../lib/wasm_runtime/src/tests), [lib/wasm_runtime/BUILD.bazel](../../../lib/wasm_runtime/BUILD.bazel).

Detailed guides and previously recorded validation: [wasm runtime](../../wasm-runtime.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
