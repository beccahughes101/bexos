# RFC 0011: Kernel hardware features and memory regions — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0011](README.md)

## Implementation summary

The kernel implements hardware-backed virtual memory, delegated VMARs, timing, and architecture-specific execution/hardening paths. The RFC’s complete hardware feature catalogue is not a portability guarantee.

## Implemented behavior

- AArch64 and x86_64 have architecture-selected trap, MMU, interrupt, and timer implementations. Kernel core provides shared CPU-feature, scheduler, and memory models.
- VMAR services allocate nested regions and control mappings; appd constructs ELF images, libraries, TLS, stacks, and guards through delegated VMARs. Lazy anonymous VMOs and resource accounting are integrated.
- AArch64 hardening tracks BTI/PAC policy and entropy prerequisites separately from detection. The kernel exports a read-only seqlock time page and realtime transform while timed owns synchronization policy.
- Kernel runtime migration serializes memory and task state; architecture-specific live handoff is distinct from ordinary context switching.

## Gaps and deviations

- The AArch64-first prose and “no wall clock in kernel” wording need interpretation: x86 code exists and the kernel does expose a userspace-managed realtime transform.
- RISC-V, universal CET/AVX/virtualization support, dynamic alternatives, and physical-board validation are not established by shared helper types or the feature roadmap.
- Feature detection, compiled protection, and activation are separate facts. Nanosecond timing estimates and broad Tier 1–3 completion claims require hardware-specific verification.

## Sources and validation

Implementation and contract evidence: [kernel/src/arch](../../../kernel/src/arch), [kernel/core/src/cpu_features.rs](../../../kernel/core/src/cpu_features.rs), [kernel/core/src/kernel_services/vmar.rs](../../../kernel/core/src/kernel_services/vmar.rs), [kernel/core/src/runtime/memory.rs](../../../kernel/core/src/runtime/memory.rs), [kernel/src/arch/aarch64/hardening.rs](../../../kernel/src/arch/aarch64/hardening.rs), [lib/time_abi](../../../lib/time_abi).

Relevant test sources and Bazel targets: [kernel/core/BUILD.bazel](../../../kernel/core/BUILD.bazel), [testing/build/architecture/BUILD.bazel](../../../testing/build/architecture/BUILD.bazel).

Detailed guides and previously recorded validation: [kernel](../../kernel.md), [x86_64 support](../../x86_64-support.md), [multiarchitecture validation](../../multiarchitecture-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
