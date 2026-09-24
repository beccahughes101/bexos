#!/usr/bin/env bash
# Run after Bazel releases its build lock (bazel run), so query can inspect
# every package, including packages absent from the maintained inventory.
set -euo pipefail
cd "${BUILD_WORKSPACE_DIRECTORY:?run with bazel run //testing/e2e/qemu:check_matrix}"
all_qemu='(attr("tags", "requires-qemu", kind(".*_test rule", deps(//testing/e2e/qemu:declared_tests))) except tests(//testing/e2e/qemu:firmware_acceptance))'
tiered='attr("tags", "e2e-tier-(presubmit|extended|focused|performance)", kind(".*_test rule", //...))'
untiered=$("${BAZEL_REAL:-bazel}" query --noshow_progress --output=label "$all_qemu except $tiered")
if [[ -n "$untiered" ]]; then
    echo 'Runnable QEMU targets without exactly one tier:' >&2
    printf '%s\n' "$untiered" >&2
    exit 1
fi
for left in presubmit extended focused performance; do
    for right in presubmit extended focused performance; do
        [[ "$left" < "$right" ]] || continue
        overlap=$("${BAZEL_REAL:-bazel}" query --noshow_progress --output=label \
          "attr(\"tags\", \"e2e-tier-$left\", $all_qemu) intersect attr(\"tags\", \"e2e-tier-$right\", $all_qemu)")
        if [[ -n "$overlap" ]]; then
            echo "Runnable QEMU targets in both $left and $right tiers:" >&2
            printf '%s\n' "$overlap" >&2
            exit 1
        fi
    done
done
uncovered=$("${BAZEL_REAL:-bazel}" query --noshow_progress --output=label \
  'attr("tags", "requires-qemu", kind(".*_test rule", //...)) except tests(set(//testing/e2e/qemu:all_architectures //testing/e2e/qemu:x86_64_development //testing/e2e/qemu:focused_aarch64 //testing/e2e/qemu:focused_x86_64 //testing/e2e/qemu:focused_x86_64_development //testing/e2e/qemu:firmware_acceptance //testing/e2e/qemu:performance)) except labels("test", kind("architecture_test rule", //...))')
if [[ -n "$uncovered" ]]; then
    echo 'Runnable QEMU scenarios missing from maintained matrices:' >&2
    printf '%s\n' "$uncovered" >&2
    exit 1
fi
echo 'Every runnable QEMU scenario has one tier and belongs to a maintained matrix.'
