# RFC 0007: Dependency-ordered service startup — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0007](README.md)

## Implementation summary

Appd implements numeric startup waves with readiness barriers, plus lazy activation for selected services. It does not implement a general dependency-DAG scheduler.

## Implemented behavior

- StartupPlan distinguishes omitted wave from wave zero, groups automatic processes, and sorts numeric waves. A ReadinessGate is required before advancing.
- Lazy service providers and shell-role processes are excluded from the ordinary wave plan and launched through their dedicated paths.
- Product manifests supply actual wave membership; appd boot/readiness and driver-registration code integrate the plan with discovered devices and persistent storage. Appd migration retains managed service state rather than rerunning cold boot.

## Gaps and deviations

- The RFC’s illustrative four-wave layout is not the complete product schedule: current manifests include later service waves and special bootstrap/shell/lazy paths.
- No general topological scheduling of arbitrary declared dependencies was found. Lazy activation implements a narrower optimization; see RFC 0059.
- Readiness tests and manifest ordering do not measure boot latency or establish physical-device initialization behavior.

## Sources and validation

Implementation and contract evidence: [services/appd/src/waves.rs](../../../services/appd/src/waves.rs), [services/appd/src/guest/readiness.rs](../../../services/appd/src/guest/readiness.rs), [services/appd/src/lazy.rs](../../../services/appd/src/lazy.rs), [device/virtual/qemu/base/aarch64/bootfs_manifest.prototxt](../../../device/virtual/qemu/base/aarch64/bootfs_manifest.prototxt), [device/base/base.aib.prototxt](../../../device/base/base.aib.prototxt).

Relevant test sources and Bazel targets: [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs), [services/appd/tests/lazy_tests.rs](../../../services/appd/tests/lazy_tests.rs).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [qemu product](../../qemu-product.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
