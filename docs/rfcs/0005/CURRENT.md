# RFC 0005: Provider-neutral TEE integration — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0005](README.md)

## Implementation summary

Teed implements provider-neutral session and trusted-package orchestration over external ABI-v1 TEE drivers. Trusty and explicit software development providers exist.

## Implemented behavior

- The external driver client defines versioned probe, session, invocation, app-load, and core-update operations. Teed selects a signed manifest dependency and handles the public TeeManager contract.
- Trusty transport resides in its driver/library stack; kernel secure-monitor access is capability gated. The software provider is a separate configured package, not an implicit production fallback.
- Teed migration retains catalog/session metadata and resource ownership, reconnecting provider endpoints rather than serializing secure-world private objects.

## Gaps and deviations

- The RFC is a portability boundary, not an implementation of every TEE: no OP-TEE, TPM-backed equivalent, or physical-board acceptance is established here.
- Rebinding the driver can fail pending operations and recreate sessions; secure-app private volatile state is not guaranteed to survive.
- Integrated secure execution and live core replacement have unresolved acceptance gates documented in the secure integration record. Software tests do not establish physical rollback protection or ConfirmationUI.

## Sources and validation

Implementation and contract evidence: [lib/tee_driver_client/src/lib.rs](../../../lib/tee_driver_client/src/lib.rs), [lib/tee_driver_trusty](../../../lib/tee_driver_trusty), [lib/tee_driver_software](../../../lib/tee_driver_software), [services/teed/src](../../../services/teed/src), [idl/bexos/tee/manager.fidl](../../../idl/bexos/tee/manager.fidl), [kernel/src/trusty.rs](../../../kernel/src/trusty.rs).

Relevant test sources and Bazel targets: [services/teed/BUILD.bazel](../../../services/teed/BUILD.bazel), [lib/tee_driver_client/BUILD.bazel](../../../lib/tee_driver_client/BUILD.bazel).

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md), [secure integration validation](../../secure-integration-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
