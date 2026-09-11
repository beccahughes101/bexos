#!/usr/bin/env bash
set -euo pipefail
for script in \
  "${RUNFILES_DIR:-$0.runfiles}/_main/third_party/trusty/image_bundle.py" \
  "$(dirname "$0")/image_bundle.py" \
  "third_party/trusty/image_bundle.py"; do
  if [[ -f "$script" ]]; then
    exec python3 "$script" "$@"
  fi
done
echo "Trusty bundle tool is missing its Bazel runfiles" >&2
exit 1
