#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
for base in . "$script_dir/../../.." "$script_dir/../.." "${RUNFILES_DIR:-/nonexistent}"; do
  for tool in "$base"/external/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/llvm-ar "$base"/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/llvm-ar; do
    [ ! -x "$tool" ] || exec "$tool" "$@"
  done
done
echo "pinned Bazel LLVM llvm-ar runfile not found" >&2
exit 1
