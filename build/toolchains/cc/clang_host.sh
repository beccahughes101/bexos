#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
# Use the C driver for C sources and the C++ driver for C++ and link actions.
driver=clang++
for arg in "$@"; do
  case "$arg" in *.c) driver=clang ;; esac
done
for base in . "$script_dir/../../.." "$script_dir/../.." "${RUNFILES_DIR:-/nonexistent}"; do
  for tool in "$base"/external/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/"$driver" "$base"/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/"$driver"; do
    if [ -x "$tool" ]; then
      for resource in "${tool%/bin/*}"/lib/clang/*; do
        if [ -d "$resource/include" ]; then
          exec "$tool" -no-canonical-prefixes -resource-dir="$resource" "$@"
        fi
      done
    fi
  done
done
echo "Bazel native LLVM compiler runfile not found" >&2
exit 1
