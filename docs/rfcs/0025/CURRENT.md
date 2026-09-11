# RFC 0025: TUF-backed update delivery — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0025](README.md)

## Implementation summary

The TUF verifier and a seeded update-feed integration exist, but the unified online delivery system is not production-complete.

## Implemented behavior

- Lib/tuf parses signed root/timestamp/snapshot/targets and delegation metadata, checks signature thresholds, version rollback, metadata references, and artifact hashes/lengths.
- ClientState can serialize accepted root/version state. Updated exposes candidate selection and stages verified supplied artifacts before calling appd/platform activation.
- Appd supplies package and service lifecycle coordination; updated has checkpoint state for its feed and update operations.

## Gaps and deviations

- The deployed feed is not wired to a live repository fetcher. A separate live HTTPS distribution library is not sufficient to activate it.
- Updated calls metadata refresh with now=0. Real current-time expiry enforcement and durable cold-boot recovery of TUF client state are not demonstrated by this integration.
- The RFC’s zero-downtime update of every application and automatic offline A/B boot-bundle synchronization are broader than current per-runner migration and durable service-archive selection.
- Production roots, repository operations, physical anti-rollback, and secure-core replacement acceptance remain deployment work.

## Sources and validation

Implementation and contract evidence: [lib/tuf/src/lib.rs](../../../lib/tuf/src/lib.rs), [services/updated/src/service.rs](../../../services/updated/src/service.rs), [services/updated/src/migration.rs](../../../services/updated/src/migration.rs), [lib/distribution/BUILD.bazel](../../../lib/distribution/BUILD.bazel), [services/appd/src/guest/update.rs](../../../services/appd/src/guest/update.rs).

Relevant test sources and Bazel targets: [lib/tuf/tests/tuf_tests.rs](../../../lib/tuf/tests/tuf_tests.rs), [services/updated/tests/updated_tests.rs](../../../services/updated/tests/updated_tests.rs).

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md), [secure integration validation](../../secure-integration-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
