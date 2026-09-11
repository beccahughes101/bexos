# RFC 0004: Userspace heart transplant — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0004](README.md)

## Implementation summary

Explicit service migration, quarantined candidates, resource adoption, and durable replacement selection are implemented. Full system continuity and every failure mode are not established merely by the available tests.

## Implemented behavior

- Appd stages a verified replacement archive, checks identity and generation, launches a deferred candidate, drives bounded bulk/delta/quiesce/activate operations, and commits the accepted archive record.
- The shared migration library and userspace adapters encode explicit versioned records. Kernel transactions transfer retained capability/resource ownership and retire the source; precommit failure can resume the source.
- Appd, core services, and D1 drivers have component-specific migration adapters and replacement archives. WASM service checkpointing extends the original ELF-only direction.

## Gaps and deviations

- Updates are per-component transactions; whole-system atomic replacement and recovery after commit are not provided by this protocol.
- Support is adapter-specific. New runner classes and services must define compatible state, resources, and rollback; secure-app private state has separate limitations.
- Existing QEMU tests cover many service and kernel replacement scenarios, but do not prove all-component continuity on physical hardware or all candidate-kernel faults. Timing budgets require measured runs.

## Sources and validation

Implementation and contract evidence: [services/appd/src/guest/update.rs](../../../services/appd/src/guest/update.rs), [services/appd/src/runner/deferred.rs](../../../services/appd/src/runner/deferred.rs), [lib/migration/src](../../../lib/migration/src), [lib/userspace/src/live_migration.rs](../../../lib/userspace/src/live_migration.rs), [kernel/src/transplant](../../../kernel/src/transplant).

Relevant test sources and Bazel targets: [lib/migration/BUILD.bazel](../../../lib/migration/BUILD.bazel), [testing/e2e/qemu/update/BUILD.bazel](../../../testing/e2e/qemu/update/BUILD.bazel), [tools/heart_transplant/coverage_test.sh](../../../tools/heart_transplant/coverage_test.sh).

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md), [secure integration validation](../../secure-integration-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
