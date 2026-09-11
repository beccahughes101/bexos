#!/bin/sh
set -eu
export PYTHONDONTWRITEBYTECODE=1
exec python3 "$TEST_SRCDIR/$TEST_WORKSPACE/testing/e2e/qemu/graphics/input_hotplug_test.py"
