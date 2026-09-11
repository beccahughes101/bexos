#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 7 ]]; then
  echo "usage: $0 PROTOC PROTO_ROOT DEVICE_PROTO TRUSTY_BIN EMULATED_BIN EXPECTED_PROTO ARCH" >&2
  exit 2
fi

protoc_bin="$1"
proto_root="$2"
device_proto="$3"
trusty_bin="$4"
emulated_bin="$5"
expected_proto="$6"
architecture="$7"

check_secure_world() {
  local name="$1"
  local bin="$2"
  local expected="$3"
  local decoded
  decoded="$("${protoc_bin}" --proto_path="${proto_root}" --decode=bexos.platform.Device "${device_proto}" < "${bin}")"
  if [[ "${decoded}" != *"secure_world: ${expected}"* ]]; then
    echo "${name}: expected secure_world ${expected}" >&2
    echo "${decoded}" >&2
    exit 1
  fi
}

check_secure_world "qemu trusty device" "${trusty_bin}" TRUSTY
if [[ "$architecture" == x86_64 ]]; then
  decoded="$("$protoc_bin" --proto_path="$proto_root" --decode=bexos.platform.Device "$device_proto" < "$trusty_bin")"
  [[ "$decoded" == *"ARCH_X86_64"* ]] || { echo "missing x86 architecture" >&2; exit 1; }
fi
check_secure_world "qemu emulated device" "${emulated_bin}" EMULATED

if ! "${protoc_bin}" --proto_path="${proto_root}" --encode=bexos.platform.Device "${device_proto}" < "${expected_proto}" >/dev/null; then
  echo "qemu trusty device prototxt failed to re-encode" >&2
  exit 1
fi
