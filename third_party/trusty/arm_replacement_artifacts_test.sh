#!/bin/sh
set -eu
tool="$1"
root="$2"
shift 2
for candidate in "$@"; do
    "$tool" --verify-firmware "$candidate" "$root" aarch64 trusty 0 0
done
