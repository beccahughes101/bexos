# RFC 0043: Crash handling and recovery — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0043](README.md)

## Implementation summary

The kernel, appd, and WASM runner have fault/exit handling and recovery mechanisms, but the proposed crashd artifact service is not implemented.

## Implemented behavior

- Kernel panic handling writes diagnostics; architecture exception paths handle process faults. WASM execution reports traps/errors through the runtime.
- Appd monitors process exit/readiness and restarts or rolls back protected packages; driver recovery separately rebinds failed devices.
- Live-transplant failure handling can abort a precommit candidate and resume the source. This is distinct from collecting a post-crash artifact.

## Gaps and deviations

- No crashd package, bexos.system.crashd exception protocol, structured minidump collector, out-of-process unwinder, or encrypted quota-managed crash vault matching the RFC was found.
- PII redaction, user-visible crash reporting, telemetry upload, and secure inspector capability delegation remain design work.
- Existing watchdog and rollback behavior cannot be described as the full crash recovery matrix, and there is no crashd heart transplant adapter to validate.

## Sources and validation

Implementation and contract evidence: [kernel/src/panic/mod.rs](../../../kernel/src/panic/mod.rs), [kernel/src/arch/x86_64/context.rs](../../../kernel/src/arch/x86_64/context.rs), [services/appd/src/watchdog.rs](../../../services/appd/src/watchdog.rs), [services/appd/src/recovery.rs](../../../services/appd/src/recovery.rs), [lib/wasm_runtime/src/instance.rs](../../../lib/wasm_runtime/src/instance.rs).

Relevant test sources and Bazel targets: [kernel/core/BUILD.bazel](../../../kernel/core/BUILD.bazel), [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs).

Detailed guides and previously recorded validation: [kernel](../../kernel.md), [appd userspace](../../appd-userspace.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
