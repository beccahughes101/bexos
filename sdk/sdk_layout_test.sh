#!/usr/bin/env bash
set -euo pipefail

archive="$1"
checksum="$2"
listing="${TEST_TMPDIR:-/tmp}/sdk-listing"
verbose="${TEST_TMPDIR:-/tmp}/sdk-listing-verbose"
tar -tzf "$archive" > "$listing"
tar -tvzf "$archive" > "$verbose"

for path in \
  bexos-sdk/MODULE.bazel \
  bexos-sdk/sdk-Cargo.lock \
  bexos-sdk/sdk-crates-lock.json \
  bexos-sdk/examples/BUILD.bazel \
  bexos-sdk/examples/wasm_service.rs \
  bexos-sdk/examples/native_service.rs \
  bexos-sdk/examples/driver.rs \
  bexos-sdk/meta/sdk.prototxt \
  bexos-sdk/rules/defs.bzl \
  bexos-sdk/rust/src/lib.rs \
  bexos-sdk/rust/component.rs \
  bexos-sdk/sysroot/aarch64/include/bexos/component.h \
  bexos-sdk/sysroot/aarch64/lib/crt0.o \
  bexos-sdk/sysroot/aarch64/lib/libc.a \
  bexos-sdk/sysroot/aarch64/lib/libbexos_runtime.a \
  bexos-sdk/sysroot/aarch64/licenses/GNU-LIBC.txt \
  bexos-sdk/sysroot/x86_64/include/bexos/component.h \
  bexos-sdk/sysroot/x86_64/lib/crt0.o \
  bexos-sdk/sysroot/x86_64/lib/libc.a \
  bexos-sdk/sysroot/x86_64/lib/libbexos_runtime.a \
  bexos-sdk/sysroot/x86_64/licenses/GNU-LIBC.txt \
  bexos-sdk/rust/wasm.wit \
  bexos-sdk/rust/wasm_guest.rs \
  bexos-sdk/tools/bin/bex_archive \
  bexos-sdk/tools/bin/bexos_assembly \
  bexos-sdk/tools/bin/config_compiler \
  bexos-sdk/tools/bin/fidlc \
  bexos-sdk/tools/bin/manifest_stamp; do
  grep -Fxq "$path" "$listing"
done

if grep -Eq '\.(key|pem)$' "$listing"; then
  echo "SDK contains private-key material" >&2
  exit 1
fi
awk '$1 ~ /^-rwxr-xr-x/ && $NF == "bexos-sdk/tools/bin/fidlc" { found=1 } END { exit !found }' "$verbose"
metadata="$(tar -xOzf "$archive" bexos-sdk/meta/sdk.prototxt)"
grep -Fq 'sdk_version { major: 0 minor: 2 patch: 0 }' <<< "$metadata"
grep -Fq 'api_level: 1' <<< "$metadata"
grep -Fq 'target_architectures: "aarch64"' <<< "$metadata"
grep -Fq 'target_architectures: "x86_64"' <<< "$metadata"
grep -Fq 'archive_format: "BEXARCV2"' <<< "$metadata"
extract="${TEST_TMPDIR:-/tmp}/sdk-sysroot"
mkdir -p "$extract"
tar -xzf "$archive" -C "$extract" \
  bexos-sdk/sysroot/aarch64/lib/libc.a \
  bexos-sdk/sysroot/x86_64/lib/libc.a
nm "$extract/bexos-sdk/sysroot/aarch64/lib/libc.a" | grep -Eq ' [Tt] memcpy$'
nm "$extract/bexos-sdk/sysroot/x86_64/lib/libc.a" | grep -Eq ' [Tt] memcpy$'
expected="$(awk '{print $1}' "$checksum")"
actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
test "$expected" = "$actual"
