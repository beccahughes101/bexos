# RFC 0050: User-created applications — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0050](README.md)

## Implementation summary

Signed package tooling, user-scoped namespaces, and WASM sandboxing exist, but the user-created application workflow is not implemented.

## Implemented behavior

- Bazel builds signed app archives; appd installs and launches supported packages with scoped service grants and per-user data directories.
- WASM fuel/memory limits and restricted children can isolate computation. Keychaind/TEE provide key operations usable by a future local signing authority.

## Gaps and deviations

- No user_created:<uid>: identity policy, per-user local certificate issuer, creation-time consent flow, local AI/IDE packager service, or transient-app cleanup transaction matching this design was found.
- PWA/web execution is unsupported by the current runner registry. Normal package signing does not establish a hardware-bound user root or its revocation semantics.
- Existing WASM migration requires supported service/checkpoint contracts; arbitrary generated apps have no automatic continuity guarantee. The proposed special storage path is not the ordinary vfsd package-data layout.

## Sources and validation

Implementation and contract evidence: [tools/app_archive](../../../tools/app_archive), [services/appd/src/runner/mod.rs](../../../services/appd/src/runner/mod.rs), [services/appd/src/namespace.rs](../../../services/appd/src/namespace.rs), [services/vfsd/src/lib.rs](../../../services/vfsd/src/lib.rs), [services/keychaind/src/service.rs](../../../services/keychaind/src/service.rs), [lib/wasm_runtime/src/budget.rs](../../../lib/wasm_runtime/src/budget.rs).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [wasm runtime](../../wasm-runtime.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
