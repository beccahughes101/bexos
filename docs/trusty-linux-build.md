# Trusty native host build portability

The build selects native Rust tools, libclang, AIDL, and RPMB helpers for Linux
x86_64, Linux ARM64, and Apple Silicon Darwin. Firmware guest architecture is
independent of the execution host. macOS SDK inputs are confined to Darwin.

AIDL and its host dependencies use the pinned Trusty superproject, plus the
Android 16 liblog revision pinned in `MODULE.bazel`. Parser generation is a
Bazel action. The separately compiled RPMB helper retains the zero-response
handover behavior. Shared target descriptor and character adaptations are
applied for both Linux and Darwin; Mach-O linker and proc-macro suffix handling
remain Darwin-specific.

## Validation

Validation on Linux x86_64 (2026-09-10):

- All five source firmware builds passed through Bazel: ARM standard and
  acceptance, plus x86 standard, acceptance, and replacement.
- The six focused host-tool tests passed, including native AIDL Rust/C++
  generation, RPMB initialization, Linux SDK exclusion, missing dependency
  diagnostics, dependency ordering, and both guest ABI patch variants.
- Existing bundle and x86 launcher unit tests passed.
- All four standard/acceptance bundles refreshed through Bazel. Packing and
  refresh validated their hashes, guest ELF architectures, variants, and
  `Linux-x86_64` host ABI metadata. Firmware actions initialized their RPMB
  templates with the native helper.
- `bazel run @rules_rust//:rustfmt` passed. An obsolete `bazel-bexos` output
  symlink from the previous checkout name initially polluted its workspace
  query; removing that generated symlink resolved the formatter failure.
- The x86 standalone boot test passed in 8.1 seconds on QEMU 10.2.1. The first
  x86 acceptance run reached storage generation 1, KeyMint HMAC/deletion, and
  AuthMgr acceptance completion, but exceeded the default 600-second deadline
  during Gatekeeper storage operations. A retry with a longer deadline is
  pending; this is not yet a passing acceptance result.
- ARM runtime validation exposed two native host compile prerequisites in the
  existing fixtures: Linux proc-macro triples for the shell component, and a
  bare-metal gate for the monitor IOMMU module matching its clock dependency.
  Both fixes are included. Runtime validation is still in progress.

This machine lacked several system development packages. Validation uses
native Ubuntu packages unpacked under `/tmp/trusty-host-packages`, supplied
through Bazel's repository/action PATH, a sandbox mount, and `LIBCLANG_PATH`.
No downloaded utilities or generated firmware are tracked in Git.

Native Linux ARM64 and Darwin machines are not available in this task; no
successful builds on those hosts are claimed. See
`third_party/trusty/README.md` for prerequisites and source-build/refresh
commands.

## Validation commands

With the documented host prerequisites installed:

```sh
bazel test //third_party/trusty:host_tools_tests //third_party/trusty:image_bundle_tests //third_party/trusty:run_x86_tests
bazel build //third_party/trusty:trusty_qemu_firmware //third_party/trusty:authmgr_acceptance_firmware //third_party/trusty:trusty_x86_64_firmware //third_party/trusty:x86_64_acceptance_firmware //third_party/trusty:x86_64_replacement_firmware
bazel run //third_party/trusty:refresh_image
bazel run //third_party/trusty:refresh_authmgr_acceptance_image
bazel run //third_party/trusty:refresh_x86_64_image
bazel run //third_party/trusty:refresh_x86_64_acceptance_image
bazel test --config=e2e --nocache_test_results //third_party/trusty:x86_64_boot_test //third_party/trusty:x86_64_acceptance_test //testing/e2e/qemu/trusty:trusty_security_stack_e2e_test_aarch64 //testing/e2e/qemu/trusty:authmgr_acceptance_e2e_test_aarch64
bazel run @rules_rust//:rustfmt
```

For this task's temporary package installation, build and refresh commands
also use:

```text
--sandbox_add_mount_pair=/tmp/trusty-host-packages
--repo_env=PATH=/tmp/trusty-host-packages/bin:/usr/local/bin:/usr/bin:/bin
--action_env=PATH=/tmp/trusty-host-packages/bin:/usr/local/bin:/usr/bin:/bin
--repo_env=LIBCLANG_PATH=/tmp/trusty-host-packages/root/usr/lib/llvm-21/lib
```

Runtime tests additionally receive that PATH via `--test_env=PATH=...` to find
the native ARM QEMU executable. These temporary paths are validation setup,
not repository build defaults.
