#!/bin/sh
set -eu
exec python3 -B "$(dirname "$0")/run_x86_tests.py"
