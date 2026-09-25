# SDK And Workspace Setup

This page is for the package author. Product-side import is covered in
[Product Integration](product-integration.md).

## Prerequisites

- Bazel 8.7.0 or a compatible Bazel launcher with Bzlmod enabled.
- A supported Linux x86-64 or macOS AArch64 host.
- An SDK 0.2.0 archive and its published `.sha256` file.
- An externally provisioned Ed25519 signing key. The SDK deliberately contains
  no private key and no key-generation workflow.

Use Bazel for builds, FIDL generation, manifest compilation, archive creation,
and formatting. Do not check generated Rust, compiled manifests, or `.bex`
archives into source control unless the external project has an explicit
binary-release policy.

## Obtain and verify the SDK

SDK releases use tag `sdk-v0.2.0` and contain one archive for each supported
host. Download the archive and matching checksum from the release, then verify
it with the host checksum tool before extracting it. For example on Linux:

```sh
sha256sum --check bexos-sdk-v0.2.0-linux-x86_64.tar.gz.sha256
tar -xzf bexos-sdk-v0.2.0-linux-x86_64.tar.gz
```

The extracted root is `bexos-sdk/`. `meta/sdk.prototxt` records the SDK
version, API level, host, target architectures, and archive format. Release
automation builds the same archive with:

```sh
bazel build //sdk:bexos_sdk //sdk:bexos_sdk_sha256
```

That command is for a BexOS source checkout. An external project consumes the
resulting module; it does not need the full operating-system source tree.

## Create the external module

A minimal repository is:

```text
acme-component/
├── MODULE.bazel
├── BUILD.bazel
├── package.prototxt
├── signing.key          # local/CI secret; do not commit
└── src/
    ├── main.rs
    └── state.rs
```

Declare the versioned dependency:

```starlark
module(name = "acme_component", version = "1.0.0")

bazel_dep(name = "bexos_sdk", version = "0.2.0")
```

Until the module is supplied by a registry, pin the released archive with an
`archive_override` and the integrity value derived from the matching release
checksum:

```starlark
archive_override(
    module_name = "bexos_sdk",
    urls = [
        "https://github.com/beccahughes101/bexos/releases/download/"
        "sdk-v0.2.0/bexos-sdk-v0.2.0-linux-x86_64.tar.gz",
    ],
    integrity = "<sha256 SRI value from the verified release checksum>",
    strip_prefix = "bexos-sdk",
)
```

Select the archive matching the build host and replace the integrity placeholder
before committing the module file. Do not omit the pin or substitute an
unverified moving URL. For local SDK development, keep the module declaration
unchanged and use command-line overrides:

```sh
bazel build \
  --override_module=bexos_sdk=/absolute/path/to/bexos-sdk \
  --override_repository=bexos_sdk=/absolute/path/to/bexos-sdk \
  //:component
```

The acceptance fixture uses both overrides so it exercises exactly the
unpacked SDK instead of reaching back into the BexOS repository.

## Signing-key file

Every public package rule requires `signing_key`. The private file consumed by
`bex_archive` is text with two 32-byte, lowercase hexadecimal fields:

```text
key_id_hex=<64 lowercase hexadecimal characters>
seed_hex=<64 lowercase hexadecimal characters>
```

Provision this file through local secret storage or CI. Add it to ignore rules,
restrict its filesystem permissions, and never copy a fixture or development
seed into a release repository. The corresponding public key and key identity
must be carried in the product signing-root record; see
[Signing handoff](manifests-and-packaging.md#signing-handoff).

## First build

Load only the public rules you use:

```starlark
load("@bexos_sdk//rules:defs.bzl", "bexos_wasm_app")

package(default_visibility = ["//visibility:public"])

bexos_wasm_app(
    name = "component",
    srcs = ["src/main.rs"],
    deps = ["@bexos_sdk//rust:bexos_wasm_guest"],
    manifest = "package.prototxt",
    signing_key = "signing.key",
)
```

Build and locate the archive through Bazel:

```sh
bazel build //:component
bazel cquery --output=files //:component
```

The target produces a signed `.bex`, not an unpacked executable distribution.
Continue with [Applications](applications.md), [Services](services.md), or
[Drivers](drivers.md) before treating this minimal target as production-ready.

## What the SDK contains

- Public rules under `@bexos_sdk//rules`.
- Native component, driver-startup, service-binding, migration, and libc Rust
  source under `@bexos_sdk//rust` and its internal libraries.
- Public BexOS FIDL and manifest schemas under `@bexos_sdk//idl`.
- AArch64 and x86-64 sysroots under `@bexos_sdk//sysroot`.
- Host tools under `@bexos_sdk//tools`.
- Runnable source examples under `@bexos_sdk//examples`.

The SDK does not promise Linux/glibc compatibility. Native binaries use the
BexOS startup and syscall ABI even when `std = True` selects the SDK-provided
libc-backed Rust runtime.
