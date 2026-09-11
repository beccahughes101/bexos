# RFC 0022: Update engine — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0022](README.md)

## Implementation summary

Direct signed uploads, package/service activation, and platform-update orchestration are implemented. Automatic production update delivery is incomplete.

## Implemented behavior

- Updated bounds upload sizes/chunks, verifies direct update manifests through lib/update, and routes package installs and protected service replacement to appd.
- Microkernel and TEE images use platform-specific activation paths; committed generation floors are reconciled with appd. Updated itself has explicit migration records and a replacement archive.
- The feed manager can verify supplied TUF metadata and artifact bytes, select candidates, and stage/apply them.

## Gaps and deviations

- The live service initializes an empty feed. Its metadata/artifacts are seeded in memory; no production network repository polling path is wired into this service.
- Feed refresh passes time zero, so library expiry checks do not establish real-time freeze protection in that path. Serialized TUF state and heart-transplant checkpoints are not evidence of cold-boot durable feed recovery.
- Physical boot-slot persistence, secure-core continuity, postcommit recovery, and all kernel-candidate failure scenarios remain separate acceptance gaps; do not infer them from a successful upload.

## Sources and validation

Implementation and contract evidence: [services/updated/src/service.rs](../../../services/updated/src/service.rs), [services/updated/src/migration.rs](../../../services/updated/src/migration.rs), [lib/update](../../../lib/update), [lib/tuf/src/lib.rs](../../../lib/tuf/src/lib.rs), [services/appd/src/guest/update.rs](../../../services/appd/src/guest/update.rs).

Relevant test sources and Bazel targets: [services/updated/tests/updated_tests.rs](../../../services/updated/tests/updated_tests.rs), [testing/e2e/qemu/update/BUILD.bazel](../../../testing/e2e/qemu/update/BUILD.bazel).

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md), [secure integration validation](../../secure-integration-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
