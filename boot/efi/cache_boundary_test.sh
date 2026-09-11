#!/usr/bin/env bash
set -euo pipefail
if [[ -s "$1" ]]; then
    echo 'Ordinary x86 product or E2E build depends on a firmware compiler:' >&2
    cat "$1" >&2
    exit 1
fi
