# RFC 0032: Application versions, pinning, and rollback — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0032](README.md)

## Implementation summary

Versioned archives, active pins, storage policies, probation, pruning, and protected-process restart/rollback are implemented.

## Implemented behavior

- Structured versions identify installed registry records and VFS archive paths. Active pin state is checkpointed and stored with the selected SYS_STATE slot.
- Appd implements single-active and multi-version policies, explicit rollback, archive allocation accounting, and inactive-version pruning with retained rollback candidates.
- The watchdog tracks exit/readiness/probation and schedules restarts or rollback. Service migration now commits durable archive identity and accepted generations.

## Gaps and deviations

- The RFC’s memory-only replacement-archive caveat is stale for the implemented appd staging/registry path; durable service selection now exists.
- An exit/readiness watchdog does not detect every hung process through periodic application heartbeats.
- Physical bootloader A/B recovery, arbitrary dependency solving, user picker UI, and compatibility of application data across rollback are not guaranteed by version pins.

## Sources and validation

Implementation and contract evidence: [lib/package_version/src/lib.rs](../../../lib/package_version/src/lib.rs), [lib/app_registry](../../../lib/app_registry), [services/appd/src/watchdog.rs](../../../services/appd/src/watchdog.rs), [services/appd/src/guest/update.rs](../../../services/appd/src/guest/update.rs), [services/vfsd/src/lib.rs](../../../services/vfsd/src/lib.rs), [idl/bexos/app/version_manager.fidl](../../../idl/bexos/app/version_manager.fidl).

Relevant test sources and Bazel targets: [lib/package_version/tests/package_version_tests.rs](../../../lib/package_version/tests/package_version_tests.rs), [lib/app_registry/BUILD.bazel](../../../lib/app_registry/BUILD.bazel), [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [secure runtime updates](../../secure-runtime-updates.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
