# x86_64 support

ARM remains the default guest. `--config=aarch64` and `--config=x86_64`
select the guest independently of the Bazel execution host. BexOS has a Q35
x86 development product and an integrated EFI/SVM product using real Trusty.
The integrated secure-product checkpoint passed two boots, persistence, WASI
and explicit secure-boot rejection cases in 765.4 s on 2026-09-07. Its maintained
E2E harness integration and full final matrix are still under validation.
The saved product now uses the resident recovery nucleus. Distinct monitor
policies have committed live with fault/hang rollback, retained secure clients
and old-memory reuse. Reboot selection and interrupted-update recovery are
implemented, with complete product/matrix validation still in progress. Live
Trusty replacement is excluded; Trusty updates activate on reboot. The separate
explicit development product uses the software TEE.

Both `nongui` and `workstation` share this architecture support. Workstation
adds native QEMU graphics/input devices; neither standard product substitutes
software TEE for integrated Trusty. Use `nongui:run_emulated` explicitly for
the x86 development profile. See [QEMU products](qemu-product.md).

## Architecture boundary

`kernel/src/arch/api.rs` defines `ArchAPI`; `CurrentArch` selects AArch64 or
x86 at compile time. Boot, saved frames, page tables, interrupts, CPU control,
time, console, power and transplant implementation live beneath their
architecture directories. Shared scheduler, IPC and syscall policy uses
normalized saved contexts. Guest entry and syscall assembly is gated by
`bexos_guest`, so host tests use host implementations.

Q35 enters through the Multiboot adapter, establishes long mode, and discovers
ACPI LAPIC, IOAPIC, HPET and PCI ECAM data. The kernel uses per-CPU GDT/TSS/IDT,
four-level paging, NX and user permissions, LAPIC scheduling timers and
reschedule IPIs. Userspace uses the baseline SSE2 ABI and FXSAVE/FXRSTOR, with
Variant II TLS addressed through FS. AVX and unsupported extended vector
state are not enabled.

## Products and commands

The explicit `virtual_x86_64_development_product_assembly` selects the
software-TEE manifest and development platform configuration. Its separate
`qemu_nvme_development_gpt.img` includes the software TEE driver package used
for replacement loading after BootFS reclamation. Q35 excludes
PL011/PL031, uses CMOS RTC, and exposes debug RPC through virtio-console's
`debug0` port. COM1 carries kernel and userspace diagnostic output. The
harness keeps those channels separate and owns its QEMU child lifetime.

```sh
bazel build -c opt --config=x86_64 //kernel:kernel //kernel:update_kernel
bazel run -c opt --config=x86_64 //device/virtual/qemu/nongui:run_emulated
bazel run --config=x86_64 //device/virtual/qemu/nongui:debugd -- health
bazel test --config=e2e //testing/e2e/qemu:x86_64_development
```

Stop the developer instance with Ctrl-C. Its default debug socket is
`/tmp/bexos-qemu-nongui-x86_64-debugd.sock`. The development harness does not start
an RPMB helper. The integrated product uses pinned OVMF with SMM-protected
variables and an authenticated EFI monitor. It runs four BexOS virtual CPUs
and a private Trusty domain under SVM/NPT, with assigned devices confined by
VT-d. The monitor verifies AVB payloads and obtains real Trusty RPMB approval
before BexOS entry. The kernel confirms its version-3 evidence against the
resident monitor. One harness-owned RPMB helper moves from the boot UART to
the normal-world `rpmb0` virtio console after the boot proxy acknowledges
release. Detailed target results and remaining gates are recorded in
[secure integration validation](secure-integration-validation.md).

Integrated product builds and the x86 E2E matrix select the source-built
EFI/monitor/Trusty closure through `//boot/efi:cached_firmware`. This includes
RFC 64's package-state secure application without relying on a previously saved
image. `//boot/efi:cached_firmware_boundary_test` checks that this dependency
remains present. Bazel caches unchanged firmware actions. Explicit
`//boot/efi:refresh_firmware` snapshots remain available for recovery; acceptance
uses `--//build/platforms:trusty_variant=acceptance`.
The integrated developer launcher is
`bazel run -c opt --config=x86_64 //device/virtual/qemu/nongui:run`.

The Q35 layout reserves handoff at `0x01000000`, the kernel at `0x02000000`,
BootFS at `0x08000000` (or `0x14000000` for images larger than 128 MiB),
replacement at `0x10000000`, and the Multiboot adapter
at `0x30000000`. Userspace begins at `0x40000000`. The adapter supports QEMU's
Multiboot1 ingress and constructs the Multiboot2 handoff expected by BexOS.
Fixed-address boot artifacts preserve the versioned BexOS handoff layout.

## ELF and package compatibility

The shared `//lib/elf` crate is `no_std` with `alloc`; both kernel bootstrap
and appd use it. Executables and libraries share dynamic validation,
relocation, symbol resolution, constructors, RELRO and TLS layout. Mapping
and package authorization stay in their callers. Architecture-specific
relocations live under `lib/elf/src/arch`.

