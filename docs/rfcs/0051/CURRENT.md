# RFC 0051: Trusty security stack — current implementation

- Reviewed: 2026-09-26
- Repository revision: current working tree; see the linked validation record for commands and results.
- Design: [RFC 0051](README.md)

## Implementation summary

Trusty services, provider-neutral teed transport, ARM QEMU live/reboot Trusty
replacement, and substantial x86 secure-monitor integration exist. Final
cross-architecture secure-update acceptance remains incomplete.

## Implemented behavior

- The pinned firmware stack and typed clients cover KeyMint, Gatekeeper, storage, AVB, AuthMgr, and the BexOS lifecycle orchestrator. Teed selects the Trusty driver through signed package configuration.
- ARM has authenticated BL33 and RPMB-related boot verification. Its signed
  permanent S-EL2 owner retains secure stage-two mappings, A/B banks, protected
  selection, the source Trusty context, and a reserved watchdog interrupt while
  independently signed S-EL1 candidates execute.
- ARM live activation preserves normal-world processes and public TEE session
  IDs while replacing the private QL-TIPC transport. It probes storage, KeyMint,
  Gatekeeper, AVB, AuthMgr BE, and the lifecycle orchestrator before durable
  commit. Incompatible, faulting, and hanging candidates restore the exact
  suspended source transport. Reboot trials commit at the first healthy return.
- Live candidates use a QEMU-specific cloned authenticated RPMB backend until
  commitment, so AuthMgr/secure-storage initialization cannot mutate the source
  store. KeyMint's verified boot/HAL records are replayed before commitment and
  its shared secret is renegotiated before update success; those records are
  included in teed heart-transplant state.
- The repository also contains x86 EFI/SVM monitor, isolated-domain transport,
  protected firmware selection, and reboot recovery code; it is no longer only
  a standalone Trusty build.
- Teed and normal-world services implement heart transplant. The x86 live replacement boundary is a separately signed scheduling-policy image; a permanent nucleus retains hardware and guest state. Trusty core activation is reboot-only in that path.

## Gaps and deviations

- The secure integration record explicitly leaves the final E2E matrices incomplete. Earlier passing checkpoints must not be attributed to this baseline as final acceptance.
- Integrated-x86 live Trusty replacement, replacement of all resident monitor
  emulation, a physical-board equivalent of the transactional RPMB backend,
  physical provisioning, and secure ConfirmationUI are not completed.
- The ARM source-built debug product uses a 30 guest-second candidate-startup
  watchdog and a 5 guest-second post-readiness cutover window. The long-term
  150 ms cutover target in the RFC remains unmet and is not claimed by current
  QEMU acceptance.
- AuthMgr/secure-timer and platform-specific integration limits retain their separate evidence. Software-provider tests and a successful firmware build are not substitutes for secure runtime acceptance.

## Sources and validation

Implementation and contract evidence: [services/teed/src](../../../services/teed/src), [lib/tee_driver_trusty](../../../lib/tee_driver_trusty), [lib/trusty_client](../../../lib/trusty_client), [secure/orchestrator/trusty](../../../secure/orchestrator/trusty), [secure/monitor/runtime](../../../secure/monitor/runtime), [boot/efi](../../../boot/efi), [boot/bl33](../../../boot/bl33).

Relevant test sources and Bazel targets: [services/teed/BUILD.bazel](../../../services/teed/BUILD.bazel), [secure/orchestrator/BUILD.bazel](../../../secure/orchestrator/BUILD.bazel), [secure/monitor/BUILD.bazel](../../../secure/monitor/BUILD.bazel), [boot/efi/BUILD.bazel](../../../boot/efi/BUILD.bazel).

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md), [secure integration validation](../../secure-integration-validation.md), [x86_64 support](../../x86_64-support.md).

Runtime results for this working tree are recorded in
[secure integration validation](../../secure-integration-validation.md) and
[testing status](../../testing-status.md). Physical hardware and production
performance were not verified; linked historical results retain their original
scope.
