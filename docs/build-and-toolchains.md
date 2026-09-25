# Build And Toolchains

BexOS uses Bazel for builds and code generation.

## Versions

- Bazel is pinned in `.bazelversion` to `8.7.0`.
- Rust toolchains are managed through `rules_rust` in `MODULE.bazel`.
- Protobuf/prototxt manifest validation uses `rules_proto` and `protobuf` from
  Bzlmod.
- Product assembly validation uses `//tools/assembly:bexos_assembly` through
  `//build/rules:assembly.bzl`. Board and package manifests remain `.prototxt`;
  product composition may use host-side `.star` sources through
  `//tools/config_compiler`.
- The configured Rust edition is `2024`.
- Guest Rust triples include `aarch64-unknown-none`, `aarch64-unknown-linux-gnu`,
  `x86_64-unknown-none`, `x86_64-unknown-linux-gnu`, and `wasm32-unknown-unknown`.
- `--config=aarch64` (default) and `--config=x86_64` select guest artifacts
  independently of the execution platform. Guest platforms use Rust's linker;
  LLVM C/C++ cross compiler binaries run on the execution platform.

## Common Commands

```sh
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel test //...
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build --config=aarch64-none //kernel/smoke:kernel_platform_smoke
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build --config=wasm32-unknown //drivers/d2/testing/bexos/smoke:wasm_platform_smoke
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build --config=aarch64-none //kernel
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build //idl:kernel_fidl_rust
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build //services/appd:camera_provider_manifest_bin
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel run //device/virtual/qemu/nongui:run
```

The explicit `BAZELISK_HOME` keeps Bazelisk cache writes out of restricted home-directory cache paths.

The headless QEMU wrapper is `//device/virtual/qemu/nongui:run`. ARM uses
`qemu-system-aarch64`; `--config=x86_64` selects `qemu-system-x86_64`, Q35,
and the Bazel-built Multiboot2 loader. Missing QEMU is a failure when a QEMU
test is requested. The default test configuration excludes the `requires-qemu`
tag; `--config=e2e` enables it and includes `--keep_going`.

```sh
bazel run //device/virtual/qemu/nongui:run
bazel run --config=x86_64 //device/virtual/qemu/nongui:run
bazel test --config=e2e //testing/e2e/qemu:all_architectures
bazel test //testing/build/architecture:all
```

The x86 launch path does not establish successful BexOS boot. See
[x86_64 support](x86_64-support.md) for current results, standalone
Trusty, firmware refresh, and integration boundaries. ARM retains its
TF-A/Trusty secure boot path and explicit software-TEE development product.

The Trusty superproject is pinned under `//third_party/trusty`. The target
`//third_party/trusty:trusty_qemu_firmware` builds the QEMU Trusty image,
TF-A BL1/BL2/BL31, and the upstream RPMB proxy through Bazel. The pinned app
closure includes Trusty KeyMint, Gatekeeper, storage, AVB, AuthMgr FE/BE, and
the BexOS orchestrator. The removed custom KeyVault and TUI are not built;
secure ConfirmationUI remains a future design until display/input ownership
is implemented.

The real firmware targets require host tools on `PATH`:

- Linux x86_64/ARM64 with native development packages, or Apple Silicon macOS
  with its SDK;
- Python 3, dtc, and xxd; compilers, binutils, bindgen, and protoc are Bazel-managed;
- GNU make (`gmake`), with BSD `make` used only if it is compatible;
- GNU sed for the generated TF-A platform patch;
- OpenSSL 3 headers and libraries, such as Homebrew `openssl@3`;
- Python packages `cryptography` and `pyelftools` available to the build-time
  Python used for Trusty/TF-A tooling.

