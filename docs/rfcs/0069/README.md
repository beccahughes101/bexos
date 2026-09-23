# RFC-0069: External SDK Generation, Out-of-Tree Component Development, and System Image Composition

* **Author:** BexOS Toolchain, Infrastructure & Platform Architecture Working Group
* **Status:** Proposed
* **Target Subsystems:** `//sdk`, `//build/rules`, `//tools`, `bex-pkg`, `image_assembler`, `sysui`, `driver_manager`
* **Applicability:** Out-of-Tree (OOT) Drivers, Third-Party Application Developers, Enterprise Add-ons, System Release Engineering

---

## 1. Summary

This RFC defines the end-to-end architecture for externalizing the BexOS development platform. It specifies:

1. **The In-Tree SDK Export Pipeline:** A hermetic Bazel packaging target (`//sdk:bexos_sdk`) producing a redistributable sysroot, target Rust standard library wrappers, host compiler toolchains (`fidlc`, `fidlgen_rust`), system IDL definitions, and Bazel/Cargo integration rules.
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
│ `bexos-sdk-v1.tar.gz` (Redistributable Architecture Bundle)                 │
│ • Sysroot (libbexos_vdso.so, headers)      • Compilers (fidlc, fidlgen_rust)│
│ • Bzlmod rules (@bexos_sdk)                • Crate stubs (bexos_driver)     │
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
│  Emits: GPT Partitioned Disk (`system.raw`, `bootfs.img`)                   │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 4. The SDK Archive Architecture

The SDK is exposed as a single versioned artifact, structured to be consumable by Bazel (via Bzlmod), CMake, or plain Rust Cargo workspaces.

### 4.1 Archive Physical Structure

```
bexos-sdk/
├── MODULE.bazel                      # Exported module definition
├── WORKSPACE.bzlmod                  # Legacy workspace compatibility marker
├── sdk.json                          # Manifest schema, target triple, and API level
├── sysroot/
│   ├── include/
│   │   ├── bexos/
│   │   │   ├── syscalls.h            # Microkernel syscall definitions
│   │   │   ├── types.h               # ABI types (zx_handle_t, zx_status_t)
│   │   │   └── vmar.h
│   │   └── zircon/                   # Compatibility layer headers
│   └── lib/
│       ├── x86_64-unknown-bexos/
│       │   ├── crt0.o                # C runtime startup stub
│       │   ├── libc.a                # Bare-metal musl-derived static libc
│       │   └── libbexos_vdso.so      # Virtual Dynamic Shared Object
│       └── aarch64-unknown-bexos/
│           ├── crt0.o
│           ├── libc.a
│           └── libbexos_vdso.so
├── tools/
│   ├── x86_64-linux/
│   │   ├── fidlc                     # IDL compiler binary
│   │   ├── fidlgen_rust              # Rust binding generator
│   │   ├── bex-pkg                   # Packaging & metadata serialization tool
│   │   └── image_assembler           # Disk image builder
│   └── aarch64-linux/
├── fidl/
│   ├── bexos.kernel/                 # Base kernel types
│   ├── bexos.hardware/               # Driver, PCI, DMA, and DeviceDataPlane
│   ├── bexos.net/                    # Stack, SocketProvider, and Routing
│   └── bexos.media/                  # CodecFactory, FrameBufferPool
├── rules/
│   ├── defs.bzl                      # Public Bazel build rules
│   ├── driver.bzl                    # bexos_driver rule implementation
│   ├── app.bzl                       # bexos_app rule implementation
│   └── fidl.bzl                      # bexos_fidl_library rule
└── rust/
    └── crates/
        ├── bexos_sys/                # Raw vDSO FFI bindings
        ├── bexos_driver/             # High-level libdriver traits
        ├── bexos_component/          # Namespace and startup handle bindings
        └── libpkg_client/            # IPC client for package resolution

```

### 4.2 In-Tree Build Target Definition

Inside the monorepo, `//sdk/BUILD.bazel` coordinates sysroot compilation, host tool cross-compilation, and tarball packaging:

