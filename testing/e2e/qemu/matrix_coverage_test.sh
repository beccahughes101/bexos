#!/usr/bin/env bash
set -euo pipefail
if [[ -s "$1" ]]; then
    echo 'QEMU scenarios missing from maintained E2E suites:' >&2
    cat "$1" >&2
    exit 1
fi
