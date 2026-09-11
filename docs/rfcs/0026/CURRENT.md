# RFC 0026: Cryptography and keychain services — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0026](README.md)

## Implementation summary

Bulk cryptography is implemented in libraries, while keychaind owns scoped secrets and KeyMint-backed key operations.

## Implemented behavior

- Lib/crypto supplies in-process primitives and a packaged native crypto library. Clients use the keychain FIDL for secret/key lifecycle rather than sending all bulk hashing through a daemon.
- Keychaind separates system and user vaults, retains encrypted envelopes and opaque hardware-key blobs, checks caller scope and authentication, and calls the TEE through its hardware provider.
- Runtime/migration modules preserve service bindings and vault metadata; lazy service activation is supported. User locking controls access to nonzero-UID stores.

## Gaps and deviations

- The service is a singleton with scoped vaults, not a separate process for every user.
- Biometric/secure confirmation prompts, broader key algorithms/providers, and physical production key provisioning are not established by the v1 password/KeyMint path.
- Hardware-backed flags depend on the configured TEE provider; software-provider tests do not prove keys are physically non-exportable.

## Sources and validation

Implementation and contract evidence: [lib/crypto](../../../lib/crypto), [lib/crypto_client](../../../lib/crypto_client), [services/keychaind/src/service.rs](../../../services/keychaind/src/service.rs), [services/keychaind/src/migration.rs](../../../services/keychaind/src/migration.rs), [services/keychaind/package/keychaind.prototxt](../../../services/keychaind/package/keychaind.prototxt), [idl/bexos/security/keychain.fidl](../../../idl/bexos/security/keychain.fidl).

Relevant test sources and Bazel targets: [services/keychaind/tests](../../../services/keychaind/tests), [lib/keychain_store/BUILD.bazel](../../../lib/keychain_store/BUILD.bazel), [lib/crypto/BUILD.bazel](../../../lib/crypto/BUILD.bazel).

Detailed guides and previously recorded validation: [services](../../services.md), [secure runtime updates](../../secure-runtime-updates.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
