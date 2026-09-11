#!/bin/sh
set -eu

tool="$1"
key="$2"
test_root="${TEST_TMPDIR}/vbmeta-test"
mkdir -p "$test_root"
printf kernel > "$test_root/kernel"
printf bootfs > "$test_root/bootfs"
printf policy > "$test_root/policy"
"$tool" "$key" "$test_root/vbmeta" "$test_root/public-key" 7 \
  "kernel=$test_root/kernel" "bootfs=$test_root/bootfs" "platform-policy=$test_root/policy"
"$tool" --verify "$test_root/vbmeta" "$test_root/public-key" \
  "kernel=$test_root/kernel" "bootfs=$test_root/bootfs" "platform-policy=$test_root/policy"
"$tool" --public-key "$key" "$test_root/independent-public-key"
cmp "$test_root/public-key" "$test_root/independent-public-key"
printf tampered > "$test_root/kernel"
if "$tool" --verify "$test_root/vbmeta" "$test_root/public-key" "kernel=$test_root/kernel"; then
  echo "tampered partition was accepted" >&2
  exit 1
fi
