# RFC 0019: Debugd-installed test applications — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0019](README.md)

## Implementation summary

Signed archive upload and launch through debugd are implemented, but complete removal of verifier packages from base images has not occurred.

## Implemented behavior

- BXD1 bundle operations buffer bounded upload chunks and forward committed archives to appd for verification, installation, and normal package launch.
- Host helpers and Bazel e2e targets provide scenario artifacts. The older loose manifest/ELF installer remains a limited test abstraction rather than the authoritative package-install path.
- Debugd checkpointing includes partial upload state; appd remains the registry/policy owner.

## Gaps and deviations

- Contrary to the RFC’s stated image policy, device/base and both architecture system-image manifests still include bexos.platform.storage_verify as a system package.
- Therefore the upload mechanism is implemented but the “all verifier applications are test-only artifacts” objective is incomplete.
- Existing upload and transplant tests do not establish that every e2e scenario exclusively uses the signed-bundle path.

## Sources and validation

Implementation and contract evidence: [services/debugd/src/service/apps.rs](../../../services/debugd/src/service/apps.rs), [services/debugd/src/service/buffered_apps.rs](../../../services/debugd/src/service/buffered_apps.rs), [services/debugd/src/service/test_apps.rs](../../../services/debugd/src/service/test_apps.rs), [host/debug_client/src/uploads.rs](../../../host/debug_client/src/uploads.rs), [device/base/base.aib.prototxt](../../../device/base/base.aib.prototxt), [device/virtual/qemu/base/aarch64/system_image.prototxt](../../../device/virtual/qemu/base/aarch64/system_image.prototxt).

Relevant test sources and Bazel targets: [services/debugd/tests/debugd_tests.rs](../../../services/debugd/tests/debugd_tests.rs), [host/debug_client/tests](../../../host/debug_client/tests), [testing/e2e/qemu/BUILD.bazel](../../../testing/e2e/qemu/BUILD.bazel).

Detailed guides and previously recorded validation: [storage preinstalled apps](../../storage-preinstalled-apps.md), [cli](../../cli.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
