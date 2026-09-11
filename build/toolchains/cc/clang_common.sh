#!/bin/sh
set -eu
triple="$1"
shift
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
shim_include="$script_dir/include"
# Bazel's depfile validation requires workspace inputs to stay relative to the
# action root; an absolute sandbox path changes on every compile action.
if [ -f build/toolchains/cc/include/assert.h ]; then
  shim_include=build/toolchains/cc/include
fi
for base in . "$script_dir/../../.." "$script_dir/../.." "${RUNFILES_DIR:-/nonexistent}"; do
  for tool in "$base"/external/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/clang "$base"/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/clang; do
    if [ -x "$tool" ]; then
      for resource in "${tool%/bin/clang}"/lib/clang/*; do
        if [ -d "$resource/include" ]; then
          exec "$tool" -no-canonical-prefixes -resource-dir="$resource" --target="$triple" --sysroot=/dev/null -ffreestanding -nostdlib -I"$shim_include" "$@"
        fi
      done
    fi
  done
done
echo "Bazel LLVM clang runfile not found" >&2
exit 1
