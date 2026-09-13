#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
archiver=llvm-ar
# Bazel's Darwin archive action uses libtool's command-line interface.
if [ "${1:-}" = "-static" ]; then
  archiver=llvm-libtool-darwin
fi
for base in "$script_dir/../../.." "$script_dir/../.." "${RUNFILES_DIR:-/nonexistent}"; do
  for tool in "$base"/external/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/"$archiver" "$base"/llvm++llvm_toolchain_minimal+llvm-toolchain-minimal-*/bin/"$archiver"; do
    if [ -x "$tool" ]; then
      exec "$tool" "$@"
    fi
  done
done
echo "Bazel LLVM $archiver runfile not found" >&2
exit 1
