# RFC 0009: Storage bootstrap and partition layout — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0009](README.md)

## Implementation summary

QEMU implements external BootFS bootstrap, encrypted BexFS SYS_STATE/STORAGE, and a pivot to persistent packages. The production cold-boot A/B design is not fully realized.

## Implemented behavior

- Image tooling emits ESP, BOOT_A, BOOT_B, SYS_STATE, and STORAGE GPT entries. The current QEMU SYS_STATE and STORAGE sizes are 32 MiB and 512 MiB; STORAGE reserves two complete BexFS namespace snapshots and fits the larger x86_64 service/replacement archives with runtime-write headroom.
- The kernel validates handoff/BootFS inputs; appd waits for storage readiness, mounts through the storage stack, retains required launch inputs, and reclaims BootFS when the pivot succeeds.
- Signed v2 package archives, durable app-registry replacement selections, and accepted generations are now implemented. These supersede the RFC’s earlier claims that package verification and live-update persistence were only future work.
- Boot drivers have heart transplant adapters; boot-state persistence and service archive selection are distinct from rewriting every cold-boot image.

## Gaps and deviations

- GPT slot names do not prove a deployed production boot manager with signed A/B bundle selection, hardware watchdog fallback, or automatic BootFS regeneration after each service transplant.
- QEMU uses development key/firmware contracts. Physical provisioning, removable hardware coverage, and production anti-rollback require separate evidence.
- The v1 archive layout and several bring-up limitations in the design are historical; current archive verification uses v2.

## Sources and validation

Implementation and contract evidence: [tools/image/generate_gpt_disk.py](../../../tools/image/generate_gpt_disk.py), [tools/image/boot_handoff.rs](../../../tools/image/boot_handoff.rs), [drivers/d1/storage/bexos/bexfs/src/sys_state.rs](../../../drivers/d1/storage/bexos/bexfs/src/sys_state.rs), [services/appd/src/guest/mod.rs](../../../services/appd/src/guest/mod.rs), [services/appd/src/guest/update.rs](../../../services/appd/src/guest/update.rs), [lib/app_archive](../../../lib/app_archive).

Relevant test sources and Bazel targets: [tools/image/BUILD.bazel](../../../tools/image/BUILD.bazel), [services/storage_verify/BUILD.bazel](../../../services/storage_verify/BUILD.bazel), [testing/e2e/qemu/update/BUILD.bazel](../../../testing/e2e/qemu/update/BUILD.bazel).

Detailed guides and previously recorded validation: [drivers storage](../../drivers-storage.md), [qemu product](../../qemu-product.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
