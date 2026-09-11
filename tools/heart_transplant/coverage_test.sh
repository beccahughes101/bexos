#!/usr/bin/env bash
set -euo pipefail

repo="${BUILD_WORKSPACE_DIRECTORY:-${TEST_SRCDIR}/${TEST_WORKSPACE}}"
missing=0

while IFS= read -r manifest; do
  case "$manifest" in
    */testing/e2e/*) continue ;;
  esac
  package_dir="${manifest%/package/*}"
  build_file="${package_dir}/BUILD.bazel"
  if [[ ! -f "$build_file" ]]; then
    echo "missing BUILD.bazel for ${manifest#${repo}/}"
    missing=1
    continue
  fi
  if ! grep -Eq 'name[[:space:]]*=[[:space:]]*"replacement_archive"' "$build_file"; then
    echo "missing replacement_archive for ${manifest#${repo}/}"
    missing=1
  fi
done < <(find "$repo/services" "$repo/drivers/d1" "$repo/apps" -name '*.prototxt' -print | sort | xargs grep -l 'HEART_TRANSPLANT')

if [[ "$missing" -ne 0 ]]; then
  exit 1
fi
