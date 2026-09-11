# RFC 0003: Native and compatibility gaming — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0003](README.md)

## Implementation summary

WASM execution and native graphics infrastructure exist, but the dual-track gaming platform is not implemented.

## Implemented behavior

- Appd launches ELF and WASM processes. The WASM runtime provides bounded execution and host interfaces; the graphics stack includes native Vello/wgpu and a VirtIO-GPU/Venus path.
- Existing WASM service migration and graphics-driver migration are reusable platform mechanisms, not proof of game or VM continuity.

## Gaps and deviations

- No game-facing general WebGPU WASM interface, packaged game engine integration, Steam/Proton runner, or gaming MicroVM lifecycle was found. Web/Android/Nix runner names are recognized but unsupported by RunnerRegistry.
- The Linux/Venus test fixture and secure monitor virtualization code do not constitute the proposed userspace gaming VM.
- WASI preview1/wasm64 engine compatibility, physical AMD/NVIDIA/Apple GPU support, VM heart transplant, sub-200 ms boot, and 95–98% native throughput are not established by current implementation evidence.

## Sources and validation

Implementation and contract evidence: [services/appd/src/runner/mod.rs](../../../services/appd/src/runner/mod.rs), [lib/wasm_runtime/wit/bexos.wit](../../../lib/wasm_runtime/wit/bexos.wit), [lib/graphics_runtime](../../../lib/graphics_runtime), [testing/e2e/qemu/venus_linux](../../../testing/e2e/qemu/venus_linux).

Relevant test sources and Bazel targets: [lib/wasm_runtime/BUILD.bazel](../../../lib/wasm_runtime/BUILD.bazel).

Detailed guides and previously recorded validation: [wasm runtime](../../wasm-runtime.md), [scened](../../scened.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
