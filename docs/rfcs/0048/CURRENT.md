# RFC 0048: Process heaps and virtual memory — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0048](README.md)

## Implementation summary

The active process heap uses VMAR reservations and anonymous VMOs with lazy physical commitment; userspace allocators grow within that boundary.

## Implemented behavior

- Lib/boot defines a 64 GiB heap VMAR and 2 MiB arena chunks. Appd maps ELF/library/TLS/stack regions separately, including guards.
- Kernel anonymous-memory reads use a shared zero page; first writes and kernel copies commit backing pages with resource-group accounting. DMA materialization follows a distinct path.
- The userspace/libc allocator grows via VMOs rather than brk/sbrk. Runtime snapshots track backing kind and committed-page changes so migration can preserve heap contents.

## Gaps and deviations

- The RFC’s 1 TiB reservation and mimalloc/jemalloc integration are future frontend choices; the current allocator is the in-repository implementation.
- Lazy commitment is not a complete swap, page-reclamation, or overcommit policy, and DMA can require eager backing.
- Virtual reservation size is not usable physical capacity. Fragmentation, exhaustion, large allocations, and transplant behavior require the corresponding runtime tests and measurements.

## Sources and validation

Implementation and contract evidence: [lib/boot/lib.rs](../../../lib/boot/lib.rs), [lib/allocator/lib.rs](../../../lib/allocator/lib.rs), [lib/bexos_libc/src/lib.rs](../../../lib/bexos_libc/src/lib.rs), [kernel/core/src/runtime/memory.rs](../../../kernel/core/src/runtime/memory.rs), [kernel/core/src/runtime/snapshot.rs](../../../kernel/core/src/runtime/snapshot.rs), [services/appd/src/runner/elf](../../../services/appd/src/runner/elf).

Relevant test sources and Bazel targets: [lib/bexos_libc/tests/libc_tests.rs](../../../lib/bexos_libc/tests/libc_tests.rs), [kernel/core/tests/runtime_snapshot_tests.rs](../../../kernel/core/tests/runtime_snapshot_tests.rs), [kernel/core/tests/allocation_failure.rs](../../../kernel/core/tests/allocation_failure.rs), [lib/allocator/BUILD.bazel](../../../lib/allocator/BUILD.bazel).

Detailed guides and previously recorded validation: [kernel](../../kernel.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
