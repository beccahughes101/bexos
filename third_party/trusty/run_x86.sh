#!/bin/sh
set -eu
for script in \
  "${RUNFILES_DIR:-$0.runfiles}/_main/third_party/trusty/run_x86.py" \
  "$(dirname "$0")/run_x86.py" \
  "third_party/trusty/run_x86.py"; do
  if [ -f "$script" ]; then
    exec python3 -B "$script" "$@"
  fi
done
echo "Standalone Trusty runner is missing its Bazel runfiles" >&2
exit 1
