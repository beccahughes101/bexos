# RFC 0057: Multiarchitecture kernel, loader, and packages — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0057](README.md)

## Implementation summary

AArch64 and x86_64 share kernel policy through explicit architecture boundaries, with matching loader, syscall, TLS, package, and migration checks.

## Implemented behavior

- ArchAPI/CurrentArch select boot, traps, MMU, CPU, timers, interrupts, power, and saved-context behavior. Host builds avoid issuing guest instructions.
- Lib/elf provides common executable/shared-object validation and relocation; native TLS uses architecture-specific layouts. Manifests stamp architecture before signing, and install/launch/dependency checks reject incompatible native content.
- Kernel snapshots/handoff carry architecture/version identity; live replacement is same-architecture. x86 begins with SSE2 context support.
- Beyond the earlier RFC checkpoint, integrated x86 EFI/SVM/Trusty transport and recovery code now exists, with explicit incomplete final acceptance records.

## Gaps and deviations

- Cross-architecture live migration, RISC-V and physical-board ports, general AVX/extended-state negotiation, and unsupported ELF relocations remain outside the implemented contract.
- Secondary-CPU ownership restrictions still constrain kernel transplant; ordinary SMP scheduling is not proof of unrestricted multi-CPU replacement.
- Integrated secure-x86 and explicit software-development products have different guarantees. The incomplete secure E2E matrix cannot be replaced by software-provider success.

## Sources and validation

Implementation and contract evidence: [kernel/src/arch/api.rs](../../../kernel/src/arch/api.rs), [kernel/src/arch/mod.rs](../../../kernel/src/arch/mod.rs), [lib/elf](../../../lib/elf), [lib/boot/lib.rs](../../../lib/boot/lib.rs), [tools/app_manifest](../../../tools/app_manifest), [services/appd/src/runner/elf/arch](../../../services/appd/src/runner/elf/arch), [secure/monitor](../../../secure/monitor).

Relevant test sources and Bazel targets: [kernel/core/tests/architecture_tests.rs](../../../kernel/core/tests/architecture_tests.rs), [testing/build/architecture/BUILD.bazel](../../../testing/build/architecture/BUILD.bazel), [lib/elf/BUILD.bazel](../../../lib/elf/BUILD.bazel).

Detailed guides and previously recorded validation: [multiarchitecture validation](../../multiarchitecture-validation.md), [x86_64 support](../../x86_64-support.md), [secure integration validation](../../secure-integration-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
