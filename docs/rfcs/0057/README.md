# RFC 0057: Multiarchitecture kernel, loader, and packages

- Created: 2026-09-06T07:49:10-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Build-time architecture selection separates shared kernel policy from architecture-specific boot, execution, and migration code. Loader, TLS, syscall, and package rules extend the boundary into userspace.

## Design overview

BexOS selects its guest architecture at build time. `ArchAPI` and `CurrentArch`
separate shared kernel policy from AArch64 and x86_64 boot, saved-context,
MMU, interrupt, time, CPU, power and transplant implementations. Userspace
syscalls and TLS follow the same boundary. Guest assembly is enabled only in
BexOS builds; a host test must never issue a guest syscall.

Saved contexts carry an architecture identifier. Scheduling and IPC operate on
normalized syscall words and saved-context operations. Architecture entry code
owns concrete trap frames, register encodings and exception decoding. x86 starts
with the SSE2 userspace ABI and FXSAVE/FXRSTOR; enabling AVX or other extended
state requires a separately negotiated context and snapshot format.

Kernel transplant is same-architecture only. Validate the preparation ABI,
handoff version, architecture, image bounds and signed update before authorizing
a switch. Retain supported ARM legacy readers. Full snapshots and incremental
records carry explicit architecture/version envelopes. Secondary CPUs may park
only under the existing scheduler ownership restrictions; their trampoline,
page tables, descriptor tables and stacks must survive old-image reclamation.
Cross-architecture live migration is outside this design.

`//lib/elf` is a `no_std` crate using `alloc`. It owns ELF validation, mapping
plans, symbols, dynamic linking, relocations, constructors, RELRO and TLS layout.
Kernel and appd provide mapping backends; appd retains package authorization and
manifest dependency/SONAME policy. Executables and DSOs use the same dynamic
linker. Relocation operates on a zero-initialized memory image, including BSS,
and mappings acquire final permissions only after relocation. Cache keys must
include source content and relocated page content so executable-specific
bindings cannot leak through shared read-only VMOs.

AArch64 uses Variant I TLS. x86_64 uses Variant II with FS pointing to the TCB
and executable TLS immediately below it. Process-owned startup metadata supplies
TLS module offsets to `__tls_get_addr` and templates to newly created threads.
Unsupported relocation types, IFUNC, COPY, text relocations, symbol versions and
undeclared dependencies fail closed.

Manifest field 20 is `Architecture`: `MULTI = 0`, `AARCH64 = 1`, `X86_64 = 2`.
The native `app_manifest` build rule stamps the selected guest architecture
before signing. Portable content explicitly opts into `MULTI`, which cannot
provide native executables or native libraries. Conflicting source declarations
are errors. Config and source manifests remain prototxt; compiled manifests are
Bazel outputs.

Publish separate native BEX archives for each CPU while retaining package and
version identities. Archive creation, assembly, installation, dependency
selection, launch and replacement validate architecture. Unlabelled native
installations remain unavailable until rebuilt and reinstalled. Never infer
architecture from old payloads to modify signed manifests. Retain unavailable
records and pins for diagnosis and compatible reinstall without blocking other
packages.

Q35 development uses an explicit software-TEE product. Its security capabilities
remain those of the software provider; unavailable secure operations remain
unavailable. Integrated x86 Trusty, secure boot and hardware-backed acceptance
remain separate future work. Adding that integration must not turn the explicit
development fixtures into substitutes for secure-only acceptance tests.

Future hardware ports may add ACPI variants, additional interrupt controllers,
IOMMUs and device drivers behind these boundaries. Existing long-term driver,
security and scheduler designs remain applicable; architecture selection alone
does not implement those future features.
