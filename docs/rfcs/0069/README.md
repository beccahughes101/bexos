# RFC-0069: External SDK Generation, Out-of-Tree Component Development, and System Image Composition

* **Author:** BexOS Toolchain, Infrastructure & Platform Architecture Working Group
* **Status:** Bazel SDK 0.2 implemented at API level 1; non-Bazel and general-image phases proposed
* **Target Subsystems:** `//sdk`, `//build/rules`, `//tools`, `bex-pkg`, `image_assembler`, `sysui`, `driver_manager`
* **Applicability:** Out-of-Tree (OOT) Drivers, Third-Party Application Developers, Enterprise Add-ons, System Release Engineering

---

## Implementation selection (SDK 0.2)

The implemented Bazel-first slice includes applications, signed native
services and D1 drivers, per-architecture sysroots/libc artifacts, verified
system-image and explicit early-BootFS import, and live heart-transplant
bindings. Cargo/CMake interfaces, OCI publication, a new kernel vDSO,
product-specific rewriting/resigning, and a general GPT image assembler remain
proposed later phases. The longer-term sections below are retained as design direction;
[`CURRENT.md`](CURRENT.md) is authoritative for implemented behavior.

The canonical authoring format is a prototxt `bexos.app.Manifest` with
`min_bexos_abi_version: 1`; `fidlc` is the combined parser and Rust generator;
and the emitted package is a signed BEXARCV2 archive. Product import installs
the unchanged archive into encrypted `STORAGE` and references it from the
system-image manifest or, with explicit product authorization, places its
verified package tree in BootFS. Cargo/CMake, OCI, a kernel vDSO, and a general
GPT image assembler remain future phases.

## 1. Summary

This RFC defines the end-to-end architecture for externalizing the BexOS development platform. It specifies:

1. **The In-Tree SDK Export Pipeline:** A hermetic Bazel packaging target (`//sdk:bexos_sdk`) producing source-based application/component runtimes, per-architecture sysroots, the combined `fidlc` parser/Rust generator, system IDL definitions, host package tools, and Bazel rules. Cargo/CMake integration remains a future phase.
2. **The Out-of-Tree (OOT) Development Contract:** A standardized, decoupled developer environment consumed via Bazel Bzlmod (`@bexos_sdk`) or standalone Cargo tooling, allowing vendors to compile proprietary device drivers, services, and native Dioxus applications against stable system ABIs.
3. **The `.bex` Canonical Package Format:** An authenticated, content-addressed package structure containing manifests, binary execution units (ELF/WASM), and component assets.
4. **Declarative Product Assembly & Image Ingestion:** A configurable image generation pipeline enabling platform release engineering to compose bootable disk images (`system.raw`, `bootfs`) combining in-tree platform daemons with pre-compiled, out-of-tree vendor packages.

---

## 2. Motivation

BexOS maintains an in-tree monorepo hosting the microkernel, foundational D1/D2 services (`pkgd`, `netstack`, `driver_manager`, `scened`), and baseline shell environments (`sysui`). While optimal for core operating system cohesion, requiring all software to live within the primary Git repository creates severe friction:

* **Vendor Intellectual Property Boundaries:** Hardware manufacturers frequently refuse to release driver source code under open-source licenses, requiring clean out-of-tree binary distribution models.
* **Independent Release Lifecycles:** Enterprise user applications, custom media codecs, and third-party peripherals need to be developed, versioned, and tagged independently of the platform monorepo's commit cadence.
* **Build Graph Saturation:** Forcing third-party consumers to clone and evaluate the entire OS build graph to compile a single hardware driver or native utility degrades developer ergonomics and CI turnaround times.
* **System Appliance Customization:** Original Equipment Manufacturers (OEMs) and enterprise appliance builders require a declarative mechanism to assemble tailored OS installation images by injecting proprietary software bundles into baseline platform releases without maintaining downstream source forks of the entire OS.

---

## 3. Architecture & Artifact Flow

