#!/usr/bin/env bash
set -euo pipefail
exec python3 "${RUNFILES_DIR:-$0.runfiles}/_main/third_party/ovmf/enroll.py" "$@"
