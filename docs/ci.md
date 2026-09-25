# GitHub Actions CI

The `Bazel CI` workflow has separate pull-request and branch paths. Pull
requests build and test affected Bazel targets without executing QEMU tests.
Pushes to `main` and manual dispatches build the complete repository, publish
the resulting build cache, and then execute the maintained E2E inventory with
one concrete Bazel test per runner.

All jobs use GitHub-hosted `ubuntu-26.04` x86_64 runners and the distribution's
QEMU 10.2 packages. Guest architecture remains independent of the execution
host. `CI` is the aggregate check available for branch protection; the workflow
does not modify branch-protection settings.

## Pull requests

The AArch64 and x86_64 jobs run in parallel after firmware preparation. Each
job compares the PR base SHA with `HEAD`, maps changed files to their nearest
Bazel package, and selects that package's targets plus their repository-wide
reverse dependencies. It writes separate target-pattern files for builds and
tests. The test selection explicitly removes targets tagged `requires-qemu`, so
no E2E or firmware-acceptance test executes on a pull request.

Changes that cannot be mapped safely select `//...`. This conservative fallback
applies to module/workspace configuration, `.bazelrc`, `.bzl` files, deletions,
renames, and submodule changes. Changes under `.github` or `tools/ci` always
select `//tools/ci:workflow_test`. An empty target file is a successful no-op
instead of a repository-wide build; documentation-only changes produce empty
target files unless they also contain a conservative-fallback change.

The architecture jobs use the equivalent of:

```sh
bazel build -c opt --config=aarch64 \
  --target_pattern_file="$RUNNER_TEMP/bexos-build-targets"
bazel test -c opt --config=aarch64 --build_tests_only \
  --target_pattern_file="$RUNNER_TEMP/bexos-test-targets"
```

The x86_64 job substitutes `--config=x86_64`. Pull requests may restore the
persistent build caches but do not publish run-scoped caches, because no E2E
job consumes them.

## Main and manual builds

The two architecture jobs perform the full optimized build and non-E2E test
pass:

```sh
bazel build -c opt --config=aarch64 //...
bazel test -c opt --config=aarch64 --build_tests_only //...
bazel build -c opt --config=x86_64 //...
bazel test -c opt --config=x86_64 --build_tests_only //...
```

`.bazelrc` excludes `requires-qemu` from ordinary `bazel test`, so these jobs
compile the repository and E2E artifacts without running guest tests. The ARM
job also validates the E2E inventory and checks `rustfmt` without changing the
checkout. Both jobs must finish successfully before E2E begins.

Each successful architecture job saves `~/.cache/bazel-disk` under a key scoped
to the workflow run, attempt, and guest architecture. The normal setup cache
remains the seed for later full builds; the immutable run cache is the exact
handoff to E2E jobs.

## Firmware and E2E

`Prepare firmware` refreshes the standard and acceptance Trusty bundles for
AArch64 and x86_64 plus standard and acceptance EFI bundles. The six gitignored
files are transferred to every downstream job in one uncompressed, one-day
artifact. Downloads always name the current workflow run's artifact.

For main and manual runs, the firmware job queries the maintained suites:

```text
//testing/e2e/qemu:aarch64
//testing/e2e/qemu:x86_64
//testing/e2e/qemu:x86_64_development
//testing/e2e/qemu:firmware_acceptance
```

The generator expands suites to concrete tests, sorts and deduplicates labels,
assigns architecture-specific x86 labels to the x86 cache and all other labels
to the ARM cache, and emits the GitHub matrix. The current inventory is 178
jobs, below GitHub's 256-job matrix limit. Focused and performance tiers remain opt-in.
There is no workflow `max-parallel`; actual concurrency is controlled by the
GitHub account's runner quota.

Every E2E runner restores the exact architecture cache with
`fail-on-cache-miss`, restores the same firmware archive, and selects one label
with `--config=e2e` plus its architecture configuration. A build preflight
materializes the target while recording Bazel's execution log. The cache guard
requires every non-test action to report a cache hit; if any compilation or
generation action executes, the job fails before starting QEMU. The subsequent
`bazel test` uses the materialized outputs and keeps test-result caching
disabled.

## Runner setup and diagnostics

The shared setup action installs native Trusty prerequisites, Python
cryptography/ELF modules, and QEMU ARM/x86 packages. Bazelisk reads
`.bazelversion`; compilers, Rust, and generators remain Bazel-managed. Jobs use
two build actions, 65% of RAM, and one local test at a time. A new run cancels
an older run for the same PR or branch, while a matrix failure does not cancel
sibling jobs.

Each job attempts guest-process cleanup and diagnostic collection even after a
failure. Diagnostics include command logs, Bazel profiles, test logs/XML, and
undeclared outputs and are retained for seven days. E2E artifact names use a
stable numeric matrix ID while the job name displays the complete Bazel label.
The firmware transport artifact expires after one day.

The aggregate `CI` job requires firmware and both architecture jobs on every
event. Pull requests require the E2E job to be skipped; main and manual runs
require the complete E2E matrix to pass. A first hosted run after CI changes is
still required to validate cache archive size and restoration, runner fan-out,
Trusty artifact reuse, and end-to-end wall time. Local validation does not
establish those hosted results.

Implementation validation is recorded in [Testing Status](testing-status.md).
## SDK release CI

Pushing an `sdk-v*` tag runs `.github/workflows/sdk-release.yml`. The tag must
exactly match the checked-in SDK version. Linux x86-64 and macOS arm64 jobs
build and smoke-test the standalone fixture; Linux additionally runs both QEMU
guest architectures. Publication creates a new GitHub release and uploads:

- `bexos-sdk-v0.1.0-linux-x86_64.tar.gz` and its `.sha256`
- `bexos-sdk-v0.1.0-macos-aarch64.tar.gz` and its `.sha256`

The workflow fails on an existing release, a version mismatch, or an
incomplete asset set; it never overwrites a release.
