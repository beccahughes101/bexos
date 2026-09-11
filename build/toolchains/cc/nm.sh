#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
for base in "$script_dir/../../.." "$script_dir/../.." "${RUNFILES_DIR:-/nonexistent}"; do
  for tool in "$base"/external/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/llvm-nm "$base"/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/llvm-nm; do
    if [ -x "$tool" ]; then
      exec "$tool" "$@"
    fi
  done
done
echo "Bazel LLVM llvm-nm runfile not found" >&2
exit 1
