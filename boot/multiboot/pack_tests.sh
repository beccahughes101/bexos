#!/bin/sh
set -eu
exec python3 "$(dirname "$0")/pack_tests.py"
