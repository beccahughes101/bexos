# Out-of-Tree Development

This guide is the current, supported path for building software outside the
BexOS source tree with the BexOS SDK. It covers SDK **0.2.0**, API level **1**,
and signed `BEXARCV2` packages. The current implementation is Bazel/Bzlmod
first. Cargo-only builds, CMake integration, OCI publication, a general disk
image assembler, and a D2 driver runtime are future work, not alternate
supported paths.

The long-term design remains in [RFC 0069](../rfcs/0069/README.md). Its
[current-state companion](../rfcs/0069/CURRENT.md) is authoritative when the
design and implementation differ.

## Choose a component

| What you are building | SDK rule | Runtime | Architecture output | Heart transplant |
| --- | --- | --- | --- | --- |
| Portable application or service | `bexos_wasm_app` | WASI 0.2 component | One `MULTI` archive | Required when its process is a service |
| Native application | `bexos_native_app` | BexOS ELF | One archive per target | Required when its process is a service |
| Native service | `bexos_service` | BexOS ELF | One archive per target | Required |
| D1 driver | `bexos_driver` | BexOS ELF | One archive per target | Required |

Use a WASM application when portability and the WASI component boundary fit
the workload. Use a native application for the native startup ABI. Use a
service for a long-running provider, and a D1 driver only when appd must bind a
signed package to a device and grant typed hardware resources.

## Supported build matrix

The released SDK archive is host-specific but contains both native target
sysroots.

| SDK host archive | Targets produced |
| --- | --- |
| `bexos-sdk-v0.2.0-linux-x86_64.tar.gz` | AArch64, x86-64, WASI 0.2 |
| `bexos-sdk-v0.2.0-macos-aarch64.tar.gz` | AArch64, x86-64, WASI 0.2 |

Native archives are architecture-specific. AArch64 and x86-64 packages keep
the same package identity but are built and signed separately. WASM packages
are stamped `MULTI`; a `MULTI` package cannot contain an ELF executable or
native shared library.

## End-to-end path

1. [Set up the SDK and external workspace](setup.md).
2. Author an [application](applications.md), [service](services.md), or
   [D1 driver](drivers.md).
3. Declare [manifests, assets, signing, and packaging](manifests-and-packaging.md).
4. Generate local bindings and declare routes using
   [FIDL and capabilities](fidl-and-capabilities.md).
5. Implement [heart transplant](heart-transplant.md) for every service or
   driver before considering it complete.
6. Hand the signed archive and public signing-root record to the
   [product integrator](product-integration.md).
7. Use the [validation and troubleshooting](testing-and-troubleshooting.md)
   workflow before release.

The package author owns source, manifests, private signing material, and the
signed `.bex`. The product integrator owns accepted signing roots, exact runner
and driver grants, placement, product assembly, and boot validation. Product
assembly verifies package bytes; it does not run package code or rewrite the
signed manifest.

## Canonical source examples

- [`sdk/export/examples`](../../sdk/export/examples/README.md) contains the
  examples shipped in the SDK archive.
- [`testing/out_of_tree_sdk/fixture`](../../testing/out_of_tree_sdk/fixture)
  is the standalone workspace used by the smoke and QEMU acceptance flow.
- [`device/virtual/qemu/sdk_acceptance`](../../device/virtual/qemu/sdk_acceptance)
  shows product-side import, grants, placement, and replacement artifacts.

These examples are executable specifications. If a copied snippet diverges
from the public rules in `@bexos_sdk//rules:defs.bzl`, the exported rules and
the acceptance fixture win.
