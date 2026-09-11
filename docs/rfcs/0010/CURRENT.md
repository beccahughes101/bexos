# RFC 0010: Board and platform configuration — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0010](README.md)

## Implementation summary

Board, product, and platform policy are represented by prototxt sources and Bazel assembly. Maintained virtual products cover AArch64 and x86_64 with different secure execution limits.

## Implemented behavior

- Platform/device/assembly protobufs define hardware, package placement, runners, TEE policy, and driver access. QEMU has per-architecture configuration and shared nongui/workstation product bundles.
- Appd decodes platform policy and applies package/signer and runner/hardware rules to launch and driver binding. Kernel process/resource operations enforce delegated authority.
- Configuration and manifests are compiled as build artifacts. Appd checkpoints policy and launch state during heart transplant.

## Gaps and deviations

- Permitted runner names do not imply implemented launchers: Web, Android, Nix, and MicroVM execution remain unsupported in the current runner registry.
- Virtual-product configuration does not demonstrate a physical-board port or production secure provisioning. x86 secure integration must be read with its explicit development and validation limits.
- The proposed monotonic platform-policy replacement and cold-boot rollback behavior is broader than merely carrying policy through an appd checkpoint; end-to-end deployment acceptance remains separate.

## Sources and validation

Implementation and contract evidence: [idl/bexos/platform](../../../idl/bexos/platform), [build/rules/assembly.bzl](../../../build/rules/assembly.bzl), [tools/assembly](../../../tools/assembly), [device/virtual/qemu/base](../../../device/virtual/qemu/base), [services/appd/src/platform_config.rs](../../../services/appd/src/platform_config.rs), [services/appd/src/runner/policy.rs](../../../services/appd/src/runner/policy.rs).

Relevant test sources and Bazel targets: [tools/assembly/BUILD.bazel](../../../tools/assembly/BUILD.bazel), [services/appd/tests/firmware_policy_tests.rs](../../../services/appd/tests/firmware_policy_tests.rs), [testing/build/architecture/BUILD.bazel](../../../testing/build/architecture/BUILD.bazel).

Detailed guides and previously recorded validation: [build assembly tooling](../../build-assembly-tooling.md), [x86_64 support](../../x86_64-support.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
