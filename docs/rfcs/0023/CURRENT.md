# RFC 0023: Structured permissions — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0023](README.md)

## Implementation summary

Structured declarations, scoped grants, encrypted per-user persistence, and capability filtering are implemented. Trusted user consent is still missing.

## Implemented behavior

- Manifests encode permission name, allowed values, required/optional status, and usage text. Runtime scope requests must fit the signed declaration.
- Appd combines package/process declarations, auto-grants required permissions, handles optional requests, and passes allowed scopes/methods to bound providers.
- System grants and nonzero-UID grants use separate stores; user lock/delete purges cached grants and revokes routes. Appd migration preserves permission and binding state.

## Gaps and deviations

- Required permissions and valid optional requests are granted without a trusted consent UI; declaring a permission is not evidence of a user approval interaction.
- The policy evaluator supports a bounded expression subset, not a general CEL interpreter. Method filtering requires providers to honor the supplied binding contract.
- The legacy string manifest format is intentionally incompatible. Generic permission plumbing does not imply every proposed hardware/application provider exists.

## Sources and validation

Implementation and contract evidence: [idl/bexos/app/manifest.proto](../../../idl/bexos/app/manifest.proto), [lib/permission_store/src/lib.rs](../../../lib/permission_store/src/lib.rs), [services/appd/src/permission_persistence.rs](../../../services/appd/src/permission_persistence.rs), [services/appd/src/permission_route.rs](../../../services/appd/src/permission_route.rs), [services/appd/src/policy.rs](../../../services/appd/src/policy.rs), [services/appd/src/lifecycle.rs](../../../services/appd/src/lifecycle.rs).

Relevant test sources and Bazel targets: [lib/permission_store/BUILD.bazel](../../../lib/permission_store/BUILD.bazel), [services/appd/tests/launch_permission_tests.rs](../../../services/appd/tests/launch_permission_tests.rs).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