```python
# //sdk/BUILD.bazel
load("//build/rules:sdk.bzl", "bexos_sdk_archive")

bexos_sdk_archive(
    name = "bexos_sdk",
    api_level = 1,
    host_tools = [
        "//tools/fidlc:fidlc",
        "//tools/fidlgen_rust:fidlgen_rust",
        "//tools/bex_pkg:bex_pkg",
        "//tools/image_assembler:image_assembler",
    ],
    fidl_libraries = [
        "//sdk/fidl/bexos.kernel:bexos.kernel",
        "//sdk/fidl/bexos.hardware:bexos.hardware",
        "//sdk/fidl/bexos.net:bexos.net",
        "//sdk/fidl/bexos.media:bexos.media",
        "//sdk/fidl/bexos.ui:bexos.ui",
    ],
    rust_crates = [
        "//sdk/rust/bexos_sys",
        "//sdk/rust/bexos_driver",
        "//sdk/rust/bexos_component",
        "//sdk/rust/libpkg_client",
    ],
    sysroots = {
        "x86_64-unknown-bexos": "//kernel/sysroot:sysroot_x86_64",
        "aarch64-unknown-bexos": "//kernel/sysroot:sysroot_aarch64",
    },
    visibility = ["//visibility:public"],
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

bazel_dep(name = "bexos_sdk", version = "1.0.0")

# Pull published SDK from remote release storage or corporate mirror
archive_override(
    module_name = "bexos_sdk",
    urls = [
        "https://dl.bexos.org/sdk/releases/v1.0.0/bexos-sdk-v1.0.0-linux-x86_64.tar.gz",
    ],
    integrity = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
    strip_prefix = "bexos-sdk",
)

```

### 5.2 Compiling an Out-of-Tree Component (`BUILD.bazel`)

The external `BUILD.bazel` leverages SDK rules to build a compliant driver package:

```python
load("@bexos_sdk//rules:driver.bzl", "bexos_driver_package")
load("@bexos_sdk//rules:fidl.bzl", "bexos_fidl_rust_library")

# Compile local vendor-specific protocols if necessary
bexos_fidl_rust_library(
    name = "acme_diagnostics_rust",
    srcs = ["fidl/acme.diagnostics.fidl"],
    deps = ["@bexos_sdk//fidl:bexos.hardware"],
)

bexos_driver_package(
    name = "acme_nic_pkg",
    driver_name = "acme-pcie-100g",
    manifest = "meta/acme_nic.json",
    srcs = glob(["src/**/*.rs"]),
    deps = [
        ":acme_diagnostics_rust",
        "@bexos_sdk//rust:bexos_driver",
        "@bexos_sdk//fidl:bexos_hardware_rust",
    ],
)

```

---

## 6. Package Specification: The `.bex` Archive

All components—whether built in-tree or out-of-tree—must produce a standardized `.bex` package. A `.bex` archive is an uncompressed, deterministic tarball structured with content-addressable BLAKE3 hashing.

```
+-----------------------------------------------------------------------+
| package.manifest (JSON: component name, version, ABI level, sandbox)  |
+-----------------------------------------------------------------------+
| meta/contents (Sorted table: internal path -> BLAKE3 byte digest)     |
+-----------------------------------------------------------------------+
| bin/executable (ELF64 PIC shared object or dynamic binary)            |
+-----------------------------------------------------------------------+
| lib/ (Dependent vendored dynamic libraries, if any)                  |
+-----------------------------------------------------------------------+
| data/ (Immutable read-only assets, config defaults, style sheets)     |
+-----------------------------------------------------------------------+
| signature.tuf (TUF cryptographic envelope & metadata signature block) |
+-----------------------------------------------------------------------+

```

### 6.1 Package Manifest Schema (`package.manifest`)

```json
{
  "schema_version": "1.0.0",
  "package_type": "driver",
  "name": "acme-pcie-100g",
  "version": "1.2.0",
  "abi_version": 1,
  "entrypoint": "bin/libacme_nic.so",
  "match_rules": [
    {
      "bus": "pci",
      "vendor_id": "0x1d0f",
      "device_ids": ["0x1000", "0x1001"]
    }
  ],
  "sandbox": {
    "colocation_policy": "isolated",
    "required_capabilities": [
      "bexos.hardware.PciDevice",
      "bexos.hardware.Interrupt",
      "bexos.hardware.Iommu"
    ]
  }
}

```

---

## 7. Product Assembly and Out-of-Tree Image Ingestion

The image generation subsystem merges in-tree binaries and out-of-tree `.bex` packages into final deployable operating system images.

### 7.1 Declarative Product Specification (`product.json`)

Product definitions govern the content of the target image:

