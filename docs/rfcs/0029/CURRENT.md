# RFC 0029: Origin-based packages and domain verification — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0029](README.md)

## Implementation summary

Origin validation, domain-policy caching, and install/refresh handlers exist. Live URL installation remains disabled in the public guest path.

## Implemented behavior

- Lib/domain_association stores decoded domain-policy records and cache metadata; appd serializes/persists cached associations and uses them for opener verification.
- AppManager parses strict HTTPS origins, enforces package/domain relationships, and has injectable fetch interfaces for archive installation and well-known refresh.
- Debugd/bexctl expose the management operations; appd migration preserves manager bindings and cache state.

## Gaps and deviations

- handle_app_manager_message constructs UnavailableFetcher for both InstallAppFromUrl and ReloadWellKnown. Host/injected fetcher behavior is not an enabled live service.
- DNS TXT, VMC/BIMI, certificate extensions, HTML meta tags, and code-host/OIDC identity verification are design alternatives without corresponding integrated verification paths.
- Best-effort association refresh and cached trust do not imply continuously verified domain ownership or a complete direct-from-web package distribution system.

## Sources and validation

Implementation and contract evidence: [lib/domain_association](../../../lib/domain_association), [services/appd/src/manager.rs](../../../services/appd/src/manager.rs), [idl/bexos/domain/association.proto](../../../idl/bexos/domain/association.proto), [idl/bexos/app/manager.fidl](../../../idl/bexos/app/manager.fidl), [lib/distribution/src/lib.rs](../../../lib/distribution/src/lib.rs).

Relevant test sources and Bazel targets: [lib/domain_association/BUILD.bazel](../../../lib/domain_association/BUILD.bazel), [lib/distribution/tests/distribution_tests.rs](../../../lib/distribution/tests/distribution_tests.rs), [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [cli](../../cli.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
