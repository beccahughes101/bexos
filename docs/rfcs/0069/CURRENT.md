# RFC 0069 current state

RFC 0069 App SDK v1 is implemented. This file describes the selected,
currently supported application scope; the RFC README retains the proposed
driver, service, sysroot, Cargo/CMake, OCI, and general image-assembler phases.

## SDK artifacts

`//sdk:bexos_sdk` selects one deterministic archive for the execution host:

- `bexos-sdk-v0.1.0-linux-x86_64.tar.gz`
- `bexos-sdk-v0.1.0-macos-aarch64.tar.gz`

`//sdk:bexos_sdk_sha256` selects the matching `.sha256` companion. Entries are
sorted and use normalized root ownership, modes, timestamps, a
`bexos-sdk/` prefix, and deterministic gzip metadata. Executable host tools use
mode 0755; source and metadata use 0644. The archive is a self-contained
Bzlmod module with pinned dependencies, public Rust application runtime source,
AArch64/x86-64 linker and platform definitions, public FIDL and manifest
sources, `fidlc`, manifest/config compilers, the BEXARCV2 tool, public rules,
examples, and `meta/sdk.prototxt`. It contains no private signing key.
The native runtime includes the startup-channel readiness primitive used by
SDK service processes.

Both host archives build portable `wasm32-wasip2` packages and native
AArch64/x86-64 BexOS ELF packages. Linux x86-64 and macOS arm64 are the v1 host
matrix.

## Bzlmod consumption

After extracting the archive, declare the SDK module and point Bazel at the
unpacked root while developing locally:

```starlark
module(name = "example_app", version = "1.0.0")
bazel_dep(name = "bexos_sdk", version = "0.1.0")
```

```sh
bazel build \
  --override_module=bexos_sdk=/opt/bexos-sdk \
  --override_repository=bexos_sdk=/opt/bexos-sdk //...
```

Load the supported API from `@bexos_sdk//rules:defs.bzl`:

- `bexos_fidl_rust_library`
- `bexos_app_manifest`
- `bexos_app_archive`
- `bexos_wasm_app`
- `bexos_native_app`

Manifests are prototxt. SDK service processes must use
`lifecycle { update_strategy: HEART_TRANSPLANT }`, and all SDK packages must
declare `min_bexos_abi_version: 1`. Native rules require
`architecture = "aarch64"` or `"x86_64"`; WASM rules stamp `MULTI`.
Every archive rule requires an explicit signing key.

The standalone fixture at `testing/out_of_tree_sdk/fixture` depends only on the
unpacked SDK module and builds generated Rust FIDL, a portable WASM service,
and native service archives for both guest architectures. Its private key is a
test-only key whose public half is already trusted by development images.

## Verification and image flow

Platform products declare imported content with
`bexos_prebuilt_app(name, archive, package_id, public_key, autoinstall)`. The
provider exposes the original archive plus a manifest extracted only after the
BEXARCV2 structure, chunk hashes, signature, and signer key have been verified.
Extraction also enforces the expected package ID, application kind, ABI level,
manifest architecture, embedded ELF machine, and heart-transplant policy.
Malformed, unsigned, tampered, wrong-signer, duplicate, future-ABI, and
architecture-mismatched inputs fail before image assembly; package code is not
executed.

Product assembly validates imported manifests together with selected in-tree
packages, dependencies, services, and duplicate IDs. Imported applications use
`SYSTEM_IMAGE` placement. The system-image manifest receives each package in
`base_packages` and, when `autoinstall = True`, in `autoinstall_packages`.
The encrypted `STORAGE` BexFS image receives the unchanged signed bytes at
`pkg/<package_id>.bex`; duplicate destinations fail. Signed default component
configuration remains authoritative. Product-specific rewriting or resigning
is outside v1. Existing QEMU products are unchanged unless passed
`prebuilt_apps`.

The signer prototxt used by assembly contains the archive's 32-byte `key_id`
and Ed25519 `public_key_hex`. The target product's application trust roots must
also authorize that public key and package namespace.

## Acceptance and release

The dedicated product is `//device/virtual/qemu/sdk_acceptance`. Its maintained
E2E scenarios require startup markers from both imported applications on
AArch64 and x86-64. The full orchestrator builds/unpacks the SDK, builds the
fixture in a separate Bazel output base using the repository override, creates
a temporary prebuilt repository, rebuilds the product with that override, and
uses the existing bounded QEMU harness:

```sh
bazel run //testing/out_of_tree_sdk:acceptance -- aarch64 qemu
bazel run //testing/out_of_tree_sdk:acceptance -- x86_64 qemu
```

The acceptance wrapper uses a disposable output base for the external fixture.
On hosts where Bazel's default output volume is small, set an absolute
`BEXOS_BAZEL_OUTPUT_USER_ROOT` on a larger volume; root-workspace SDK and QEMU
actions then use that Bazel output root while fixture cleanup remains bounded.

Tags matching `sdk-v*` trigger the SDK release workflow. The exact tag/version
match is checked before Linux x86-64 and macOS arm64 archives are built and
fixture-smoked. Linux also runs both QEMU acceptance targets. Publication
creates, rather than overwrites, a GitHub release and uploads both archives and
checksums.

## Validation evidence

Validation results are recorded here only after the corresponding Bazel
commands complete. The implementation was verified with:

- `bazel run @rules_rust//:rustfmt`
- focused SDK packaging/layout, manifest, archive verification/extraction, and
  product assembly tests under both `--config=aarch64` and
  `--config=x86_64`
- `bazel run //testing/e2e/qemu:check_matrix`
- `bazel run //testing/out_of_tree_sdk:acceptance -- aarch64 qemu`
- `bazel run //testing/out_of_tree_sdk:acceptance -- x86_64 qemu`

Both acceptance runs built the fixture from the unpacked SDK in a separate
Bazel output base, assembled the signed prebuilt archives into the requested
guest image, and passed the bounded QEMU checks for native and WASM install,
WASM compilation/instantiation, and service startup. The macOS arm64 host
archive was built locally. Linux x86-64 host archive production and smoke tests
remain enforced by the tag-only release workflow because that artifact cannot
be produced on the macOS validation host.

## Remaining phases

External driver/service SDKs, binary sysroot/libc distribution, Cargo and
CMake consumption, OCI SDK publication, product-specific external archive
rewriting/resigning, and a general GPT image assembler remain proposed and are
not v1 capabilities.
