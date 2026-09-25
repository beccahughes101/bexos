# RFC 0036: WASM runtime and driver architecture — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0036](README.md)

## Implementation summary

A shared Wasmtime/Pulley runtime runs standalone WASI components, restricted
embedded children, and the RFC 71 in-process network-extension sandbox. The
proposed WASM peripheral driver host is not implemented.

## Implemented behavior

- Ordinary WASM is compiled on-device to Pulley execution through the shared engine configuration and BexOS memory/TLS adapter; appd launches a separately verified native runner.
- WASI 0.2, versioned kernel/sandbox/service interfaces, composed Dioxus libraries, explicit resource limits, and checked hostcalls are implemented.
- Core/component services have checkpoint/restore and trusted-runner replacement support. Supported paused children participate; command-child migration has explicit limits.
- `vswitchd` reuses the runtime's BexOS linear-memory and Pulley-stack adapter
  for raw core-WASM network extensions. That profile has no WASI, admits only
  the monotonic-time import, copies bounded vectors through guest linear
  memory, meters fuel, and uses a 500 microsecond epoch watchdog. Module bytes,
  configuration, counters, fault state, and exported checkpoint memory
  participate in heart transplant.

## Gaps and deviations

- Native guest JIT, WASI preview1, guest threads, and QuickJS packaging are not supplied by this runtime. Network extensions also use Pulley rather than native JIT.
- Network extension vectors currently use validated bounded copies. Direct VMO
  aliasing remains a future design and is not implied by the shared runtime.
- D2 driver builds are smoke targets. No general WASM HID/USB driver host or the proposed bexos:driver hostcall contract was found.
- WASI filesystem/socket compatibility is partial and returns explicit unsupported errors. Checkpoint support does not mean arbitrary WASI command execution can be resumed across replacement.

## Sources and validation

Implementation and contract evidence: [lib/wasm_engine](../../../lib/wasm_engine), [lib/wasm_runtime](../../../lib/wasm_runtime), [network extension ABI](../../../lib/network_extension_abi), [services/wasm_runner](../../../services/wasm_runner), [services/vswitchd](../../../services/vswitchd), and [drivers/d2/testing/bexos/smoke](../../../drivers/d2/testing/bexos/smoke).

Relevant test sources and Bazel targets: [lib/wasm_runtime/BUILD.bazel](../../../lib/wasm_runtime/BUILD.bazel), [services/wasm_runner/BUILD.bazel](../../../services/wasm_runner/BUILD.bazel), [services/appd/tests/wasm_tests.rs](../../../services/appd/tests/wasm_tests.rs).

Detailed guides and previously recorded validation: [wasm runtime](../../wasm-runtime.md), [dioxus](../../dioxus.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
