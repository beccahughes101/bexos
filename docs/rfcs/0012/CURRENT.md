# RFC 0012: Kernel scheduling — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0012](README.md)

## Implementation summary

The active kernel uses a shared SMP scheduler with fair and deadline policies, capability-gated profiles, and resource-group accounting.

## Implemented behavior

- The scheduler implements priority bands, fair rotation, deadline admission, per-CPU ownership, affinity, wakeups, and timer deadlines. Architecture runtimes invoke it for actual guest scheduling.
- Task/profile FIDL operations expose scheduling controls; appd requires the realtime permission plus platform package/signer authorization.
- Channel call handling includes priority donation. Structured resource groups account for CPU and memory, with GPU reservation capabilities; scheduler state and accounting have snapshot/handover code.

## Gaps and deviations

- The deferred resource hierarchy is broader than current enforcement. GPU reservations are not hardware GPU scheduling, and CPU shares do not establish every proposed bandwidth/latency guarantee.
- Arbitrary nested donation and cycle handling remain hardening concerns; ordinary SMP scheduling does not prove every cross-CPU live-transplant scenario.
- Realtime latency, fairness under sustained load, and cutover budgets require measured guest workloads, not only host model tests.

## Sources and validation

Implementation and contract evidence: [kernel/core/src/sched.rs](../../../kernel/core/src/sched.rs), [kernel/core/src/sched](../../../kernel/core/src/sched), [kernel/src/sched/mod.rs](../../../kernel/src/sched/mod.rs), [kernel/core/src/kernel_services/scheduler.rs](../../../kernel/core/src/kernel_services/scheduler.rs), [services/appd/src/runner/mod.rs](../../../services/appd/src/runner/mod.rs), [idl/bexos/kernel/scheduler.fidl](../../../idl/bexos/kernel/scheduler.fidl).

Relevant test sources and Bazel targets: [kernel/core/BUILD.bazel](../../../kernel/core/BUILD.bazel).

Detailed guides and previously recorded validation: [kernel](../../kernel.md), [scened](../../scened.md), [multiarchitecture validation](../../multiarchitecture-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