```json
{
  "product_name": "bexos_datacenter_node",
  "target_arch": "x86_64-unknown-bexos",
  "partitions": {
    "bootfs": {
      "compression": "zstd",
      "packages": [
        "//services/driver_manager:driver_manager_pkg",
        "//services/pkgd:pkgd_pkg",
        "@acme_vendor//:acme_nic_pkg"
      ]
    },
    "system": {
      "filesystem": "bexfs",
      "read_only": true,
      "packages": [
        "//apps/sysui:sysui_pkg",
        "//services/networkd:networkd_pkg",
        "//services/scened:scened_pkg",
        "//prebuilts/enterprise:monitoring_agent_pkg"
      ]
    }
  }
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
)

# Pattern B: Fetching an external Git repository that builds with the SDK
git_repository = use_repo_rule("@bazel_tools//tools/build_defs/repo:git.bzl", "git_repository")
git_repository(
    name = "acme_vendor",
    remote = "git@github.com:acme-corp/acme-bexos-drivers.git",
    commit = "a1b2c3d4e5f6...",
)

```

### 7.3 System Image Assembler (`image_assembler`)

The `image_assembler` host tool reads the product specification, extracts all `.bex` packages, verifies dependencies, and lays out the target disk structure:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ `image_assembler` PROCESSING RUN                                            │
│                                                                             │
│ 1. Validate ABI Level:                                                      │
│    Verifies all in-tree and external .bex packages match `abi_version == 1`.│
│                                                                             │
│ 2. Deduplicate Shared Blobs:                                                │
│    Extracts package layers into a flat, content-addressed block store.      │
│                                                                             │
│ 3. Build BootFS Partition:                                                  │
│    Serializes critical bring-up drivers and services into an uncompressed,  │
│    linearly indexed microkernel ramdisk image (`bootfs.img`).               │
│                                                                             │
│ 4. Build System Partition:                                                  │
│    Constructs an immutable, cryptographically hashed Merkle-tree filesystem │
│    (`system.img`) containing applications, non-boot drivers, and fonts.     │
│                                                                             │
│ 5. GPT Packaging:                                                           │
│    Generates standard GPT partition table with EFI System Partition (ESP),  │
│    BootFS (A/B), and System (A/B) containers.                               │
└─────────────────────────────────────────────────────────────────────────────┘

```

```python
# //build/images/BUILD.bazel
load("//build/rules:image.bzl", "bexos_disk_image")

bexos_disk_image(
    name = "bexos_install_image",
    product_config = "//products:datacenter_node.json",
    kernel = "//kernel:bexos_kernel",
    bootloader = "//bootloader:uefi_bootloader",
    output = "bexos_x86_64.raw",
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

1. **Mandatory Package Signatures:** Out-of-tree packages bundled into production release images must include valid TUF metadata (`signature.tuf`). Unsigned or developer-signed packages are rejected unless the image assembler is invoked with `--allow-unsigned-packages` (enforced via hardware secure boot fuses).
2. **Capability Auditing:** During image composition, `image_assembler` evaluates the permissions declared in every external driver's manifest. If an out-of-tree package demands unattenuated hardware access or undocumented capabilities, the build halts with a policy violation.
3. **No Dynamic Execution During Assembly:** The image assembler treats all `.bex` inputs strictly as passive data streams. No installation scripts, maintainer hooks, or pre/post-install code are executed during image creation.

---

## 10. Implementation Roadmap

### Phase 1: Toolchain Scaffolding & SDK Archive Export

* Construct the `//sdk:bexos_sdk` rule in Bazel, packaging host binaries (`fidlc`, `fidlgen_rust`, `bex-pkg`), sysroots, and core FIDL protocols.
* Publish `bexos-sdk` tarball releases via GitHub Actions automation.

### Phase 2: Standalone Out-of-Tree Proving Ground

* Create an external sample repository (`[github.com/beccahughes101/bexos-samples](https://github.com/beccahughes101/bexos-samples)`) containing an out-of-tree PCIe driver and a Dioxus Native user app.
* Configure Bzlmod to pull the published SDK and verify independent, hermetic compilation into `.bex` packages.

### Phase 3: Declarative Image Assembler

* Build the `image_assembler` host tool in Rust.
* Implement `bexos_disk_image` Bazel rules consuming `product.json` definitions.
* Validate bootable QEMU execution of a disk image composed of in-tree services alongside external `.bex` driver binaries.