The platform externalization lifecycle operates as a two-phase unidirectional loop:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ IN-TREE PLATFORM REPOSITORY (github.com/beccahughes101/bexos)               │
│                                                                             │
│  [ Kernel ABI / vDSO ]  [ FIDL Protocols ]  [ Rust Crates ]  [ Host Tools ] │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ `bazel build //sdk:sdk`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `bexos-sdk-v0.2.0-<host>.tar.gz` (Redistributable Bazel SDK)                │
│ • Rust app/service/driver runtime source    • AArch64/x86-64 sysroots        │
│ • Bzlmod rules and fidlc                    • Signed BEXARCV2 tooling        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Consumed via Bzlmod / Registry
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ OUT-OF-TREE VENDOR REPOSITORY (e.g., github.com/acme-corp/acme-nic)         │
│                                                                             │
│  [ Vendor Driver Source ] ──► Compiles against `@bexos_sdk`                 │
│                               Produces hermetic package: `acme_nic.bex`     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Published as OCI Artifact / Tarball
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ PRODUCT COMPOSITION & IMAGE ASSEMBLER (`//build/images:bexos_image`)        │
│                                                                             │
│  Ingests:                                                                   │
│  ├── In-Tree Base Targets (`//services/networkd`, `//services/pkgd`)       │
│  └── External Prebuilt Packages (`@prebuilts//:acme_nic.bex`)               │
│                                                                             │
│  Emits: system install manifest + encrypted STORAGE image                   │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 4. The SDK Archive Architecture

SDK 0.2 is exposed as a versioned Bzlmod artifact. Cargo/CMake consumption
remains a later phase.

### 4.1 Archive Physical Structure

```
bexos-sdk/
├── MODULE.bazel                      # Pinned, self-contained Bzlmod module
├── sdk-Cargo.lock                    # Pinned WASM binding dependencies
├── sdk-crates-lock.json
├── meta/sdk.prototxt                 # SDK version and host metadata
├── idl/                              # Public FIDL and manifest protos
├── rules/                            # Public app, archive, FIDL rules/linkers
├── rust/                             # Native and portable app runtime source
├── platforms/                        # Native AArch64/x86-64 and WASI platforms
├── sysroot/{aarch64,x86_64}/         # Headers, licenses, crt0/libc/runtime archives
├── examples/                         # Prototxt app/service/driver examples
└── tools/bin/
    ├── fidlc                         # Combined parser and Rust generator
    ├── manifest_stamp
    ├── config_compiler
    ├── bexos_assembly
    └── bex_archive                   # Signed BEXARCV2 tooling
```

Future phases may add Cargo/CMake facades, a kernel vDSO, OCI publication, and
the general disk image assembler shown in the longer-term architecture.

### 4.2 In-Tree Build Target Definition

Inside the monorepo, `//sdk/BUILD.bazel` packages normalized source files and
host tools into the two supported host archives:

```python
# //sdk/BUILD.bazel
load(":sdk_archive.bzl", "sdk_archive")

sdk_archive(
    name = "bexos_sdk_linux_x86_64",
    srcs = [":export_sources", "//idl:sdk_public_sources"],
    renamed_files = {"meta/linux_x86_64.prototxt": "meta/sdk.prototxt"},
    tools = {
        "//tools/fidlc:fidlc": "tools/bin/fidlc",
        "//tools/app_archive:bex_archive": "tools/bin/bex_archive",
        # manifest/config/assembly tools omitted here for brevity
    },
    out = "bexos-sdk-v0.2.0-linux-x86_64.tar.gz",
)

```

---

## 5. The Out-of-Tree Developer Experience

External repositories do not maintain copies of BexOS build infrastructure; they declare an external dependency on the SDK via standard package mechanisms.

### 5.1 Out-of-Tree `MODULE.bazel` Configuration

An external vendor repo (e.g., `acme-nic-driver`) configures Bazel Bzlmod as follows:

```python
module(
    name = "acme_nic_driver",
    version = "1.2.0",
)

bazel_dep(name = "bexos_sdk", version = "0.2.0")

# Pull published SDK from remote release storage or corporate mirror
archive_override(
    module_name = "bexos_sdk",
    urls = [
        "https://github.com/beccahughes101/bexos/releases/download/sdk-v0.2.0/bexos-sdk-v0.2.0-linux-x86_64.tar.gz",
    ],
    integrity = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
    strip_prefix = "bexos-sdk",
)

```

