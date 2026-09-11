# RFC 0035: Application signing and trust stores — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0035](README.md)

## Implementation summary

Trustd and the archive verifier implement separate application-signing and TLS trust surfaces with development ecosystem packaging.

## Implemented behavior

- Bazel selects ecosystem inputs and builds public policy/root bundles. Signed v2 archives carry algorithm, chain, digest, and signature; appd verifies them before accepting manifests.
- Trustd parses ordered DER X.509 chains, verifies Ed25519/P-256 signatures, checks issuer/CA/key-usage/code-signing/time constraints, and applies revocation data.
- TLS root export is separate from app trust. Enterprise-root mutation is method gated; migration preserves dynamic trust/revocation state and bound clients.

## Gaps and deviations

- The checked-in production profile is a template, not deployable production signing infrastructure. Operational roots, endpoints, provisioning, and signer custody remain external requirements.
- The verifier supports its explicit algorithms and ordered-chain contract, not unrestricted Web PKI compatibility.
- Offline verification and host tests do not establish live revocation distribution, physical secure boot/RPMB, or every production time/bootstrap condition.

## Sources and validation

Implementation and contract evidence: [services/trustd/src](../../../services/trustd/src), [services/trustd/src/x509.rs](../../../services/trustd/src/x509.rs), [lib/app_archive](../../../lib/app_archive), [lib/trust_store](../../../lib/trust_store), [ecosystem/bexos](../../../ecosystem/bexos), [idl/bexos/security/trust.fidl](../../../idl/bexos/security/trust.fidl).

Relevant test sources and Bazel targets: [services/trustd/BUILD.bazel](../../../services/trustd/BUILD.bazel), [lib/app_archive/BUILD.bazel](../../../lib/app_archive/BUILD.bazel), [lib/trust_store/BUILD.bazel](../../../lib/trust_store/BUILD.bazel).

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md), [services](../../services.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
