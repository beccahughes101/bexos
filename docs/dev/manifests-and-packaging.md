# Manifests, Packaging, And Signing

The source of truth for an SDK package is a prototxt `bexos.app.Manifest`.
Bazel compiles it to `package.bexmanifest`, stamps the selected architecture,
generates the default component configuration, and signs both code and metadata
into a `BEXARCV2` archive. Generated protobuf and archive files are Bazel
outputs and should not be committed.

## Required package identity

Every SDK package needs:

```textproto
package_name: "com.acme.component"
name: "ACME component"
min_bexos_abi_version: 1
```

`package_name` is the stable installation and policy identity. It must match
the product import and, for a driver, `driver_info.package_id`. API level 1
rejects a missing/zero ABI or a value above the product maximum.

The SDK rules stamp `architecture`; do not maintain separate hand-edited
architecture fields in otherwise shared source manifests. Native output is
stamped `AARCH64` or `X86_64`. Portable output is stamped `MULTI`.

## Processes and runner paths

An ELF runner path must be below `/pkg/bin/`, contain no traversal, and match
the `binary_name` used by the rule. A WASM rule stores `<name>.wasm` in the
same directory. Every service process must select `HEART_TRANSPLANT`.

The component type is part of the archive build contract:

- An application cannot contain driver metadata or bind rules.
- A service package contains only service processes and no driver metadata.
- A driver contains exactly one service process, matching driver identity, and
  at least one valid bind rule.

These constraints are checked when the SDK creates the archive and again when
the product imports it.

## Public rule reference

The stable load surface is `@bexos_sdk//rules:defs.bzl`.

```starlark
bexos_wasm_app(
    name,
    srcs,
    manifest,
    signing_key,
    crate_name = None,
    deps = [],
    data = {},
    compression = "zstd",
)

bexos_native_app(
    name,
    srcs,
    manifest,
    signing_key,
    architecture,
    crate_name = None,
    binary_name = None,
    deps = [],
    data = {},
    compression = "zstd",
)

bexos_service(
    name,
    srcs,
    manifest,
    signing_key,
    architecture,
    crate_name = None,
    binary_name = None,
    deps = [],
    fidl_deps = [],
    link_deps = [],
    data = {},
    compression = "zstd",
    std = False,
)

bexos_driver(
    name,
    srcs,
    manifest,
    signing_key,
    architecture,
    crate_name = None,
    binary_name = None,
    deps = [],
    fidl_deps = [],
    link_deps = [],
    data = {},
    compression = "zstd",
    std = False,
)
```

For lower-level composition, `bexos_app_manifest` compiles and stamps a source
manifest, while `bexos_app_archive` packages explicit entries. Prefer the four
component macros unless the package shape genuinely requires manual entries:

```starlark
bexos_app_manifest(name, src, architecture)

bexos_app_archive(
    name,
    manifest,
    signing_key,
    entries,
    compression = "zstd",
    component_type = "application",
)
```

`architecture` for `bexos_app_manifest` is the protobuf spelling `AARCH64`,
`X86_64`, or `MULTI`; component macros accept lowercase `aarch64` or `x86_64`.

## Archive contents

A generated archive contains at least:

```text
package.bexmanifest
config/component.bexconfig
bin/<component>[.wasm]
```

Additional `data` entries retain their archive-relative names. `BEXARCV2`
sorts its entry table, optionally compresses entries with zstd, hashes 4 KiB
chunks with BLAKE3, signs the content root with Ed25519, and records the signer
key ID. Import validates the archive structure, chunks, signature, normalized
paths, manifest contract, architecture, and every ELF payload machine.

Inspect a built archive without extracting or executing it:

```sh
bazel run @bexos_sdk//tools:bex_archive -- inspect \
  --archive /absolute/path/to/component.bex \
  --public-key /absolute/path/to/signing-root.prototxt
```

The command prints verified entry names, sizes, compression, and content root.
Use `bazel cquery --output=files //:target` to obtain the Bazel output path.

## Component configuration

`config_schema` declares typed defaults and constraints in the signed manifest.
The package rule generates the matching `BEXCFG` default blob. Product-side
rewriting or resigning is not part of SDK 0.2, so an external archive retains
its signed defaults. Runtime overrides are a separate product/appd policy
decision.

## Signing handoff

The package author hands the integrator:

1. The exact signed `.bex` bytes.
2. The expected package ID and component type.
3. Target architecture, or `MULTI` for portable content.
4. Whether placement is `SYSTEM_IMAGE` or explicitly authorized `BOOTFS`, plus
   the signed boot wave for `BOOTFS`.
5. The public signing-root prototxt and its `anchor_id`/signer identity.
6. Release digest/version metadata and, for an update, the intended migration
   generation policy.

A product signing-root record used by the importer contains an anchor identity,
key ID, and Ed25519 public key, for example:

```textproto
anchor_id: "acme-production-root"
key_id: "acme-component-signing-key-v0001"
public_key_hex: "<64 lowercase hexadecimal characters>"
```

The textual `key_id` must be exactly 32 bytes and must encode the same bytes as
the private key file's `key_id_hex`.

The product's trust configuration must independently authorize the root and its
package namespace. Do not send the private `seed_hex`; the product needs only
the public record.

## Packaging checklist

- Package ID, process names, binary paths, and driver identity agree.
- Native targets are built separately for every supported architecture.
- Service and driver manifests declare heart transplant and realistic timeouts.
- Data paths are relative, normalized, and rooted in the package.
- The release archive is signed by the key represented by the handed-off root.
- The archive verifies through the SDK tool before product integration.
