# RFC 0036: WASM runtime and driver architecture — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0036](README.md)

## Implementation summary

A shared Wasmtime/Pulley runtime runs standalone WASI components and restricted embedded children. The proposed WASM peripheral driver host is not implemented.

## Implemented behavior

- Ordinary WASM is compiled on-device to Pulley execution through the shared engine configuration and BexOS memory/TLS adapter; appd launches a separately verified native runner.
- WASI 0.2, versioned kernel/sandbox/service interfaces, composed Dioxus libraries, explicit resource limits, and checked hostcalls are implemented.
- Core/component services have checkpoint/restore and trusted-runner replacement support. Supported paused children participate; command-child migration has explicit limits.

## Gaps and deviations

- Native guest JIT, WASI preview1, guest threads, and QuickJS packaging are not supplied by this runtime.
- D2 driver builds are smoke targets. No general WASM HID/USB driver host or the proposed bexos:driver hostcall contract was found.
- WASI filesystem/socket compatibility is partial and returns explicit unsupported errors. Checkpoint support does not mean arbitrary WASI command execution can be resumed across replacement.

## Sources and validation

Implementation and contract evidence: [lib/wasm_engine](../../../lib/wasm_engine), [lib/wasm_runtime/src/engine.rs](../../../lib/wasm_runtime/src/engine.rs), [lib/wasm_runtime/src/wasi](../../../lib/wasm_runtime/src/wasi), [lib/wasm_runtime/src/migration.rs](../../../lib/wasm_runtime/src/migration.rs), [services/wasm_runner](../../../services/wasm_runner), [drivers/d2/testing/bexos/smoke](../../../drivers/d2/testing/bexos/smoke).

Relevant test sources and Bazel targets: [lib/wasm_runtime/BUILD.bazel](../../../lib/wasm_runtime/BUILD.bazel), [services/wasm_runner/BUILD.bazel](../../../services/wasm_runner/BUILD.bazel), [services/appd/tests/wasm_tests.rs](../../../services/appd/tests/wasm_tests.rs).

Detailed guides and previously recorded validation: [wasm runtime](../../wasm-runtime.md), [dioxus](../../dioxus.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
