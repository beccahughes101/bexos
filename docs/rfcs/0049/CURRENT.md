# RFC 0049: Boot handoff ABI — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0049](README.md)

## Implementation summary

The current shared handoff ABI is version 5, with legacy compatibility and optional secure-monitor/framebuffer fields. The RFC’s v3-only description is an earlier baseline.

## Implemented behavior

- Lib/boot defines architecture-selected addresses, memory/update reservations, BootFS and evidence ranges, CPU topology, entropy, monitor metadata, and framebuffer metadata.
- Compatibility normalization handles older layouts, including the historical v4 graphics/monitor variants, and clears absent extensions. Image tooling can still emit v3 for ordinary no-framebuffer images.
- Boot entry adapters and kernel validation check ranges/overlap/version and delegate immutable BootFS and optional framebuffer resources before appd starts.
- The update reservation is retained for kernel replacement/snapshot work; it is not itself a whole-system update protocol.

## Gaps and deviations

- Consumers must not assume the sixteen-word v3 record is the latest layout or copy architecture-specific fixed addresses to another product.
- Optional framebuffer fields and boot adapters do not prove a production A/B boot manager or every FDT/ACPI/EFI board path.
- Secure execution and hardware rollback guarantees depend on verified evidence and the selected firmware path; a structurally valid handoff alone is insufficient.

## Sources and validation

Implementation and contract evidence: [lib/boot/lib.rs](../../../lib/boot/lib.rs), [lib/boot/handoff_compat.rs](../../../lib/boot/handoff_compat.rs), [lib/boot/framebuffer.rs](../../../lib/boot/framebuffer.rs), [tools/image/boot_handoff.rs](../../../tools/image/boot_handoff.rs), [boot/multiboot/loader.c](../../../boot/multiboot/loader.c), [boot/bl33/main.rs](../../../boot/bl33/main.rs).

Relevant test sources and Bazel targets: [lib/boot/BUILD.bazel](../../../lib/boot/BUILD.bazel), [tools/image/BUILD.bazel](../../../tools/image/BUILD.bazel), [kernel/tests/image_validation.rs](../../../kernel/tests/image_validation.rs), [boot/multiboot/BUILD.bazel](../../../boot/multiboot/BUILD.bazel).

Detailed guides and previously recorded validation: [bootui](../../bootui.md), [x86_64 support](../../x86_64-support.md), [multiarchitecture validation](../../multiarchitecture-validation.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
