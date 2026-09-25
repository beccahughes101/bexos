#!/bin/sh
set -eu
triple="$1"
arch="$2"
shift 2
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
for base in . "$script_dir/../../.." "$script_dir/../.." "${RUNFILES_DIR:-/nonexistent}"; do
  for clang in "$base"/external/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/clang "$base"/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/clang; do
    if [ -x "$clang" ]; then
      for resource in "${clang%/bin/clang}"/lib/clang/*; do
        if [ -d "$resource/include" ]; then
          exec "$clang" -no-canonical-prefixes -resource-dir="$resource" --target="$triple" -ffreestanding -nostdlib -isystem "$script_dir/../sysroot/$arch/include" "$@"
        fi
      done
    fi
  done
done
echo "pinned Bazel LLVM clang runfile not found" >&2
exit 1
