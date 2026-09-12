#!/usr/bin/env bash
set -euo pipefail

# The fixture genrule exercises the compiler in Bazel's execution configuration.
test -s "$1"
test "$(wc -c < "$2")" -eq 32
