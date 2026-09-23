#!/usr/bin/env bash
set -euo pipefail
exec python3 -B "$TEST_SRCDIR/$TEST_WORKSPACE/secure/platform/layout_test.py"
