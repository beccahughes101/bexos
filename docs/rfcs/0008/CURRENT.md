# RFC 0008: Storage layers and filesystem capabilities — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0008](README.md)

## Implementation summary

The maintained storage path uses D1 block drivers, BexFS, ArchiveFS, MemFS, and capability-scoped namespaces. Several broader filesystem and hardware choices remain proposals.

## Implemented behavior

- NVMe exposes registered buffers and block FIFO requests. BexFS restricts access to GPT partition extents and adapts block transfers to its roseFS-backed filesystem.
- ArchiveFS serves immutable package files; the shared archive library verifies signed v2 archives and payload integrity. MemFS supplies temporary roots; diskimage provides file-backed block storage for user volumes.
- Appd supplies /pkg, /data, /tmp, /shared, and declared /deps handles, with access decided before ordinary file operations. Storage drivers and vfsd have explicit migration adapters preserving endpoint and mount state.

## Gaps and deviations

- The diagram’s separate BlobFS package partition is not the shipped package store: signed archives live on BexFS STORAGE and are exposed by ArchiveFS.
- AHCI, SD/eMMC, broad filesystem compatibility, automatic removable-media mounting, and picker-minted file access are not supplied by this storage path. USB mass storage is a separate limited implementation.
- Zero-copy and throughput claims require per-driver measurement; the QEMU stack includes copying, bounded queues, and polling.

## Sources and validation

Implementation and contract evidence: [idl/bexos/storage/block.fidl](../../../idl/bexos/storage/block.fidl), [idl/bexos/fs/fs.fidl](../../../idl/bexos/fs/fs.fidl), [drivers/d1/storage/nvmexpress/nvme/src](../../../drivers/d1/storage/nvmexpress/nvme/src), [drivers/d1/storage/bexos](../../../drivers/d1/storage/bexos), [lib/app_archive](../../../lib/app_archive), [services/appd/src/namespace.rs](../../../services/appd/src/namespace.rs).

Relevant test sources and Bazel targets: [drivers/d1/storage/nvmexpress/nvme/tests](../../../drivers/d1/storage/nvmexpress/nvme/tests), [drivers/d1/storage/bexos/bexfs/tests](../../../drivers/d1/storage/bexos/bexfs/tests), [drivers/d1/storage/bexos/archivefs/tests](../../../drivers/d1/storage/bexos/archivefs/tests).

Detailed guides and previously recorded validation: [drivers storage](../../drivers-storage.md), [usb](../../usb.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