### 5.2 Compiling an Out-of-Tree Application (`BUILD.bazel`)

The external `BUILD.bazel` uses the exported v1 rules to build signed app
packages. The private key is supplied by the consumer and is not in the SDK:

```python
load(
    "@bexos_sdk//rules:defs.bzl",
    "bexos_fidl_rust_library",
    "bexos_native_app",
    "bexos_wasm_app",
)

# Compile local vendor-specific protocols if necessary
bexos_fidl_rust_library(
    name = "acme_diagnostics_rust",
    srcs = ["fidl/acme.diagnostics.fidl"],
)

bexos_wasm_app(
    name = "diagnostics",
    srcs = ["src/diagnostics.rs"],
    deps = ["@bexos_sdk//rust:bexos_wasm_guest"],
    manifest = "meta/diagnostics.prototxt",
    signing_key = "//keys:release_signing.key",
)

bexos_native_app(
    name = "diagnostics_aarch64",
    srcs = ["src/native.rs"],
    architecture = "aarch64",
    manifest = "meta/diagnostics.prototxt",
    signing_key = "//keys:release_signing.key",
)

```

`bexos_service` and `bexos_driver` are implemented with the source runtime,
sysroot, FIDL, C `link_deps`, and signed manifest contracts described below.

---

## 6. Package Specification: The `.bex` Archive

SDK components produce signed BEXARCV2 `.bex` archives. BEXARCV2 is a binary
container with a sorted entry table, optional per-entry zstd compression,
4-KiB BLAKE3 chunk hashes, a signed BLAKE3 content root, signer key ID, and an
Ed25519 signature. `package.bexmanifest` is the compiled protobuf manifest;
`config/component.bexconfig` carries the signed default configuration.

```
+-----------------------------------------------------------------------+
| BEXARCV2 header and sorted entry metadata                            |
+-----------------------------------------------------------------------+
| package.bexmanifest (compiled bexos.app.Manifest protobuf)           |
+-----------------------------------------------------------------------+
| bin/executable (ELF64 PIC shared object or dynamic binary)            |
+-----------------------------------------------------------------------+
| lib/ (Dependent vendored dynamic libraries, if any)                  |
+-----------------------------------------------------------------------+
| data/ (Immutable read-only assets, config defaults, style sheets)     |
+-----------------------------------------------------------------------+
| BEXSIGV2 trailer (key ID, content root, Ed25519 signature)           |
+-----------------------------------------------------------------------+

```

### 6.1 Package Manifest Source (`package.prototxt`)

```textproto
package_name: "com.acme.diagnostics"
name: "ACME diagnostics"
min_bexos_abi_version: 1
processes {
  name: "diagnostics"
  runner: "wasm"
  service: true
  wave: 7
  lifecycle { update_strategy: HEART_TRANSPLANT }
  runner_options {
    [type.googleapis.com/bexos.app.WasmRunnerOptions] {
      path: "/pkg/bin/diagnostics.wasm"
    }
  }
}
```

---

## 7. Product Assembly and Out-of-Tree Image Ingestion

The image generation subsystem merges in-tree binaries and out-of-tree `.bex` packages into final deployable operating system images.

### 7.1 Declarative Product and System-Image Inputs

Product definitions and system-image manifests are prototxt. Out-of-tree apps
are appended by the reusable image rules, so the checked-in base manifest can
remain product-owned:

```textproto
base_packages: "//services/appd:appd_elf"
base_packages: "//services/netstack:netstackd_archive"
autoinstall_packages {
  package: "//services/netstack:netstackd_archive"
}
```

### 7.2 Ingesting External Packages into the Monorepo Build Graph

When building custom product images, external prebuilt packages are mapped into the monorepo's workspace using `MODULE.bazel`:

```python
# In monorepo: MODULE.bazel

# Pattern A: Direct HTTP download of pre-compiled .bex artifact
http_file = use_repo_rule("@bazel_tools//tools/build_defs/repo:http.bzl", "http_file")
http_file(
    name = "prebuilt_acme_nic",
    urls = ["https://packages.acme.corp/bexos/acme_nic-1.2.0.bex"],
    sha256 = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    downloaded_file_path = "acme_nic.bex",
)

```

### 7.3 Verified Image Flow

