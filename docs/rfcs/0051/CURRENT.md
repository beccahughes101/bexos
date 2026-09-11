# RFC 0051: Trusty security stack — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0051](README.md)

## Implementation summary

Trusty services, provider-neutral teed transport, and substantial x86 secure-monitor integration exist. Final secure-update acceptance remains incomplete.

## Implemented behavior

- The pinned firmware stack and typed clients cover KeyMint, Gatekeeper, storage, AVB, AuthMgr, and the BexOS lifecycle orchestrator. Teed selects the Trusty driver through signed package configuration.
- ARM has authenticated BL33 and RPMB-related boot verification. The repository also contains x86 EFI/SVM monitor, isolated-domain transport, protected firmware selection, and reboot recovery code; it is no longer only a standalone Trusty build.
- Teed and normal-world services implement heart transplant. The x86 live replacement boundary is a separately signed scheduling-policy image; a permanent nucleus retains hardware and guest state. Trusty core activation is reboot-only in that path.

## Gaps and deviations

- The secure integration record explicitly leaves the final E2E matrices incomplete. Earlier passing checkpoints must not be attributed to this baseline as final acceptance.
- Live Trusty private-state replacement, replacement of all resident monitor emulation, physical-board provisioning/RPMB, and secure ConfirmationUI are not completed.
- AuthMgr/secure-timer and platform-specific integration limits retain their separate evidence. Software-provider tests and a successful firmware build are not substitutes for secure runtime acceptance.

## Sources and validation

Implementation and contract evidence: [services/teed/src](../../../services/teed/src), [lib/tee_driver_trusty](../../../lib/tee_driver_trusty), [lib/trusty_client](../../../lib/trusty_client), [secure/orchestrator/trusty](../../../secure/orchestrator/trusty), [secure/monitor/runtime](../../../secure/monitor/runtime), [boot/efi](../../../boot/efi), [boot/bl33](../../../boot/bl33).

Relevant test sources and Bazel targets: [services/teed/BUILD.bazel](../../../services/teed/BUILD.bazel), [secure/orchestrator/BUILD.bazel](../../../secure/orchestrator/BUILD.bazel), [secure/monitor/BUILD.bazel](../../../secure/monitor/BUILD.bazel), [boot/efi/BUILD.bazel](../../../boot/efi/BUILD.bazel).

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md), [secure integration validation](../../secure-integration-validation.md), [x86_64 support](../../x86_64-support.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