Linux also needs Bison, Flex, and native libclang/LLVM development packages.
See the [native prerequisite list](../third_party/trusty/README.md#native-host-prerequisites)
and [GitHub Actions CI](ci.md) for clean-runner setup and automated checks.

## Cross-Target Shape

The target platforms live in `build/platforms`:

- `//build/platforms:kernel_aarch64`
- `//build/platforms:userspace_freestanding_aarch64`
- `//build/platforms:userspace_aarch64`
- `//build/platforms:wasm32_guest`
- `//build/platforms:secure_aarch64`
- Corresponding `kernel_x86_64`, `userspace_freestanding_x86_64`,
  `userspace_x86_64`, and `secure_x86_64` platforms.

The smoke targets are small `#![no_std]` Rust libraries. They exist to verify
that Bazel can select the requested platform and Rust target without pulling in
kernel or driver runtime decisions.

The `//kernel` target is a `#![no_std]` Rust binary selected by the guest
architecture. `--config=aarch64-none` remains supported, and
`--config=x86_64-none` selects the x86 bare-metal platform for standalone
smoke builds. The ARM implementation starts the std-linked `appd` from external BootFS, alongside the
secure orchestrator model, the Trusted UI future-state model, and shared
heart-transplant checkpoint metadata.

`//services/appd:appd_elf` is the architecture-selected std-linked ELF
packaged at `/boot/pkg/bexos.platform.appd/bin/appd`. `//services/vfsd`
builds the bootfs-launched storage broker plus a signed `.bex` archive target;
its source manifest is `services/vfsd/package/vfsd.prototxt`, compiled by Bazel
to `package.bexmanifest`. Shared startup, allocation, syscall transport, and
linker support live under `//lib/userspace`, with service `std` libc shims under
`//lib/bexos_libc`; generated package bytes are not
committed.

## Signed App Archives

`//lib/app_archive` owns the `.bex` archive format shared by host tools and the
ArchiveFS driver. The format uses Ed25519 signatures, a signed BLAKE3 content
root, per-4 KiB BLAKE3 payload hashes, and optional zstd-compressed payloads.
`//lib/redb` owns the reusable redb custom-storage wrapper for BexOS block
stores, and `//lib/app_registry` builds the system app registry schema and
transactional package lifecycle operations on top of it.

Development trust roots are converted to redb stores by Bazel:

```sh
bazel build //ecosystem/bexos/dev/roots:pki_roots_redb //ecosystem/bexos/dev/roots:app_signing_roots_redb
```

The TLS store consumes certificate files from `//ecosystem/bexos/dev/roots/pki`; the
app-signing store consumes source prototxt metadata from
`//ecosystem/bexos/dev/roots/apps`.

Build rules should use `app_archive` from `//build/rules:app_archive.bzl`:

```python
load("//build/rules:app_archive.bzl", "app_archive")

app_archive(
    name = "storage_verify_archive",
    manifest = "//services/storage_verify:manifest",
    entries = {"bin/storage_verify": "//services/storage_verify:storage_verify"},
    compression = "none",
)
```

Package manifests are protobuf text files (`.prototxt`) at source and compiled
to `.bexmanifest` outputs by Bazel before archive creation.

Structured app config follows the same pattern: manifests declare
`config_schema`, product definitions provide overrides, and `product_app_config`
or `app_config` emits `*.bexconfig` blobs. `app_archive` places those blobs at
`config/component.bexconfig` when a package opts in.

The default signing key is `//lib/app_archive:qemu_test_private.key`. It is a
checked-in QEMU/local-development test key only; production package signing and
production KeyMint enrollment and provisioned device roots are separate work. Use
`//tools/app_archive:bex_archive` to create or verify archives directly.

The QEMU app lifecycle path installs signed bundle bytes through appd,
stores archives as `STORAGE/pkg/<package>.bex` through vfsd, and launches by
mounting those archives with ArchiveFS signature verification. App manifests
remain source `.prototxt` files compiled by Bazel before archive creation.

The workstation UI is available through `bazel run --config=aarch64 //device/virtual/qemu/workstation:run` or `--config=x86_64`. It shares the existing firmware dependencies; see [QEMU products](qemu-product.md).
## Out-of-tree application SDK

`bazel build //sdk:bexos_sdk //sdk:bexos_sdk_sha256` produces the host-selected
SDK 0.2.0 archive and checksum. Supported hosts are Linux x86-64 and macOS
arm64; both archives target AArch64 and x86-64 BexOS applications. Extract the
archive, declare `bazel_dep(name = "bexos_sdk", version = "0.2.0")`, and use
`--override_module=bexos_sdk=/absolute/path/to/bexos-sdk` together with
`--override_repository=bexos_sdk=/absolute/path/to/bexos-sdk` for a local
checkout. Public app, service, driver, FIDL, and archive rules are loaded from
`@bexos_sdk//rules:defs.bzl`.

Component manifests are prototxt, must declare
`min_bexos_abi_version: 1`, and every SDK-built service process must select
`HEART_TRANSPLANT`. Archive creation always requires an explicit private key;
release SDK archives contain no private key. See
`bexos_service` and `bexos_driver` support freestanding or std-linked Rust,
FIDL dependencies and C `link_deps`; the archive also carries per-architecture
headers, `crt0.o`, `libc.a`, and the runtime archive. See
`docs/rfcs/0069/CURRENT.md` for import, early-BootFS, and policy details.
