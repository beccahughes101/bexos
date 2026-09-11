#!/bin/sh
set -eu
export PYTHONDONTWRITEBYTECODE=1
root="${RUNFILES_DIR:-$0.runfiles}/_main"
if [ ! -f "$root/testing/performance/scened/run.py" ]; then
    root="${TEST_SRCDIR:?missing Bazel runfiles}/${TEST_WORKSPACE:-_main}"
fi
exec python3 "$root/testing/performance/scened/run.py" "$root" "$@"
