#!/usr/bin/env bash
set -euo pipefail
exec python3 -B "$1" "$2" "$3"
