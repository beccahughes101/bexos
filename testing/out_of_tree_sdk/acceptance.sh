#!/usr/bin/env bash
set -euo pipefail

architecture="${1:-aarch64}"
mode="${2:-qemu}"
case "$architecture" in
  aarch64|x86_64) ;;
  *) echo "unsupported architecture: $architecture" >&2; exit 2 ;;
esac

workspace="${BUILD_WORKSPACE_DIRECTORY:-}"
if [[ -z "$workspace" ]]; then
  module_runfile="${RUNFILES_DIR:-}/${TEST_WORKSPACE:-bexos}/MODULE.bazel"
  workspace="$(dirname "$(realpath "$module_runfile")")"
fi
bazel_bin="${BAZEL:-$(command -v bazel)}"
root_bazel_args=()
if [[ -n "${BEXOS_BAZEL_OUTPUT_USER_ROOT:-}" ]]; then
  if [[ "$BEXOS_BAZEL_OUTPUT_USER_ROOT" != /* ]]; then
    echo "BEXOS_BAZEL_OUTPUT_USER_ROOT must be an absolute path" >&2
    exit 2
  fi
  mkdir -p "$BEXOS_BAZEL_OUTPUT_USER_ROOT"
  root_bazel_args+=("--output_user_root=$BEXOS_BAZEL_OUTPUT_USER_ROOT")
fi
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bexos-sdk-acceptance.XXXXXX")"
cleanup() {
  status=$?
  trap - EXIT
  if [[ -d "$tmp" ]]; then
    chmod -R u+w "$tmp" 2>/dev/null || true
    rm -rf "$tmp" 2>/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT

cd "$workspace"
"$bazel_bin" "${root_bazel_args[@]}" build //sdk:bexos_sdk
sdk_archive="$("$bazel_bin" "${root_bazel_args[@]}" cquery --output=files //sdk:bexos_sdk | tail -n 1)"
tar -xzf "$sdk_archive" -C "$tmp"
cp -R "$workspace/testing/out_of_tree_sdk/fixture" "$tmp/fixture"
cp "$tmp/fixture/BUILD.sdk.bazel" "$tmp/fixture/BUILD.bazel"
cp "$tmp/fixture/MODULE.sdk.bazel" "$tmp/fixture/MODULE.bazel"

fixture_output="$tmp/fixture-output"
cd "$tmp/fixture"
"$bazel_bin" --output_base="$fixture_output" build \
  --override_module="bexos_sdk=$tmp/bexos-sdk" \
  --override_repository="bexos_sdk=$tmp/bexos-sdk" //:archives
wasm_archive="$("$bazel_bin" --output_base="$fixture_output" cquery \
  --override_module="bexos_sdk=$tmp/bexos-sdk" \
  --override_repository="bexos_sdk=$tmp/bexos-sdk" --output=files //:wasm_app | tail -n 1)"
native_archive="$("$bazel_bin" --output_base="$fixture_output" cquery \
  --override_module="bexos_sdk=$tmp/bexos-sdk" \
  --override_repository="bexos_sdk=$tmp/bexos-sdk" --output=files //:native_${architecture}_app | tail -n 1)"
service_archive="$("$bazel_bin" --output_base="$fixture_output" cquery \
  --override_module="bexos_sdk=$tmp/bexos-sdk" \
  --override_repository="bexos_sdk=$tmp/bexos-sdk" --output=files //:service_${architecture} | tail -n 1)"
service_replacement_archive="$("$bazel_bin" --output_base="$fixture_output" cquery \
  --override_module="bexos_sdk=$tmp/bexos-sdk" \
  --override_repository="bexos_sdk=$tmp/bexos-sdk" --output=files //:service_replacement_${architecture} | tail -n 1)"
driver_archive="$("$bazel_bin" --output_base="$fixture_output" cquery \
  --override_module="bexos_sdk=$tmp/bexos-sdk" \
  --override_repository="bexos_sdk=$tmp/bexos-sdk" --output=files //:driver_${architecture} | tail -n 1)"
driver_replacement_archive="$("$bazel_bin" --output_base="$fixture_output" cquery \
  --override_module="bexos_sdk=$tmp/bexos-sdk" \
  --override_repository="bexos_sdk=$tmp/bexos-sdk" --output=files //:driver_replacement_${architecture} | tail -n 1)"

prebuilt="$tmp/prebuilt"
mkdir -p "$prebuilt"
cp "$wasm_archive" "$prebuilt/wasm_app.bex"
cp "$native_archive" "$prebuilt/native_${architecture}_app.bex"
cp "$native_archive" "$prebuilt/native_$([[ "$architecture" == aarch64 ]] && echo x86_64 || echo aarch64)_app.bex"
cp "$service_archive" "$prebuilt/service_${architecture}.bex"
cp "$service_archive" "$prebuilt/service_$([[ "$architecture" == aarch64 ]] && echo x86_64 || echo aarch64).bex"
cp "$service_replacement_archive" "$prebuilt/service_replacement.bex"
cp "$driver_archive" "$prebuilt/driver_${architecture}.bex"
cp "$driver_archive" "$prebuilt/driver_$([[ "$architecture" == aarch64 ]] && echo x86_64 || echo aarch64).bex"
cp "$driver_replacement_archive" "$prebuilt/driver_replacement.bex"
printf '%s\n' 'module(name = "bexos_sdk_fixture_prebuilt", version = "0.0.0")' > "$prebuilt/MODULE.bazel"
printf '%s\n' 'package(default_visibility = ["//visibility:public"])' 'exports_files(glob(["*.bex"]))' > "$prebuilt/BUILD.bazel"
"$bazel_bin" --output_base="$fixture_output" shutdown

if [[ "$mode" == "smoke" ]]; then
  echo "out-of-tree SDK fixture built for $architecture"
  exit 0
fi

cd "$workspace"
"$bazel_bin" "${root_bazel_args[@]}" test \
  --config=e2e \
  --override_repository="bexos_sdk_fixture_prebuilt=$prebuilt" \
  --test_output=streamed \
  "//testing/e2e/qemu/sdk:sdk_acceptance_${architecture}"
