#!/usr/bin/env bash
set -euo pipefail
source_dir=$(cd "$(dirname "$1")" && pwd)
output=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
compiler=$(cd "$(dirname "$3")" && pwd)/$(basename "$3")
work=$(mktemp -d "${TMPDIR:-/tmp}/bexos-ninja.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
cp -RL "$source_dir" "$work/source"
cd "$work/source"
export CXX="$compiler"
if [[ $(uname -s) == Darwin ]]; then export SDKROOT=$(xcrun --show-sdk-path); fi
python3 -B configure.py --bootstrap
cp ninja "$output"