Product assembly verifies every external archive as passive bytes, validates
its extracted manifest with the in-tree product graph, adds its ID to the
system-image install manifest, and installs the unchanged archive into the
encrypted `STORAGE` BexFS image:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PRODUCT/IMAGE RULE PROCESSING                                               │
│                                                                             │
│ 1. Validate ABI Level:                                                      │
│    Verifies all external packages have `min_bexos_abi_version <= 1`.        │
│                                                                             │
│ 2. Verify Signed Package:                                                   │
│    Validates BEXARCV2 chunks/content root/signature and expected signer.    │
│                                                                             │
│ 3. Extend System Manifest:                                                  │
│    Adds package IDs to base_packages and requested autoinstall entries.     │
│                                                                             │
│ 4. Build STORAGE:                                                           │
│    Encrypts BexFS and stores exact bytes at pkg/<package_id>.bex.            │
│                                                                             │
│ 5. Preserve Base Product:                                                   │
│    Existing products change only when explicitly passed prebuilt_apps.      │
└─────────────────────────────────────────────────────────────────────────────┘

```

```python
load("//build/rules:prebuilt_app.bzl", "bexos_prebuilt_app")

bexos_prebuilt_app(
    name = "acme_diagnostics",
    archive = "@prebuilt_acme_nic//file",
    package_id = "com.acme.diagnostics",
    public_key = "//products/keys:acme_signer.prototxt",
    autoinstall = True,
)

```

---

## 8. Compatibility, ABI Stability & Evolution

To guarantee out-of-tree binaries run reliably across OS upgrades, BexOS enforces strict interface stability guarantees.

### 8.1 API Levels vs. ABI Numbers

* **Target Triple:** External code compiles against `x86_64-unknown-bexos` or `aarch64-unknown-bexos`.
* **API Level:** An integer incremented monotonically with each public feature release (e.g., Level 1, Level 2).
* **vDSO Stability:** System calls are not direct software interrupts (`int 0x80` or bare `syscall`). All system calls enter userspace through the vDSO (`libbexos_vdso.so`). The vDSO provides ABI backward-compatibility shims across minor kernel versions.

### 8.2 FIDL Wire Stability

* All FIDL protocols exposed via the SDK are flagged as `@available(added=1)`.
* Changes to existing FIDL definitions in `bexos.*` must preserve ordinal layouts. Structural mutations require deprecation cycles across at least two API Levels.
* If an out-of-tree driver built against API Level 1 runs on an API Level 2 host, `driver_manager` checks the binary's manifest and instantiates compatibility shims if necessary.

---

## 9. Security & Verification

1. **Mandatory Package Signatures:** Every SDK import must be BEXARCV2-signed with Ed25519. Assembly requires the expected product signing root and rejects unsigned, malformed, tampered, or wrongly signed inputs. There is no unsigned bypass.
2. **Capability Auditing:** SDK 0.2 validates driver resources/bind rules and requires exact product runner and driver grants. Broader general-image capability planning remains future work.
3. **No Dynamic Execution During Assembly:** The image assembler treats all `.bex` inputs strictly as passive data streams. No installation scripts, maintainer hooks, or pre/post-install code are executed during image creation.

---

## 10. Implementation Roadmap

### Phase 1: Toolchain Scaffolding & SDK Archive Export

* Implemented: construct `//sdk:bexos_sdk` with combined `fidlc`, BEXARCV2/config tools, public runtime source, FIDL, examples, service/driver rules, and dual-architecture sysroots.
* Implemented: publish host-specific `bexos-sdk` tarballs and checksums from `sdk-v*` tags.
* Implemented: package `crt0.o`, `libc.a`, the runtime archive, headers/licenses, and source-linked Rust std/libc.

### Phase 2: Standalone Out-of-Tree Proving Ground

* Implemented: maintain a separate fixture workspace proving FIDL, portable
  WASM, std Rust plus C linkage, services, a PCI e1000e driver, replacements,
  and both native architectures against only the unpacked SDK.

### Phase 3: Declarative Image Assembler

* Implemented: verified prebuilt ingestion through reusable product,
  system-image, BootFS-overlay, and encrypted BexFS rules, with
  dual-architecture QEMU acceptance and provenance-aware policy.
