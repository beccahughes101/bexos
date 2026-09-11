#!/usr/bin/env bash
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
exec python3 "${RUNFILES_DIR:-$0.runfiles}/_main/testing/performance/scened/telemetry_test.py"
