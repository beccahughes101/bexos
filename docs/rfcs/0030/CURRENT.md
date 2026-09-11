# RFC 0030: Background job scheduling — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0030](README.md)

## Implementation summary

Jobd is a shipped, declaration-bound background scheduler integrated with appd workers, user storage, time, power, and network watchers.

## Implemented behavior

- Caller package/UID identity and signed manifest job declarations bound scheduling requests. Installation-instance changes invalidate stored jobs.
- Durable system/user jobs use separate redb stores; locked users lose store access. Durable jobs wait for a realtime anchor, while transient jobs use monotonic time and do not survive cold boot.
- Due jobs are filtered by provider conditions and flex windows, batched under wake leases, launched through appd, and stopped on completion/failure/timeout.
- Migration retains jobs, running worker channels, batch leases, declaration caches, watcher bindings, and store routing, then reopens and merges durable state.

## Gaps and deviations

- Package updates require rescheduling after installation-instance changes; jobs are not automatically portable across arbitrary package revisions.
- Provider unavailability blocks jobs rather than fabricating charging/network/time state. QEMU does not validate physical battery-aware energy savings.
- Worker execution is not an arbitrary cron shell or a guarantee of exactly-once external effects across crashes and reboot. Timing and resource behavior remain bounded by the implemented runners/providers.

## Sources and validation

Implementation and contract evidence: [services/jobd/src/service.rs](../../../services/jobd/src/service.rs), [services/jobd/src/runtime.rs](../../../services/jobd/src/runtime.rs), [services/jobd/src/storage.rs](../../../services/jobd/src/storage.rs), [services/jobd/src/migration.rs](../../../services/jobd/src/migration.rs), [lib/job_store](../../../lib/job_store), [idl/bexos/app/worker.fidl](../../../idl/bexos/app/worker.fidl).

Relevant test sources and Bazel targets: [services/jobd/tests/jobd_tests.rs](../../../services/jobd/tests/jobd_tests.rs), [testing/e2e/qemu/jobs/BUILD.bazel](../../../testing/e2e/qemu/jobs/BUILD.bazel).

Detailed guides and previously recorded validation: [services](../../services.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
