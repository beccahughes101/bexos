#!/usr/bin/env bash
set -euo pipefail

aarch64_elf="$1"
x86_64_elf="$2"
nm="$3"

test -s "$aarch64_elf"
test -s "$x86_64_elf"
"$nm" "$aarch64_elf" | grep -Eq ' [Tt] _start$'
"$nm" "$aarch64_elf" | grep -Eq ' [Tt] memcpy$'
"$nm" "$x86_64_elf" | grep -Eq ' [Tt] _start$'
"$nm" "$x86_64_elf" | grep -Eq ' [Tt] memcpy$'
