# RFC 0021: Storage and VFS broker — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0021](README.md)

## Implementation summary

Vfsd is the mount/package-store broker and appd supplies scoped namespace capabilities. Ordinary filesystem operations go to the directory/file providers.

## Implemented behavior

- The VFS manager exposes package, user/system data, home, temporary, and shared-vault directory operations. Appd chooses grants and launch namespaces.
- Vfsd coordinates BexFS, ArchiveFS, MemFS, and diskimage resources, including user unlock/lock and backing-volume management.
- Its migration adapter preserves mounted roots, user volume metadata/key VMOs, retained driver endpoints, and bound clients so adoption does not repeat cold mounts.

## Gaps and deviations

- The proposed general volume discovery/automount, external filesystem selection, file picker, and media-indexer grant flows are not implemented by the current manager contract.
- USB block publication does not automatically provide removable filesystem mounts to apps.
- The sketch’s full layered overlay namespace should not be confused with the concrete per-package directory and file-backed user-volume implementation.

## Sources and validation

Implementation and contract evidence: [idl/bexos/vfs/manager.fidl](../../../idl/bexos/vfs/manager.fidl), [services/vfsd/src/guest.rs](../../../services/vfsd/src/guest.rs), [services/vfsd/src/guest/migration.rs](../../../services/vfsd/src/guest/migration.rs), [services/vfsd/src/lib.rs](../../../services/vfsd/src/lib.rs), [services/appd/src/namespace.rs](../../../services/appd/src/namespace.rs).

Relevant test sources and Bazel targets: [services/vfsd/tests/vfsd_tests.rs](../../../services/vfsd/tests/vfsd_tests.rs), [drivers/d1/storage/bexos/diskimage/tests](../../../drivers/d1/storage/bexos/diskimage/tests).

Detailed guides and previously recorded validation: [drivers storage](../../drivers-storage.md), [appd userspace](../../appd-userspace.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
