# RFC 0006: User identity, encrypted homes, and keychains — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0006](README.md)

## Implementation summary

Local password authentication, encrypted user homes, and a separate keychain service are implemented; the broader multi-factor and cloud identity design is partial.

## Implemented behavior

- Usersd enrolls/verifies passwords through its authentication backend, stores Gatekeeper handles and auth-bound KeyMint key blobs, derives U-KEK after verification, and supplies a temporary key VMO to vfsd.
- Lock revokes filesystem access and invalidates cached authentication state. Vfsd provides per-user backing volumes and per-package data directories.
- Keychaind is a separate singleton with encrypted secret envelopes and opaque KeyMint blobs. Usersd, keychaind, and vfsd each have migration state and HEART_TRANSPLANT manifests; shell login uses the same usersd authentication path.

## Gaps and deviations

- Biometrics, passkeys, cloud SSO/OIDC token brokering, cached enterprise login, and per-user keychain processes remain unimplemented extensions.
- The normative interfaces are UserManager and the security keychain FIDL, not the illustrative AccountManager sketch.
- QEMU/software-provider coverage must not be described as physical hardware-rooted provisioning or proof of every lock/suspend key-erasure property.

## Sources and validation

Implementation and contract evidence: [services/usersd/src/service.rs](../../../services/usersd/src/service.rs), [services/usersd/src/auth.rs](../../../services/usersd/src/auth.rs), [services/usersd/src/migration.rs](../../../services/usersd/src/migration.rs), [services/keychaind/src/service.rs](../../../services/keychaind/src/service.rs), [services/vfsd/src/guest.rs](../../../services/vfsd/src/guest.rs), [idl/bexos/user/manager.fidl](../../../idl/bexos/user/manager.fidl).

Relevant test sources and Bazel targets: [services/usersd/tests/users_tests.rs](../../../services/usersd/tests/users_tests.rs), [services/keychaind/tests](../../../services/keychaind/tests), [services/vfsd/tests](../../../services/vfsd/tests).

Detailed guides and previously recorded validation: [services](../../services.md), [sysui](../../sysui.md), [secure runtime updates](../../secure-runtime-updates.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
