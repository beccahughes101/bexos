#!/bin/sh
set -eu
tool="$1"
root="$2"
"$tool" --verify-firmware "$3" "$root" x86_64 trusty 0 0
shift 3
for candidate in "$@"; do
    "$tool" --verify-firmware "$candidate" "$root" x86_64 hypervisor 0 0
done
