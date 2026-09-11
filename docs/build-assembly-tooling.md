# Build, Assembly, And Tooling

Guest selection is controlled by `--config=aarch64` (default) or `--config=x86_64`.
Kernel, userspace, secure platforms and product packaging follow it; host tools stay on
the execution platform. See [x86_64 support](x86_64-support.md) for the toolchain and
Multiboot2 contracts.

## Bazel Workspace

The repository is built with Bazel and Bzlmod. Rust targets use `rules_rust`; protobuf generation uses `protoc`; platform artifacts are assembled by custom Starlark rules and Rust/Python tools.

Important build packages:

- `//build/platforms`: target platforms including AArch64 no-std, WASM, and secure-world-related platforms.
- `//build/rules`: custom rules for FIDL, userspace binaries, app manifests, app archives, product assembly, and component config.
- `//ecosystem/bexos`: selected BexOS ecosystem profile, defaulting to dev,
  with dev/prod profile packages under `//ecosystem/bexos/{dev,prod}`.
- `//tools/fidlc`: BexOS FIDL compiler.
- `//tools/app_archive:bex_archive`: signed app archive tool.
- `//tools/assembly:bexos_assembly`: product/config assembly tool.
- `//tools/image`: BootFS, boot handoff, BexFS, GPT disk, and manifest validation tools.
- `//tools/qemu:qemu_runner`: host-side QEMU runner support.

## Userspace Binary Rule

`userspace_binary` in `build/rules/userspace.bzl` wraps `rust_binary` for architecture-selected freestanding components such as D1 drivers. It applies:

- platform `//build/platforms:userspace_freestanding_aarch64` or `:userspace_freestanding_x86_64`;
- architecture-specific linker script under `//lib/userspace`;
- panic abort;
- static relocation model;
- optimized, stripped output;
- common dependencies on `//lib/userspace` and `//idl:kernel_fidl_rust`.

`std_service_binary` wraps service ELFs under `//services` for the
selected `aarch64-unknown-linux-gnu` or `x86_64-unknown-linux-gnu` Rust target, preserving the BexOS `_start(channel)`
ABI while linking `//lib/bexos_libc`. Current userspace ELF targets include
appd, debugd, traced, vfsd, update_engine, powerd, users,
keychain, netstackd, timed, teed, prefsd, storage_verify, and D1 drivers.

## App Manifests

`app_manifest` compiles prototxt files into `.bexmanifest` binary protobufs using `bexos.app.Manifest`.

Current manifest sources include:

- `services/appd/package/appd.prototxt`
- `services/debugd/package/debugd.prototxt`
- `services/traced/package/traced.prototxt`
- `services/vfsd/package/vfsd.prototxt`
- `services/updated/package/updated.prototxt`
- `services/powerd/package/powerd.prototxt`
- `services/usersd/package/usersd.prototxt`
- `services/prefsd/package/prefsd.prototxt`
- `services/keychaind/package/keychaind.prototxt`
- `services/timed/package/timed.prototxt`
- `services/storage_verify/manifest.prototxt`
- `drivers/d1/<type>/<vendor>/<device>/package/*.prototxt`

## App Archives

`app_archive` in `build/rules/app_archive.bzl` builds signed `.bex` archives with `//tools/app_archive:bex_archive`.
The default signer comes from `//ecosystem/bexos:app_signing_key`. Archive
bytes are v2-only (`BEXARCV2`/`BEXSIGV2`) and include algorithm, signer-chain
bytes, BLAKE3 content digest, and signature; v1 archive magic is rejected.

Current replacement archives exist for appd, debugd, vfsd, updated, prefsd, and D1 drivers. They package a replacement ELF plus manifest, generally with zstd compression. These are used by tests and update/migration flows, not committed as generated files.

## Product Assembly

`assembly_input_bundle`, `bexos_product`, and `product_app_config` in `build/rules/assembly.bzl` compile prototxt product inputs and produce:

- product definition binary;
- assembly index;
- bootfs label list;
- system-image label list;
- package-specific component config blobs.

Component config blobs use the `BEXCFG` table format. Assembly now emits v2
blobs with schema fingerprint and generation metadata while userspace continues
to read v1 blobs for compatibility. The `component_config_rust` rule invokes
`//tools/assembly:bexos_assembly config-rust` to generate a Rust
`ComponentConfig` binding from the compiled prototxt manifest schema; generated
files remain Bazel outputs and are not checked in.

Writable component configuration shares validation with runtime through
`//lib/component_config`. Manifest fields support preference scope, display
metadata, unsigned ranges, and string enums. `component_config_rust` also emits
an opt-in `LiveComponentConfig` binding. `product_app_config_policy` validates
explicit product `locked_fields` and emits a binary protobuf policy; pass it to
`app_archive(config_policy=...)` to include `config/component.bexpolicy`.
See [preference validation](services.md#preference-validation) for schema,
assembly, generated-binding, and QEMU coverage.

`//device/virtual/qemu/nongui` and `//device/virtual/qemu/workstation` are
architecture-selected products. Each has a `products.star` definition that
loads shared QEMU configuration and compiles on the host before assembly
validation. Workstation additionally selects the portable and QEMU graphics
bundles. Board, policy, package, and QEMU launch inputs remain prototxt. Storage-preinstalled QEMU archives are collected by
`QEMU_STORAGE_PREINSTALLS` in `device/virtual/qemu/base/images.bzl`; see
[Storage Preinstalled Apps](storage-preinstalled-apps.md).
The QEMU BootFS image embeds `//ecosystem/bexos:public_bundle` at
`/system/ecosystem/bexos.bundle`, plus the selected TLS and app-signing root
stores.

## Image Tools

`//tools/image` provides:

- `assemble_bootfs.py`: builds BootFS image contents from explicit entries and ELF files.
- `validate_bootfs_manifest.py`: validates bootfs manifest package labels against assembled product labels.
- `boot_handoff`: writes the versioned kernel handoff, verified-boot evidence,
  and layout script. Secure/RPMB flags are valid only when their verified
  backends are active.
- `bexfs_image`: creates BexFS images, including initial system state.
- `generate_gpt_disk.py`: combines partition images into a GPT disk.

The QEMU board target uses these tools to build `bootfs.img`,
`boot_handoff.bin`, `boot_evidence.bin`, `boot_layout.sh`,
`sys_state.bexfs.img`, and `qemu_nvme_gpt.img`.
The secure runfiles also include Bazel-built TF-A/Trusty images, upstream
`rpmb_dev`, and the development `RPMB_DATA` template.

## Common Commands

Build the QEMU kernel and boot artifacts:

```sh
bazel build //device/virtual/qemu/nongui:virtual_aarch64
```

Run the QEMU developer instance:

```sh
bazel run //device/virtual/qemu/nongui:run
```

Use the debug client from another terminal:

```sh
bazel run //device/virtual/qemu/nongui:debugd -- health
bazel run //device/virtual/qemu/nongui:debugd -- ps
bazel run //device/virtual/qemu/nongui:debugd -- exec debugd.version
bazel run //device/virtual/qemu/nongui:debugd -- trace record --duration-ms 5000 -o /tmp/bexos.pftrace
```

Format Rust:

```sh
bazel run @rules_rust//:rustfmt
```
