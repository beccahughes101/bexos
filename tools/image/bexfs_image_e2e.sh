#!/usr/bin/env bash
set -euo pipefail

IMAGE_TOOL="$1"
DISK_TEMPLATE="$2"
KEY_FILE="$3"
WORK_IMAGE="${TEST_TMPDIR}/bexfs-sys-state.img"

cp "${DISK_TEMPLATE}" "${WORK_IMAGE}"
chmod u+rw "${WORK_IMAGE}"

COMMON=(
  --inspect
  --image "${WORK_IMAGE}"
  --partition SYS_STATE
  --label SYS_STATE
  --key-file "${KEY_FILE}"
  --path boot_state.bin
  --sys-state
)

"${IMAGE_TOOL}" "${COMMON[@]}" --expect-generation 1
"${IMAGE_TOOL}" "${COMMON[@]}" --set-generation 2 --set-active-slot B --expect-generation 2
"${IMAGE_TOOL}" "${COMMON[@]}" --expect-generation 2

if LC_ALL=C grep -aF "BEXSYS01" "${WORK_IMAGE}" >/dev/null; then
  echo "plaintext SYS_STATE magic is present in the encrypted disk image" >&2
  exit 1
fi

echo "bexfs image e2e: encrypted write, flush, remount, and inspection passed"
