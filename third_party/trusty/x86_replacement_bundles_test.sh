#!/bin/sh
set -eu
tool="$1"
shift
work=$(mktemp -d "${TEST_TMPDIR:-/tmp}/bexos-x86-bundles.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
index=0
while [ "$#" -ne 0 ]; do
    variant="$1"
    bundle="$2"
    shift 2
    destination="$work/$index"
    "$tool" --architecture x86_64 --variant "$variant" extract "$destination" "$bundle"
    index=$((index + 1))
done
