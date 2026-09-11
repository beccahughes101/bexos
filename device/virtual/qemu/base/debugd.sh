#!/usr/bin/env bash
set -euo pipefail

bexctl="$1"
product="$2"
architecture="$3"
shift 3

exec "$bexctl" --socket "/tmp/bexos-qemu-${product}-${architecture}-debugd.sock" "$@"
