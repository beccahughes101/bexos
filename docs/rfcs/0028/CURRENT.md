# RFC 0028: Shared libraries and package dependencies — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0028](README.md)

## Implementation summary

The package system supports native shared-library dependencies and WASM component libraries with declared version/ABI requirements.

## Implemented behavior

- LIBRARY packages carry native or WASM_COMPONENT exports. Appd resolves dependency manifests and grants private read-only /deps paths.
- The native ELF loader validates and maps declared shared objects, handles symbols/relocations/TLS, and passes linker metadata; the TEE and crypto packages use this mechanism.
- WASM composition resolves signed component graphs before launch; the shared Dioxus component is a real product consumer.
- Library packages have no standalone process migration. Consumer/runner replacement owns any live state and rebinds or recomposes its dependencies.

## Gaps and deviations

- Native loading targets the BexOS ABI, not arbitrary Linux/glibc compatibility. Version matching is a bounded implementation, not a general package SAT solver.
- Early BootFS services use retained resolver inputs; stable dependency-directory proxy pivoting is not complete.
- React Native/Flutter/QuickJS bridges and the illustrative crypto/parser WIT packages are not all provided. Installing a new library does not hot-swap every running consumer automatically.

## Sources and validation

Implementation and contract evidence: [idl/bexos/app/manifest.proto](../../../idl/bexos/app/manifest.proto), [services/appd/src/runner/elf](../../../services/appd/src/runner/elf), [services/appd/src/namespace.rs](../../../services/appd/src/namespace.rs), [services/appd/src/runner/wasm.rs](../../../services/appd/src/runner/wasm.rs), [lib/wasm_runtime/src/component.rs](../../../lib/wasm_runtime/src/component.rs), [apps/dioxus_shared](../../../apps/dioxus_shared).

Relevant test sources and Bazel targets: [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs), [services/appd/tests/wasm_tests.rs](../../../services/appd/tests/wasm_tests.rs), [lib/elf/BUILD.bazel](../../../lib/elf/BUILD.bazel).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [dioxus](../../dioxus.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