* Future: implement a general GPT `image_assembler` and out-of-tree driver image
  composition.

---

## 11. Retained later-phase design

This section preserves both the now-selected service/driver/sysroot shape and
the longer-term non-Bazel, OCI, vDSO, and general image-assembler design. The
future interfaces must keep the prototxt manifest, BEXARCV2 signature, and
passive-assembly security model defined above.

### 11.1 Binary sysroot and runtime distribution

SDK 0.2 extends the source archive with a stable binary sysroot for both guest
architectures:

```
bexos-sdk/
├── sysroot/
│   ├── include/
│   │   ├── bexos/
│   │   │   ├── syscalls.h
│   │   │   ├── types.h
│   │   │   └── vmar.h
│   │   └── zircon/                   # compatibility headers, if retained
│   └── lib/
│       ├── x86_64-unknown-bexos/
│       │   ├── crt0.o
│       │   ├── libc.a
│       │   └── libbexos_vdso.so
│       └── aarch64-unknown-bexos/
│           ├── crt0.o
│           ├── libc.a
│           └── libbexos_vdso.so
├── rust/crates/
│   ├── bexos_sys/                    # raw vDSO ABI bindings
│   ├── bexos_component/              # namespace/startup handles
│   ├── bexos_driver/                 # driver startup/resource bindings
│   └── libpkg_client/
├── cmake/                            # future toolchain/config packages
└── cargo/                            # future target/linker configuration
```

Those binaries require an explicit compatibility and release policy. The
sysroot must be built hermetically for both guest targets, versioned with the
SDK ABI, and tested against an independently built consumer. Distributing it is
is provided by SDK 0.2; the new kernel-vDSO portion of this sketch remains future work.

### 11.2 External drivers and platform services

SDK 0.2 provides `bexos_driver` and `bexos_service` surfaces backed by the
sysroot and stable public FIDL. They cover PCI/MMIO/DMA/interrupt startup
resources, driver match metadata,
component namespaces, startup handles, and capability declarations without
exposing monorepo-private implementation libraries.

Driver and service outputs use signed BEXARCV2 packages and
prototxt manifests. Every SDK-built service process must declare
`HEART_TRANSPLANT`; importing a driver or service must additionally validate
its package kind, target architecture, ABI level, signer authorization, and
capability policy before assembly. No package hook or binary may execute on the
build host.

The proving ground remains outside the monorepo build graph and includes a PCIe
driver and native platform service for both guest architectures. A separately versioned source repository can consume the
published SDK through Bzlmod; checking out a vendor repository with a pinned
commit remains a product-integration option, but it is not a substitute for
verifying the resulting signed archive.

### 11.3 Cargo and CMake consumers

Cargo and CMake facades remain proposed for consumers that cannot adopt Bazel.
They should select only published target triples and linker definitions, use
the same combined `fidlc` generator and manifest/config compilers, and produce
byte-for-byte equivalent BEXARCV2 package inputs. They must not introduce an
alternate JSON manifest, an unsigned development package format, or a second
ABI contract.

### 11.4 OCI publication

An OCI distribution may mirror immutable SDK archives and signed package
artifacts after the tarball release process is stable. OCI manifests should
carry the SDK version, host platform, guest target set, checksum, and ABI level;
the archive checksum and package signer remain the trust inputs. OCI is an
additional transport, not a replacement for GitHub release assets or BEXARCV2
verification.

### 11.5 General image assembler

The proposed general assembler expands the v1 product/image flow into a tool
that consumes prototxt product definitions and emits complete GPT media. Its
planned stages are:

1. Verify every in-tree and external package, signer, package kind,
   architecture, ABI, dependency, service, and capability declaration.
2. Resolve duplicate package IDs and destinations before writing any image.
3. Generate the boot filesystem and system install manifest.
4. Populate encrypted `STORAGE` with unchanged signed packages and other
   declared filesystem content.
5. Construct the target GPT/boot partitions and secure-boot metadata for the
   selected architecture.

The assembler must remain hermetic and deterministic, must never run package
code, and must fail closed on malformed or incompatible inputs. Product-specific
rewriting or resigning requires a separate policy and is not inherited from
SDK 0.2.
