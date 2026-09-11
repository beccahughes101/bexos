# RFC 0018: Static asset packages — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0018](README.md)

## Implementation summary

Process-free library packages and declared dependency mounts provide the basic static-asset mechanism. The example asset distribution ecosystem is not shipped as described.

## Implemented behavior

- Manifest validation accepts LIBRARY packages without processes; dependency declarations include version requirements and mount aliases.
- Appd resolves declared package dependencies and builds read-only /deps namespace entries. Assets can use ordinary signed archives and ArchiveFS directory access.
- Resource groups are process CPU/memory/GPU accounting declarations; they are not an implemented protocol for selecting subsets of asset files. Static packages have no process state to transplant.

## Gaps and deviations

- The RFC’s CA, timezone, icon, and model package names are examples; no complete product set matching those examples was found.
- Updates to dependency packages do not imply atomic remapping of all running consumers. Early BootFS dependency-directory proxy pivoting remains a separate gap.
- The claim of one identical handle and zero duplication across all consumers is not a measured cache/storage invariant.

## Sources and validation

Implementation and contract evidence: [idl/bexos/app/manifest.proto](../../../idl/bexos/app/manifest.proto), [services/appd/src/manifest.rs](../../../services/appd/src/manifest.rs), [services/appd/src/namespace.rs](../../../services/appd/src/namespace.rs), [services/appd/src/runner/mod.rs](../../../services/appd/src/runner/mod.rs), [drivers/d1/storage/bexos/archivefs](../../../drivers/d1/storage/bexos/archivefs).

Relevant test sources and Bazel targets: [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs), [lib/app_manifest/BUILD.bazel](../../../lib/app_manifest/BUILD.bazel).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [build assembly tooling](../../build-assembly-tooling.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
