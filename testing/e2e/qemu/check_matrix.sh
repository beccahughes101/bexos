#!/usr/bin/env bash
# Run after Bazel releases its build lock (bazel run), so query can inspect
# every package, including packages absent from the maintained inventory.
set -euo pipefail
cd "${BUILD_WORKSPACE_DIRECTORY:?run with bazel run //testing/e2e/qemu:check_matrix}"
uncovered=$("${BAZEL_REAL:-bazel}" query --noshow_progress --output=label \
  'attr("tags", "requires-qemu", kind(".*_test rule", //...)) except tests(set(//testing/e2e/qemu:all_architectures //testing/e2e/qemu:x86_64_development //testing/e2e/qemu:firmware_acceptance //testing/e2e/qemu:performance)) except labels("test", kind("architecture_test rule", //...))')
if [[ -n "$uncovered" ]]; then
    echo 'Runnable QEMU scenarios missing from maintained matrices:' >&2
    printf '%s\n' "$uncovered" >&2
    exit 1
fi
echo 'Every runnable QEMU scenario belongs to a maintained matrix.'
