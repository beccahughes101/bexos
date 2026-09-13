# Trusty QEMU firmware

`//third_party/trusty:trusty_qemu_firmware` builds the pinned Trusty
superproject with the legacy TF-A `SPD=trusty` dispatcher for QEMU. The image
contains upstream Trusty storage, KeyMint, Gatekeeper, AVB, AuthMgr FE/BE, and
the retained BexOS orchestrator TA. These seven functional TAs are accompanied
by the required upstream hwcrypto, hwbcc, hwcryptohal, and system-state
providers. ConfirmationUI is intentionally excluded
until the platform has secure display/input ownership.

The build requires `git`, GNU Make (`gmake` on macOS), `python3`, `dtc`, and
`xxd` on the host. The compiler, binutils, Rust compiler, bindgen, libclang,
and protoc inputs come from Bazel-managed toolchains. The wrapper patches the
pinned upstream tree in a temporary Bazel output directory; generated firmware
outputs are not committed.

Both variants retain debug assertions but omit embedded TA symbol tables and
use size optimization with inlining. This leaves space for secure task heaps,
page tables, and storage buffers in QEMU's fixed secure RAM region. The bundle
retains `lk.elf` for host debugging. Rust diagnostics use native Trusty writes.
Both variants configure four secure CPUs to match the QEMU machine. The kernel
checks Trusty's reported CPU capacity before invoking PSCI on secondary CPUs;
older single-CPU bundles need an explicit refresh.

This QEMU target is a development contract only. Production boards must provide
provisioned AVB roots, authenticated BL33 loading, RPMB, and a working Trusty
SMC/shared-memory transport. Missing support fails closed.

## Native host prerequisites

The source-build rules select native tools independently of the Trusty guest
architecture. Supported host selections are Linux x86_64, Linux ARM64, and
Apple Silicon Darwin. Both ARM and x86 firmware use the pinned Rust 1.80.1
compiler and nightly/2026-07-16 formatter for that host. AIDL (including its
Bison/Flex parser) and the RPMB helper are built from pinned sources by Bazel;
Linux ARM64 does not require x86 emulation.

Install native C/C++ development headers/libraries, GNU make, GNU sed, Python 3,
`dtc`, `xxd`, Bison, Flex, OpenSSL development headers/libraries, and native
libclang. On Ubuntu/Debian, the additional package names are
`build-essential bison flex device-tree-compiler libclang-dev llvm-dev libssl-dev`.
On Darwin retain Xcode command line tools and the macOS SDK, GNU sed (`gsed`),
and Homebrew OpenSSL 3 and Bison 3 or newer (`brew install bison`). Xcode's
Bison 2.3 cannot generate the AIDL grammar. Bazel discovers Homebrew's keg-only
Bison in either standard prefix; a custom installation can be selected with
`--repo_env=BISON=/absolute/path/to/bison`.
Darwin's SDK and linker are used only for host tools;
they are not Linux build dependencies.

For a nonstandard Linux LLVM installation, pass
`--repo_env=LIBCLANG_PATH=/path/to/libclang-directory` (or the full library
filename) to Bazel. Otherwise the rule checks `llvm-config`, versioned
`llvm-config` commands, and conventional native LLVM library directories.
An invalid explicit override fails with a dependency diagnostic instead of
silently selecting another installation.

`bazel test //third_party/trusty:host_tools_tests` exercises native tool startup,
AIDL Rust/C++ generation, RPMB initialization, SDK selection, and compatibility
patches for both guest architectures. Source-build and runtime validation are
recorded separately in `docs/trusty-linux-build.md`; host selection alone
is not evidence that a native build passed on a different machine.

## Explicit firmware refresh

Run `bazel run //third_party/trusty:refresh_image` to compile firmware and
atomically save `third_party/trusty/image.bin`. QEMU runs and tests consume
this gitignored snapshot through `cached_firmware`, which validates the bundle
and extracts declared Bazel outputs. They do not depend on the compilation
rule. Source-build and refresh targets are tagged `manual`, so wildcard
repository builds also use the saved firmware. A missing or invalid bundle
fails with the refresh command.

`image.bin` is a tar firmware bundle, not a raw guest disk. It contains BL1,
BL2, BL31, Trusty (`lk.bin` and `lk.elf`), the matching signed BL33 verifier,
TF-A certificates, the host RPMB helper, a pristine RPMB template, and
`build.prototxt` with the format version, image variant, host ABI, artifact
sizes, and SHA-256 digests. Writable RPMB state belongs to the QEMU instance
outside the bundle and persists across that instance's reboots.

Source changes never refresh a saved bundle automatically. Refresh after
changing Trusty, TF-A, secure applications, firmware build settings, BL33, or
its signing inputs. Changes confined to the normal-world kernel, services,
drivers, and BootFS reuse the saved firmware. Failed refreshes preserve the
previous complete bundle. The bundle includes a host executable and must be
refreshed when changing host operating system or architecture.

AuthMgr acceptance uses a separate image with its test service and client:

```sh
bazel run //third_party/trusty:refresh_authmgr_acceptance_image
bazel test //testing/e2e/qemu/trusty:authmgr_acceptance_e2e_test --test_tag_filters=requires-qemu
```

Its gitignored `authmgr_acceptance_image.bin` cannot be used as the standard
bundle. Acceptance application sources are excluded from the standard
firmware build.

## Verification results

The results below describe the earlier repository baseline. Results for the
native Linux host portability changes are tracked separately in
`docs/trusty-linux-build.md`.

Bazel rustfmt and all 85 repository test targets pass, including 114 kernel
host tests. The complete tagged QEMU suite passes all 25 targets. Uncached
standalone AuthMgr acceptance also passes with the isolated four-CPU bundle.
Coverage includes authenticated boot and tamper rejection, KeyMint, Gatekeeper
password replacement, secure-storage restart persistence, protected application
discovery, and service/kernel handover with rollback and resource reclamation.

The full ten-service chain passes with appd at 70 ms. Appd batches persistent
activation writes and flushes both backing volumes before readiness. Retired
private memory is scrubbed in bounded kernel maintenance before allocator reuse;
the e2e tests require complete postcommit reclamation.

The final action graph for all 64 QEMU targets has no firmware compilation
script inputs. A repeated uncached appd run passes with both bundle hashes
unchanged and only two local test actions. QEMU and RPMB helpers are reaped.
See `docs/testing-status.md` for the verified commands and results.
