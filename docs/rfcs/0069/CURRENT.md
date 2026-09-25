# RFC 0069 current state

RFC 0069 SDK 0.2 is implemented at API level 1. The distributable SDK supports
signed WASM/native applications, native services, D1 drivers, source-linked
Rust `std`/libc, C helpers, and product-authorized early BootFS placement.
The RFC README preserves the later Cargo, CMake, OCI, kernel-vDSO, package
rewriting, and general GPT-assembler designs.

## SDK artifacts and authoring API

`//sdk:bexos_sdk` selects a deterministic host archive:

- `bexos-sdk-v0.2.0-linux-x86_64.tar.gz`
- `bexos-sdk-v0.2.0-macos-aarch64.tar.gz`

The matching `//sdk:bexos_sdk_sha256` output covers the complete archive. Both
host archives target AArch64 and x86-64 and retain `api_level: 1` and
`min_bexos_abi_version: 1`. They contain pinned Bzlmod dependencies, public
FIDL sources and generation rules, host tools, source-distributed component,
driver-startup, hardware-resource, service-binding, migration and libc crates,
and per-architecture sysroots. Each sysroot contains curated BexOS/GNU ABI
headers and license notices plus `crt0.o`, `libc.a`, and
`libbexos_runtime.a`. Private signing keys are never included.

Consume the unpacked module with:

```starlark
module(name = "example_component", version = "1.0.0")
bazel_dep(name = "bexos_sdk", version = "0.2.0")
```

The stable rule surface in `@bexos_sdk//rules:defs.bzl` is:

- `bexos_fidl_rust_library`
- `bexos_app_manifest` and `bexos_app_archive`
- `bexos_wasm_app` and `bexos_native_app`
- `bexos_service` and `bexos_driver`

`bexos_service` and `bexos_driver` select AArch64 or x86-64 explicitly, accept
Rust/FIDL dependencies and C `link_deps`, and produce signed BEXARCV2 archives.
`std = True` selects the source-distributed BexOS libc runtime; the default is
freestanding Rust. Manifests remain prototxt. Every service/driver process must
be a service process with `HEART_TRANSPLANT`; driver manifests additionally
require exact driver identity, bounded hardware-resource declarations, bind
rules, and a signed boot wave. Examples decode `Startup`, consume capability
handles/resources, and use the versioned live-migration `State` protocol to
adopt state and preserved handles.

## Verified product import

Products import new content with:

```starlark
bexos_prebuilt_component(
    name = "vendor_driver",
    archive = "@vendor//:driver.bex",
    package_id = "com.vendor.driver.nic",
    signing_root = "//product/roots:vendor.prototxt",
    signer_id = "vendor-production-root",
    component_type = "driver",
    placement = "BOOTFS",
    boot_wave = 4,
    autoinstall = False,
)
```

`component_type` is `application`, `service`, or `driver`; `placement` defaults
to `SYSTEM_IMAGE`. `BOOTFS` requires an explicit `boot_wave` that must exactly
match the signed manifest and can never be autoinstalled. The signing-root
input supplies the Ed25519 key, key ID, and product root `anchor_id`.
`bexos_prebuilt_app` remains the compatibility wrapper for system-image
applications.

The importer verifies archive structure, chunks, signature/root, package ID,
ABI, manifest architecture, every ELF payload machine, role contract,
heart-transplant policy, boot wave, bind rules/resources, and normalized paths
before creating a declared package tree. It never runs package code or hooks.
Product assembly rejects duplicate package IDs and placements, incompatible
library dependencies, and in-tree/prebuilt collisions. External native
components require an exact `<package>@<root>` runner grant; drivers require a
second exact driver grant.

System-image imports retain the original signed archive bytes in encrypted
`STORAGE/pkg/<package>.bex`. BootFS imports are overlaid at
`/boot/pkg/<package-id>/` from verified trees. The assembly index carries the
verified root and role into BootFS; appd uses that provenance for runner and
driver policy rather than promoting external code to the official platform
signer. Storage import loads product app-signing roots and records the actual
verified root identity in the app registry.

## Acceptance

`testing/out_of_tree_sdk/fixture` builds WASM/native applications, a std/libc
early service with a C helper, a D1 e1000e PCI driver, and replacement archives
using only the unpacked SDK. The AArch64 and x86-64 QEMU acceptance products
overlay the service and driver in BootFS, attach the e1000e device, prove the
service starts before the storage pivot, verify structured driver resources,
and apply live replacements while checking versioned state and handle/resource
continuity. Wrong roots, architecture/ABI, wave, runner grant, driver grant,
manifest shape, and unsafe archive paths fail closed in focused tests or
analysis.

```sh
bazel run //testing/out_of_tree_sdk:acceptance -- aarch64 smoke
bazel run //testing/out_of_tree_sdk:acceptance -- x86_64 smoke
bazel run //testing/out_of_tree_sdk:acceptance -- aarch64 qemu
bazel run //testing/out_of_tree_sdk:acceptance -- x86_64 qemu
```

Release tags must exactly match `sdk-v0.2.0`. The release workflow builds both
host archives, smoke-builds the external workspace, validates archive layout,
and runs the Linux QEMU matrix.

## Out of scope

Cargo/CMake consumption, OCI publication, a new kernel vDSO, product-side
rewriting/resigning, and the general GPT image assembler remain future work.
The shipped C ABI and Rust `std` support are Bazel-first and use the current
kernel service/syscall ABI. Ordinary external packages remain storage-installed
by default; early BootFS placement is always explicit and product-authorized.