Bazel stamps native package manifests before archive signing. Native
executables and shared libraries require `AARCH64` or `X86_64`; `MULTI` is
portable content only. Separate native archives keep the same package/version
identities. Wrong-machine payloads and incompatible native labels are
rejected. Existing unlabelled native installations remain unavailable until
rebuilt and reinstalled; their signed manifests are not rewritten.

Kernel snapshots, incremental records, prepare records and handoffs carry
architecture and format versions. Supported legacy ARM readers are retained.
Cross-architecture live migration is rejected. x86 same-architecture transplant
uses reserved parking/trampoline memory and retains the current scheduler
ownership restrictions.

## Standalone Trusty

The Trusty superproject remains pinned at
`ca93893635956b9605ee7897b09ecc8370e25c26`, including its pinned
`generic-x86_64` device dependency. Standard firmware includes storage,
KeyMint, Gatekeeper, AVB, AuthMgr FE/BE, the BexOS orchestrator, and their
required providers. The source recipe uses Bazel-managed LLVM, Rust,
bindgen and protoc. Its existing host preparation currently requires
Apple Silicon macOS, the macOS SDK, GNU make/sed, Python 3, dtc, xxd, and
OpenSSL development headers. Both QEMU system emulators must be on PATH
for the complete matrix.

```sh
bazel run //third_party/trusty:refresh_image
bazel run //third_party/trusty:refresh_authmgr_acceptance_image
bazel run //third_party/trusty:refresh_x86_64_image
bazel run //third_party/trusty:refresh_x86_64_acceptance_image
bazel run //third_party/trusty:run_x86_64
bazel test --config=e2e //third_party/trusty:x86_64_boot_test
bazel test --config=e2e //third_party/trusty:x86_64_acceptance_test
```

Consumers extract source-built firmware bundles. Saved snapshots are explicit recovery artifacts.
Refresh validates artifact hashes and architecture/variant metadata before
atomically replacing the gitignored bundle. Extraction checks host
compatibility and rejects ARM/x86 or standard/acceptance mismatches.

The development storage bridge passes opaque authenticated RPMB frames
between Trusty's storage proxy and the host RPMB emulator over COM2.
The host state file survives a guest restart. This is development transport
inside a standalone VM, not an isolated x86 secure world. The generic profile
uses upstream fake HWKEY/HWRNG and development keybox providers. Its explicit
QEMU TCG backend recognizes the TCG CPUID signature and exposes no protected
memory-sharing, MMIO-guard, or device-assignment capability. Unknown hypervisor
signatures remain errors. It offers
no secrecy from the host; bundle hashes do not authenticate guest boot.
The acceptance
image additionally contains real TIPC clients for storage, KeyMint HMAC and
key deletion, Gatekeeper verification/rejection, AVB, orchestrator, and the
AuthMgr acceptance/rejection flow. A two-boot test checks storage generation
across reboot. Acceptance clients and their expanded test ACLs are excluded
from the standard image.

## Validation

See [multiarchitecture validation](multiarchitecture-validation.md) for earlier
results and their scope. Those passes predate the current integration work and
matrix repair; they do not certify the current tree or integrated BexOS x86
security. [Secure integration validation](secure-integration-validation.md)
records current focused results, including the authenticated EFI and SVM probes.

Isolated x86 Trusty now boots under the in-tree SVM runtime, including two-boot
service acceptance through the signed EFI loader. Its monitor owns nested
RAM mappings, the NMI watchdog, and virtual interrupt/timer devices. Combined
development fixtures also boot four BexOS virtual CPUs with assigned NVMe and
modern virtio devices through monitor-owned VT-d. Both authenticated boots
complete BexFS persistence, debugd startup and the WASI randomness fixture;
the latest two-boot EFI checkpoint took 518.6 seconds. Those BexOS fixtures
still use the software TEE provider. The integrated product uses actual
BexOS-to-Trusty transport. Production provisioning and live Trusty replacement
remain outside this Q35 work; the saved firmware retains the development
provider limitations described above.
The x86 secure-monitor library has a tested register ABI, shared-memory
registration table, SVM/NPT diagnostic probe, authenticated EFI diagnostic
entry, and permanent execution ownership. The EFI product loader starts
authenticated recovery, selects protected component identities, then admits
the verified BexOS and Trusty service domains. See [the Trusty design](rfcs/0051/README.md)
and [the multiarchitecture design](rfcs/0057/README.md).

RFC 0064's earlier x86 pkgd, replacement archive and consumer builds predate the
latest networking and lifecycle changes. Final-tree x86 source-firmware, EFI
boundary, nongraphical/workstation assembly and package-resolution guest checks
remain outstanding. Existing x86 boot and security results do not validate
pkgd's new protected package-state endpoint or reboot persistence. See the
[package-resolution gap table](rfcs/0064/CURRENT.md#current-gaps).
