# GitHub Actions CI

The `Bazel CI` workflow runs on every pull request, pushes to `main`, and manual
dispatch. It uses GitHub-hosted `ubuntu-26.04` x86_64 runners (currently public
preview) with the distribution's QEMU 10.2 packages. See the
[GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
and [Ubuntu package](https://packages.ubuntu.com/resolute/amd64/qemu-system-arm).
Guest selection remains
independent of the execution host. No self-hosted runner or repository secret
is required. `CI` is the aggregate check name available for branch protection;
the workflow does not change branch-protection settings.

## Jobs and commands

`Prepare firmware` runs the following in order, always with `-c opt`:

```sh
bazel run -c opt --config=aarch64 //third_party/trusty:refresh_image
bazel run -c opt --config=aarch64 //third_party/trusty:refresh_authmgr_acceptance_image
bazel run -c opt --config=x86_64 //third_party/trusty:refresh_x86_64_image
bazel run -c opt --config=x86_64 //third_party/trusty:refresh_x86_64_acceptance_image
bazel run -c opt --config=x86_64 //boot/efi:refresh_firmware
bazel run -c opt --config=x86_64 --//build/platforms:trusty_variant=acceptance //boot/efi:refresh_firmware
```

The six saved bundles are transferred to downstream jobs in a tar archive,
preserving relative paths and modes. Downloads use the current workflow run's
artifact only. Every run invokes the refresh targets; Bazel may reuse valid
cached actions. Firmware validators retain their host-ABI, hash, architecture,
and variant checks. Firmware and generated sources remain untracked.

Two build/test jobs select `aarch64` and `x86_64` respectively:

```sh
bazel build -c opt --config=aarch64 //...
bazel test -c opt --config=aarch64 --build_tests_only //...
bazel build -c opt --config=x86_64 //...
bazel test -c opt --config=x86_64 --build_tests_only //...
```

Host tests retain `.bazelrc`'s exclusion of `requires-qemu`. The ARM build/test
job also runs `bazel run //testing/e2e/qemu:check_matrix` and
`bazel run @rules_rust//:rustfmt`, then requires `git diff --exit-code` to pass.
`//tools/ci:workflow_test` validates workflow syntax and script parsing with a
checksum-pinned, Bazel-managed actionlint executable.

Four independent E2E jobs execute these maintained suite labels:

```sh
bazel test --config=e2e --nocache_test_results --test_env=BEXOS_QEMU_LIVE_LOG=1 //testing/e2e/qemu:aarch64
bazel test --config=e2e --nocache_test_results --test_env=BEXOS_QEMU_LIVE_LOG=1 //testing/e2e/qemu:x86_64
bazel test --config=e2e --nocache_test_results --test_env=BEXOS_QEMU_LIVE_LOG=1 //testing/e2e/qemu:x86_64_development
bazel test --config=e2e --nocache_test_results --test_env=BEXOS_QEMU_LIVE_LOG=1 //testing/e2e/qemu:firmware_acceptance
```

`--config=e2e` selects optimized guests and keeps going after failures. Existing
test deadlines and exclusive execution are preserved. Performance suites remain
opt-in; no functional scenarios receive new exclusions, retries, or tolerated
failures. The top-level inventory includes the existing Dioxus smoke,
preferences, and SysUI boot/migration/recovery scenarios on both guest
architectures. Existing runtime failures make CI fail.

## Runner setup and resources

The shared composite action installs the native Trusty prerequisites, Python
cryptography/ELF modules, and QEMU ARM/x86 packages. It places `/usr/bin` first
on PATH so distro Python and its modules agree. Bazelisk reads `.bazelversion`;
compilers, Rust, and code generators remain managed by Bazel.

The setup script is restricted to disposable GitHub-hosted Linux runners. It
reclaims unused .NET, Android, and Haskell SDK directories before fetching the
large build dependencies. Jobs log disk availability and tool versions, limit
Bazel to two concurrent actions and 65% of RAM, and limit local test concurrency
to one. Build, firmware, and E2E jobs have a 360-minute limit. A new run cancels
an older run for the same PR or branch; an individual matrix failure does not
cancel sibling jobs.

Bazelisk and repository downloads are cached. Build caches are separated by
Ubuntu version, host architecture, and job role. PRs can restore caches but do
not save them. Actions use immutable commit pins, checkout does not retain
credentials, and the workflow token has only `contents: read` permission.

## Diagnostics and validation

Each job attempts guest-process cleanup and diagnostic upload even after a
failure. Normal test harness cleanup remains primary; the runner cleanup step
terminates remaining QEMU/RPMB helpers without stopping Bazel. Diagnostics
contain command logs, available Bazel `test.log`/`test.xml` files, and undeclared
test outputs from every Bazel configuration, retained for seven days. Firmware
transport artifacts expire after one day. A setup or compilation failure can legitimately produce no test logs;
the diagnostic inventory states that explicitly.

The aggregate `CI` check fails if any required job fails, is cancelled, or is
skipped. Check the corresponding job and its diagnostics to distinguish setup,
compilation, and runtime failures. Hard runner termination may prevent final
cleanup/upload steps; GitHub disposes of the runner VM after the job.

Implementation validation is recorded in [Testing Status](testing-status.md).
Local validation does not establish that a GitHub-hosted run passed. A first
hosted run is required to measure cold-cache disk use, full-suite duration,
and runtime outcomes on Ubuntu 26.04.
