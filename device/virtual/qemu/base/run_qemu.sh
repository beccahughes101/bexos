#!/usr/bin/env bash
set -euo pipefail

runner="$1"
shift

exec "$runner" "$@"
